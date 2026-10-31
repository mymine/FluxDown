//! 共享：多对端取块、部分内容提供、坏对端封禁、原分享方缺失时的回落，以及提供端的授权检查。

use std::time::Duration;

use fluxdown_share::swarm::wire::{SwarmReply, SwarmRequest, SwarmStatus};
use fluxdown_share::ticket::sign_ticket;
use fluxdown_share::wire::{read_frame, write_frame};
use fluxdown_share::{
    CHUNK_SIZE, ChunkSet, ReceiveSession, SWARM_ALPN, SessionEvent, SessionReport, SessionRole,
    SwarmFile, SwarmPeer, SwarmServedFile,
};
use iroh::endpoint::{Connection, ConnectionError, RecvStream};
use reqwest::StatusCode;

use crate::common::{
    Peer, Rig, clone_file, far_future, http, loopback_hint, partial_peer, payload, peer,
    provide_peer, rig, rig_with, root_of, swarm_ticket_for,
};

const MIB: usize = 1024 * 1024;

/// 等到就绪的共享对端数达到 `n`。
async fn wait_peers(session: &ReceiveSession, n: u32) {
    let mut rx = session.watch_swarm_peers();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if *rx.borrow_and_update() >= n {
                break;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .expect("swarm peers did not become ready");
}

/// 给接收会话挂上 `peers`（都在回环上）。
async fn attach(rig: &Rig, peers: &[(&Peer, bool)]) {
    let root = rig.file.manifest.files[0].root_hash.clone();
    let ticket = swarm_ticket_for(
        &rig.signer,
        &rig.file.manifest.share_id,
        &rig.receiver.ep,
        &root,
        far_future(),
    );
    let mut list = Vec::new();
    for (p, complete) in peers {
        list.push(SwarmPeer {
            endpoint_id: p.ep.id(),
            complete: *complete,
            addr_hint: Some(loopback_hint(&p.ep).await),
        });
    }
    rig.session.set_swarm(vec![SwarmFile {
        file_index: 0,
        ticket,
        peers: list,
    }]);
}

async fn get_range(url: &str, range: &str) -> reqwest::Response {
    http().get(url).header("Range", range).send().await.unwrap()
}

/// 原分享方连不上：整个文件由一个持有完整内容的对端提供，字节一致，来源计量为 `peer`。
#[tokio::test]
async fn peer_serves_whole_file_when_origin_is_offline() {
    let rig = rig_with("swarm-offline", 20 * MIB, |p| {
        p.addr_hint = None;
        p.connect_timeout = Duration::from_millis(1500);
    })
    .await;
    let provider = provide_peer(rig.signer.ring.clone(), &rig.file);
    attach(&rig, &[(&provider, true)]).await;
    wait_peers(&rig.session, 1).await;

    let resp = http().get(rig.url()).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.bytes().await.unwrap();
    assert_eq!(&body[..], &rig.file.data[..]);
    assert_eq!(rig.session.source_bytes().peer, (20 * MIB) as u64);
    let summary = rig.session.summary().await.unwrap();
    assert_eq!(summary.bytes.peer, (20 * MIB) as u64);
    // 本会话已校验的块都登记在位图里。
    let have = rig.session.have(0).unwrap();
    assert_eq!(have.count(), 5);
}

/// 原分享方与两个对端同时在线：并发区间请求的块分散到多条路线，结果逐字节一致。
#[tokio::test]
async fn concurrent_ranges_spread_over_origin_and_two_peers() {
    let rig = rig("swarm-spread", 32 * MIB).await;
    let provide_a = provide_peer(rig.signer.ring.clone(), &rig.file);
    let provide_b = provide_peer(rig.signer.ring.clone(), &rig.file);
    attach(&rig, &[(&provide_a, true), (&provide_b, true)]).await;
    wait_peers(&rig.session, 2).await;
    assert_eq!(rig.session.swarm_peers(), 2);

    let url = rig.url();
    let part = 8 * MIB;
    let ranges: Vec<String> = (0..4)
        .map(|i| format!("bytes={}-{}", i * part, (i + 1) * part - 1))
        .collect();
    let mut tasks = Vec::new();
    for range in ranges {
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            let resp = get_range(&url, &range).await;
            assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
            resp.bytes().await.unwrap().to_vec()
        }));
    }
    let mut assembled = Vec::new();
    for task in tasks {
        assembled.extend(task.await.unwrap());
    }
    assert_eq!(assembled, rig.file.data);
    let peer_bytes = rig.session.source_bytes().peer;
    assert!(
        peer_bytes > 0 && peer_bytes < (32 * MIB) as u64,
        "peer bytes {peer_bytes} should be a strict share of the file"
    );
}

/// 部分内容的对端只提供它持有且磁盘校验通过的块；位图里谎称持有的块回 `Missing`，
/// 没有其他路线时适配器回 503。
#[tokio::test]
async fn partial_peer_serves_only_chunks_it_verifies() {
    let rig = rig_with("swarm-partial", 20 * MIB, |p| {
        p.addr_hint = None;
        p.connect_timeout = Duration::from_millis(1500);
    })
    .await;
    let partial = partial_peer(rig.signer.ring.clone(), &rig.file, &[0, 1, 2], &[3]);
    attach(&rig, &[(&partial, false)]).await;
    wait_peers(&rig.session, 1).await;

    let url = rig.url();
    let head = get_range(&url, &format!("bytes=0-{}", 3 * CHUNK_SIZE - 1)).await;
    assert_eq!(head.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        &head.bytes().await.unwrap()[..],
        &rig.file.data[..3 * MIB * 4]
    );
    assert_eq!(rig.session.source_bytes().peer, 3 * CHUNK_SIZE);

    let lying = get_range(
        &url,
        &format!("bytes={}-{}", 3 * CHUNK_SIZE, 4 * CHUNK_SIZE - 1),
    )
    .await;
    assert_eq!(lying.status(), StatusCode::SERVICE_UNAVAILABLE);
    // 第 4 块对端根本没有（位图也不含）：同样没有路线。
    let absent = get_range(&url, &format!("bytes={}-", 4 * CHUNK_SIZE)).await;
    assert_eq!(absent.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(rig.session.source_bytes().peer, 3 * CHUNK_SIZE);
}

/// 对端文件被篡改（内容表里登记了篡改后文件的真实修改时间，提供端不会察觉）：
/// 接收方每块校验失败，对端两次后被封禁，原分享方接手，最终字节正确。
#[tokio::test]
async fn corrupt_peer_is_banned_and_origin_takes_over() {
    let rig = rig("swarm-corrupt", 64 * MIB).await;
    let mut tampered = rig.file.data.clone();
    for byte in tampered.iter_mut().step_by(4096) {
        *byte ^= 0xFF;
    }
    let bad_file = clone_file(&rig.file, tampered);
    let bad = provide_peer(rig.signer.ring.clone(), &bad_file);
    assert_eq!(root_of(&bad_file), root_of(&rig.file));
    attach(&rig, &[(&bad, true)]).await;
    wait_peers(&rig.session, 1).await;

    // 16 块并发取：直连有负载时对端会被选中，坏数据被拒绝并回落到原分享方。
    let chunks = rig.file.manifest.files[0].chunk_count();
    let mut tasks = Vec::new();
    for i in 0..chunks {
        let session = rig.session.clone();
        tasks.push(tokio::spawn(async move {
            session.fetch_chunk(0, i).await.unwrap()
        }));
    }
    let mut assembled = Vec::new();
    for task in tasks {
        assembled.extend_from_slice(&task.await.unwrap());
    }
    assert_eq!(assembled, rig.file.data);
    let summary = rig.session.summary().await.unwrap();
    assert!(
        summary.verify_failures >= 2,
        "verify_failures = {}",
        summary.verify_failures
    );
    assert_eq!(rig.session.source_bytes().peer, 0, "no corrupt byte counts");
    // 封禁之后不再有就绪对端。
    assert_eq!(rig.session.swarm_peers(), 0);
}

async fn ask(conn: &Connection, request: &SwarmRequest) -> (SwarmReply, RecvStream) {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, request).await.unwrap();
    send.finish().unwrap();
    let reply = read_frame(&mut recv).await.unwrap();
    (reply, recv)
}

fn grant(ticket: String) -> SwarmRequest {
    SwarmRequest::Grant { ticket }
}

/// 提供端的授权检查：作用域、`rep`、根哈希、内容表，以及未授权不得取数。
#[tokio::test]
async fn grant_checks_scope_peer_root_and_registry() {
    let rig = rig("swarm-grant", 5 * MIB).await;
    let provider = provide_peer(rig.signer.ring.clone(), &rig.file);
    let hint = loopback_hint(&provider.ep).await;
    let endpoint = rig.receiver.ep.ensure_bound().await.unwrap();
    let conn = endpoint.connect(hint, SWARM_ALPN).await.unwrap();
    let root = rig.file.manifest.files[0].root_hash.clone();
    let sid = "share-x";
    let receiver_z32 = rig.receiver.ep.id_z32();

    // recv 作用域的票据不能用来共享。
    let recv = sign_ticket(
        &rig.signer.secret,
        &payload(sid, &receiver_z32, far_future()),
    )
    .unwrap();
    assert_eq!(
        ask(&conn, &grant(recv)).await.0.status,
        SwarmStatus::BadTicket
    );

    // 缺少 root。
    let mut no_root = payload(sid, &receiver_z32, far_future());
    no_root.scope = "swarm".into();
    let no_root = sign_ticket(&rig.signer.secret, &no_root).unwrap();
    assert_eq!(
        ask(&conn, &grant(no_root)).await.0.status,
        SwarmStatus::BadTicket
    );

    // `rep` 是别人。
    let other = peer(rig.signer.ring.clone());
    let foreign = swarm_ticket_for(&rig.signer, sid, &other.ep, &root, far_future());
    assert_eq!(
        ask(&conn, &grant(foreign)).await.0.status,
        SwarmStatus::PeerMismatch
    );

    // 已过期。
    let expired = swarm_ticket_for(&rig.signer, sid, &rig.receiver.ep, &root, 1);
    assert_eq!(
        ask(&conn, &grant(expired)).await.0.status,
        SwarmStatus::Expired
    );

    // 本机不持有该内容。
    let unknown = swarm_ticket_for(
        &rig.signer,
        sid,
        &rig.receiver.ep,
        &"ff".repeat(32),
        far_future(),
    );
    assert_eq!(
        ask(&conn, &grant(unknown)).await.0.status,
        SwarmStatus::NotFound
    );

    // 未授权就取数 / 查位图。
    let get = SwarmRequest::Get {
        root: root.clone(),
        chunk: 0,
    };
    assert_eq!(ask(&conn, &get).await.0.status, SwarmStatus::NotGranted);
    let have = SwarmRequest::Have { root: root.clone() };
    assert_eq!(ask(&conn, &have).await.0.status, SwarmStatus::NotGranted);

    // 正确授权之后：完整内容、取块、越界、查位图。
    let good = swarm_ticket_for(&rig.signer, sid, &rig.receiver.ep, &root, far_future());
    let (granted, _) = ask(&conn, &grant(good)).await;
    assert_eq!(granted.status, SwarmStatus::Ok);
    assert!(granted.complete);
    assert_eq!(granted.size, (5 * MIB) as u64);

    let (reply, mut data) = ask(&conn, &get).await;
    assert_eq!(reply.status, SwarmStatus::Ok);
    assert_eq!(reply.len, CHUNK_SIZE);
    let mut buf = vec![0u8; usize::try_from(reply.len).unwrap()];
    data.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf[..], &rig.file.data[..buf.len()]);

    let last = SwarmRequest::Get {
        root: root.clone(),
        chunk: 1,
    };
    assert_eq!(ask(&conn, &last).await.0.len, (MIB) as u64);
    let beyond = SwarmRequest::Get {
        root: root.clone(),
        chunk: 2,
    };
    assert_eq!(ask(&conn, &beyond).await.0.status, SwarmStatus::BadRequest);
    let (have_reply, _) = ask(&conn, &have).await;
    assert_eq!(have_reply.status, SwarmStatus::Ok);
    assert!(have_reply.complete);
    assert_eq!(have_reply.len, 0);
    let bad_root = SwarmRequest::Have {
        root: "not-hex".into(),
    };
    assert_eq!(
        ask(&conn, &bad_root).await.0.status,
        SwarmStatus::BadRequest
    );

    // 完整内容的文件被删：Get 回 `changed`，提供端发出 ContentChanged。
    let mut provider = provider;
    std::fs::remove_file(&rig.file.path).unwrap();
    assert_eq!(ask(&conn, &get).await.0.status, SwarmStatus::Changed);
    let mut saw = false;
    while let Ok(event) = provider.events.try_recv() {
        if let fluxdown_share::SessionEvent::ContentChanged { root_hash } = event {
            assert_eq!(root_hash, root);
            saw = true;
        }
    }
    assert!(saw, "ContentChanged was not emitted");
}

/// 原分享方的文件被改（它回 `changed`）：有对端时改走对端，传输仍然成功，不出现 409。
#[tokio::test]
async fn origin_changed_file_falls_back_to_peers() {
    let rig = rig("swarm-changed", 12 * MIB).await;
    // 对端持有自己的副本（路径独立）。
    let copy = clone_file(&rig.file, rig.file.data.clone());
    let provider = provide_peer(rig.signer.ring.clone(), &copy);
    attach(&rig, &[(&provider, true)]).await;
    wait_peers(&rig.session, 1).await;

    // 原分享方的源文件被追加一个字节：大小不符 → changed。
    let mut changed = rig.file.data.clone();
    changed.push(0);
    std::fs::write(&rig.file.path, &changed).unwrap();

    let resp = http().get(rig.url()).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(&resp.bytes().await.unwrap()[..], &rig.file.data[..]);
    assert_eq!(rig.session.source_bytes().peer, (12 * MIB) as u64);
}

/// 位图的线上位序、往返，以及长度 / 越界位的拒绝。
#[test]
fn chunkset_bytes_round_trip_and_reject_bad_lengths() {
    let set = ChunkSet::new(10);
    assert!(set.insert(0));
    assert!(!set.insert(0), "second insert is not new");
    assert!(set.insert(9));
    assert!(!set.insert(10), "out of range");
    assert!(!set.contains(10));
    assert_eq!(set.count(), 2);
    // 第 i 块 ↔ 第 i/8 字节的 1 << (i%8) 位。
    assert_eq!(set.to_bytes(), vec![0b0000_0001, 0b0000_0010]);

    let back = ChunkSet::from_bytes(10, &set.to_bytes()).unwrap();
    assert_eq!(back.count(), 2);
    assert!(back.contains(0) && back.contains(9) && !back.contains(5));

    // 长度必须恰好是 ceil(len / 8)。
    assert!(ChunkSet::from_bytes(10, &[0]).is_none());
    assert!(ChunkSet::from_bytes(10, &[0, 0, 0]).is_none());
    // 超出 len 的位不得置位。
    assert!(ChunkSet::from_bytes(10, &[0, 0b0000_0100]).is_none());
    assert!(ChunkSet::from_bytes(10, &[0xFF, 0b0000_0011]).is_some());

    assert!(set.remove(0));
    assert!(!set.remove(0));
    assert_eq!(set.count(), 1);

    // 字边界。
    let wide = ChunkSet::new(65);
    assert!(wide.insert(64));
    assert_eq!(wide.to_bytes().len(), 9);
    assert_eq!(wide.to_bytes()[8], 1);
    assert!(
        ChunkSet::from_bytes(65, &wide.to_bytes())
            .unwrap()
            .contains(64)
    );
    let empty = ChunkSet::new(0);
    assert!(empty.is_empty());
    assert!(empty.to_bytes().is_empty());
    assert!(ChunkSet::from_bytes(0, &[]).is_some());
}

/// 已向 `provider` 完成一次 swarm 授权的连接。
struct Granted {
    rig: Rig,
    provider: Peer,
    conn: Connection,
    root: String,
}

const SWARM_SID: &str = "swarm-session";

async fn granted_connection() -> Granted {
    let rig = rig(SWARM_SID, 5 * MIB).await;
    let provider = provide_peer(rig.signer.ring.clone(), &rig.file);
    let hint = loopback_hint(&provider.ep).await;
    let endpoint = rig.receiver.ep.ensure_bound().await.unwrap();
    let conn = endpoint.connect(hint, SWARM_ALPN).await.unwrap();
    let root = rig.file.manifest.files[0].root_hash.clone();
    let ticket = swarm_ticket_for(
        &rig.signer,
        SWARM_SID,
        &rig.receiver.ep,
        &root,
        far_future(),
    );
    assert_eq!(ask(&conn, &grant(ticket)).await.0.status, SwarmStatus::Ok);
    Granted {
        rig,
        provider,
        conn,
        root,
    }
}

/// 取一块并读到流结束（服务端在 `finish` 之后才记账，读到 FIN 即保证已记账）。
async fn fetch_chunk(conn: &Connection, root: &str, chunk: u64) -> u64 {
    let request = SwarmRequest::Get {
        root: root.to_owned(),
        chunk,
    };
    let (reply, mut data) = ask(conn, &request).await;
    assert_eq!(reply.status, SwarmStatus::Ok);
    let body = data
        .read_to_end(usize::try_from(CHUNK_SIZE).unwrap() + 1)
        .await
        .unwrap();
    assert_eq!(body.len() as u64, reply.len);
    reply.len
}

async fn wait_final_swarm_report(provider: &Peer) -> SessionReport {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let found = provider
                .sink
                .sessions
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.is_final && r.role == SessionRole::Swarm)
                .cloned();
            if let Some(report) = found {
                return report;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("no final swarm report")
}

async fn opened_swarm_session(provider: &mut Peer) -> (String, String, String) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let SessionEvent::Opened {
                session_id,
                share_id,
                role: SessionRole::Swarm,
                peer_endpoint_id,
            } = provider.events.recv().await.expect("event channel closed")
            {
                return (session_id, share_id, peer_endpoint_id);
            }
        }
    })
    .await
    .expect("no swarm session opened")
}

/// 提供方连接是一个 role=swarm 的会话：分享 ID 取票据 `sid`，按内容累计只计成功发出的 `Get` 数据，
/// 连接结束发终态报告（路径字节与报告里的服务量一致）。
#[tokio::test]
async fn swarm_provider_reports_served_bytes_per_root_and_final() {
    let mut g = granted_connection().await;
    let (session_id, share_id, peer) = opened_swarm_session(&mut g.provider).await;
    assert_eq!(share_id, SWARM_SID);
    assert_eq!(peer, g.rig.receiver.ep.id_z32());

    // 块 0（4 MiB）、块 1（1 MiB）、块 0 再取一次：累计 9 MiB / 3 块。
    let mut served = 0;
    served += fetch_chunk(&g.conn, &g.root, 0).await;
    served += fetch_chunk(&g.conn, &g.root, 1).await;
    served += fetch_chunk(&g.conn, &g.root, 0).await;
    assert_eq!(served, (9 * MIB) as u64);
    // 越界、未授权的内容、位图查询都不计入服务量。
    let beyond = SwarmRequest::Get {
        root: g.root.clone(),
        chunk: 2,
    };
    assert_eq!(
        ask(&g.conn, &beyond).await.0.status,
        SwarmStatus::BadRequest
    );
    let other = SwarmRequest::Get {
        root: "ff".repeat(32),
        chunk: 0,
    };
    assert_eq!(ask(&g.conn, &other).await.0.status, SwarmStatus::NotGranted);
    let have = SwarmRequest::Have {
        root: g.root.clone(),
    };
    assert_eq!(ask(&g.conn, &have).await.0.status, SwarmStatus::Ok);

    g.conn.close(0u32.into(), b"done");
    let report = wait_final_swarm_report(&g.provider).await;
    assert_eq!(report.session_id, session_id);
    assert_eq!(report.share_id, SWARM_SID);
    assert_eq!(report.role, SessionRole::Swarm);
    assert_eq!(report.peer_endpoint_id, g.rig.receiver.ep.id_z32());
    assert_eq!(
        report.swarm_files,
        vec![SwarmServedFile {
            root_hash: g.root.clone(),
            bytes: served,
            chunks: 3,
        }]
    );
    assert_eq!(report.summary.error, None);
    // 发出的 UDP 字节不少于应用层数据（路径统计按发送方向计）。
    assert!(report.summary.bytes.direct + report.summary.bytes.relay >= served);
    // 只有这一份终态报告，且所有报告都是 swarm 角色。
    let reports = g.provider.sink.sessions.lock().unwrap().clone();
    assert_eq!(reports.iter().filter(|r| r.is_final).count(), 1);
    assert!(reports.iter().all(|r| r.role == SessionRole::Swarm));
    // 接收方（本测试里的原始客户端）不产生会话上报。
    assert!(g.rig.receiver.sink.sessions.lock().unwrap().is_empty());
}

/// 授权失败的连接不开会话、不上报。
#[tokio::test]
async fn failed_grant_opens_no_swarm_session() {
    let rig = rig("swarm-nogrant", 5 * MIB).await;
    let mut provider = provide_peer(rig.signer.ring.clone(), &rig.file);
    let hint = loopback_hint(&provider.ep).await;
    let endpoint = rig.receiver.ep.ensure_bound().await.unwrap();
    let conn = endpoint.connect(hint, SWARM_ALPN).await.unwrap();
    let expired = swarm_ticket_for(
        &rig.signer,
        "swarm-nogrant",
        &rig.receiver.ep,
        &rig.file.manifest.files[0].root_hash,
        1,
    );
    assert_eq!(
        ask(&conn, &grant(expired)).await.0.status,
        SwarmStatus::Expired
    );
    conn.close(0u32.into(), b"done");
    conn.closed().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(provider.sink.sessions.lock().unwrap().is_empty());
    while let Ok(event) = provider.events.try_recv() {
        assert!(
            !matches!(event, SessionEvent::Opened { .. }),
            "unexpected session: {event:?}"
        );
    }
}

/// `kill_session` 对共享提供方会话同样生效：连接以 0x11 关闭，终态报告带 `session killed`，
/// 之后会话不再存在。
#[tokio::test]
async fn kill_session_closes_swarm_connection() {
    let mut g = granted_connection().await;
    let (session_id, _, _) = opened_swarm_session(&mut g.provider).await;
    fetch_chunk(&g.conn, &g.root, 1).await;

    let sender = g.provider.ep.sender();
    assert!(!sender.kill_session("no-such-session"));
    assert!(sender.kill_session(&session_id));
    let closed = tokio::time::timeout(Duration::from_secs(10), g.conn.closed())
        .await
        .expect("swarm connection was not closed");
    match closed {
        ConnectionError::ApplicationClosed(app) => assert_eq!(u64::from(app.error_code), 0x11),
        other => panic!("unexpected close reason: {other:?}"),
    }

    let report = wait_final_swarm_report(&g.provider).await;
    assert_eq!(report.session_id, session_id);
    assert_eq!(report.summary.error.as_deref(), Some("session killed"));
    // 被中断之前已成功发出的数据仍然记账。
    assert_eq!(
        report.swarm_files,
        vec![SwarmServedFile {
            root_hash: g.root.clone(),
            bytes: MIB as u64,
            chunks: 1,
        }]
    );
    assert!(!sender.kill_session(&session_id), "ended session is gone");
}

fn priority_ticket(rig: &Rig, root: &str, pri: u8) -> String {
    let mut p = payload("share-pri", &rig.receiver.ep.id_z32(), far_future());
    p.scope = "swarm".into();
    p.root = Some(root.to_owned());
    p.pri = pri;
    sign_ticket(&rig.signer.secret, &p).unwrap()
}

/// 保留槽位只接受 `pri = 1` 的票据；保留数为 0 时与原行为相同。
#[tokio::test]
async fn reserved_slots_refuse_non_priority_grants() {
    let rig = rig("swarm-reserved", 5 * MIB).await;
    let provider = provide_peer(rig.signer.ring.clone(), &rig.file);
    provider.ep.set_swarm_policy(8, 0, 0);
    let hint = loopback_hint(&provider.ep).await;
    let endpoint = rig.receiver.ep.ensure_bound().await.unwrap();
    let root = rig.file.manifest.files[0].root_hash.clone();

    let mut conns = Vec::new();
    for _ in 0..24 {
        let conn = endpoint.connect(hint.clone(), SWARM_ALPN).await.unwrap();
        let ticket = priority_ticket(&rig, &root, 0);
        assert_eq!(ask(&conn, &grant(ticket)).await.0.status, SwarmStatus::Ok);
        conns.push(conn);
    }
    // 第 25 条落在保留槽位：pri = 0 被拒。
    let conn = endpoint.connect(hint.clone(), SWARM_ALPN).await.unwrap();
    let ticket = priority_ticket(&rig, &root, 0);
    assert_eq!(ask(&conn, &grant(ticket)).await.0.status, SwarmStatus::Busy);
    // pri = 1 成功。
    let conn = endpoint.connect(hint.clone(), SWARM_ALPN).await.unwrap();
    let ticket = priority_ticket(&rig, &root, 1);
    assert_eq!(ask(&conn, &grant(ticket)).await.0.status, SwarmStatus::Ok);
    conns.push(conn);
    // 保留数为 0：pri = 0 也成功。
    provider.ep.set_swarm_policy(0, 0, 0);
    let conn = endpoint.connect(hint, SWARM_ALPN).await.unwrap();
    let ticket = priority_ticket(&rig, &root, 0);
    assert_eq!(ask(&conn, &grant(ticket)).await.0.status, SwarmStatus::Ok);
    conns.push(conn);
}

/// 每日上限：达到后 `Get` 回 Busy。
#[tokio::test]
async fn daily_cap_returns_busy_after_limit() {
    let g = granted_connection().await;
    g.provider.ep.set_swarm_policy(0, (8 * MIB) as u64, 0);
    assert_eq!(fetch_chunk(&g.conn, &g.root, 0).await, (4 * MIB) as u64);
    assert_eq!(fetch_chunk(&g.conn, &g.root, 0).await, (4 * MIB) as u64);
    let request = SwarmRequest::Get {
        root: g.root.clone(),
        chunk: 0,
    };
    assert_eq!(ask(&g.conn, &request).await.0.status, SwarmStatus::Busy);
    let (_, used) = g.provider.ep.swarm_daily_usage();
    assert_eq!(used, (8 * MIB) as u64);
    // 取消上限后恢复服务。
    g.provider.ep.set_swarm_policy(0, 0, 0);
    assert_eq!(fetch_chunk(&g.conn, &g.root, 0).await, (4 * MIB) as u64);
}

/// 接收方的会话报告带逐对端字节，合计等于来源计量。
#[tokio::test]
async fn receiver_reports_bytes_per_peer() {
    let rig = rig_with("swarm-perpeer", 20 * MIB, |p| {
        p.addr_hint = None;
        p.connect_timeout = Duration::from_millis(1500);
    })
    .await;
    let provider = provide_peer(rig.signer.ring.clone(), &rig.file);
    attach(&rig, &[(&provider, true)]).await;
    wait_peers(&rig.session, 1).await;
    let resp = http().get(rig.url()).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    resp.bytes().await.unwrap();
    rig.session.close().await;

    let reports = rig.receiver.sink.sessions.lock().unwrap().clone();
    let mut per_peer = 0;
    for report in reports.iter().filter(|r| r.is_final) {
        for entry in &report.peer_bytes {
            assert_eq!(entry.endpoint_id, provider.ep.id_z32());
            per_peer += entry.bytes;
        }
    }
    assert_eq!(per_peer, (20 * MIB) as u64);
}

/// 同一 session_id 下，每个对端的 `peerBytes` 在多次上报中单调不减（终态报告也是累计值）。
#[tokio::test]
async fn peer_bytes_are_cumulative_within_a_session_id() {
    let rig = rig("swarm-mono", 32 * MIB).await;
    let provide_a = provide_peer(rig.signer.ring.clone(), &rig.file);
    let provide_b = provide_peer(rig.signer.ring.clone(), &rig.file);
    attach(&rig, &[(&provide_a, true), (&provide_b, true)]).await;
    wait_peers(&rig.session, 2).await;
    let url = rig.url();
    for i in 0..4u64 {
        let range = format!(
            "bytes={}-{}",
            i * 8 * MIB as u64,
            (i + 1) * 8 * MIB as u64 - 1
        );
        let resp = get_range(&url, &range).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        resp.bytes().await.unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    rig.session.close().await;

    let reports = rig.receiver.sink.sessions.lock().unwrap().clone();
    let mut last: std::collections::HashMap<(String, String), u64> = Default::default();
    let mut seen = 0;
    for report in reports.iter().filter(|r| r.role == SessionRole::Receiver) {
        for entry in &report.peer_bytes {
            let key = (report.session_id.clone(), entry.endpoint_id.clone());
            let prev = last.insert(key, entry.bytes).unwrap_or(0);
            assert!(
                entry.bytes >= prev,
                "peerBytes went backwards within a session id"
            );
            seen += 1;
        }
    }
    assert!(seen > 0, "expected at least one peerBytes entry");
}
