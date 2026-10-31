//! 进程内端到端：两个端点（无中继）、回环 HTTP 适配器、真实 HTTP 客户端分两段取回并续传。

use std::time::{Duration, Instant};

use fluxdown_share::{PathKind, SessionEvent, SessionRole};
use reqwest::StatusCode;

use crate::common::{Rig, http, rig};

const MIB: usize = 1024 * 1024;

/// 把响应体读到 `limit` 字节后丢弃连接（模拟下载器被打断）。
async fn read_partial(url: &str, range: &str, limit: usize) -> Vec<u8> {
    let mut resp = http().get(url).header("Range", range).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let mut got = Vec::new();
    while got.len() < limit {
        match resp.chunk().await.unwrap() {
            Some(c) => got.extend_from_slice(&c),
            None => break,
        }
    }
    got
}

#[tokio::test]
async fn twenty_mib_two_ranges_with_interruption_and_resume() {
    let mut rig = rig("share-e2e", 20 * MIB).await;
    let url = rig.url();
    let data = rig.file.data.clone();
    let total = data.len();

    // HEAD：长度、Accept-Ranges、ETag（根哈希）。
    let head = http().head(&url).send().await.unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["accept-ranges"], "bytes");
    assert_eq!(head.headers()["content-length"], total.to_string());
    assert_eq!(
        head.headers()["etag"],
        format!("\"{}\"", rig.file.manifest.files[0].root_hash)
    );

    // 第一段：前 9_000_000 字节（跨块边界），完整读完。
    let first_end = 9_000_000usize;
    let resp = http()
        .get(&url)
        .header("Range", format!("bytes=0-{}", first_end - 1))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers()["content-range"],
        format!("bytes 0-{}/{total}", first_end - 1)
    );
    assert_eq!(resp.headers()["content-length"], first_end.to_string());
    let part1 = resp.bytes().await.unwrap();
    assert_eq!(&part1[..], &data[..first_end]);

    // 第二段：从 first_end 起读一部分后“断线”，并在分享方强制踢掉会话，逼接收方重连。
    let got = read_partial(&url, &format!("bytes={first_end}-"), 3 * MIB).await;
    assert!(got.len() >= 3 * MIB);
    assert_eq!(&got[..], &data[first_end..first_end + got.len()]);
    let sessions = rig.sender.ep.sender().sessions();
    assert_eq!(sessions.len(), 1, "sender sees one live session");
    assert!(rig.sender.ep.sender().kill_session(&sessions[0].session_id));

    // 续传：从已收到的位置继续，经过重连后取完剩余。
    let resume_from = first_end + got.len();
    let started = Instant::now();
    let resp = http()
        .get(&url)
        .header("Range", format!("bytes={resume_from}-"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let tail = resp.bytes().await.unwrap();
    let tail_secs = started.elapsed().as_secs_f64();
    assert_eq!(&tail[..], &data[resume_from..]);

    // 拼接结果与原文件逐字节一致，BLAKE3 一致。
    let mut assembled = Vec::with_capacity(total);
    assembled.extend_from_slice(&part1);
    assembled.extend_from_slice(&got);
    assembled.extend_from_slice(&tail);
    assert_eq!(assembled.len(), total);
    assert_eq!(blake3::hash(&assembled), blake3::hash(&data));

    // 回环直连：来源徽标是“直连·局域网”。
    assert_eq!(rig.session.path_kind(), PathKind::DirectLan);

    eprintln!(
        "resume after interruption + reconnect: {:.1} MiB in {:.0} ms",
        tail.len() as f64 / MIB as f64,
        tail_secs * 1000.0
    );

    // 会话事件：发送方至少经历 打开 → 路径选定 → 关闭（被踢掉的那一个，带错误文本）。
    rig.session.close().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut events = Vec::new();
    while let Ok(ev) = rig.sender.events.try_recv() {
        events.push(ev);
    }
    assert!(events.iter().any(|e| matches!(
        e,
        SessionEvent::Opened {
            role: SessionRole::Sender,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        SessionEvent::PathChanged {
            kind: PathKind::DirectLan,
            ..
        }
    )));
    let killed = events.iter().find_map(|e| match e {
        SessionEvent::Closed {
            error: Some(err), ..
        } => Some(err.clone()),
        _ => None,
    });
    assert_eq!(killed.as_deref(), Some("session killed"));

    // 上报：双方都产生终态报告，字节落在直连一侧，BLAKE3 校验失败为 0。
    let sender_reports = rig.sender.sink.sessions.lock().unwrap().clone();
    let receiver_reports = rig.receiver.sink.sessions.lock().unwrap().clone();
    let sender_final = sender_reports
        .iter()
        .filter(|r| r.is_final)
        .collect::<Vec<_>>();
    let receiver_final = receiver_reports
        .iter()
        .filter(|r| r.is_final)
        .collect::<Vec<_>>();
    assert!(!sender_final.is_empty() && !receiver_final.is_empty());
    let sent: u64 = sender_final.iter().map(|r| r.summary.bytes.direct).sum();
    let recv: u64 = receiver_final.iter().map(|r| r.summary.bytes.direct).sum();
    assert!(sent >= total as u64, "sender direct bytes {sent}");
    assert!(recv >= total as u64, "receiver direct bytes {recv}");
    assert!(
        receiver_final
            .iter()
            .all(|r| r.summary.bytes.relay == 0 && r.summary.verify_failures == 0)
    );
    assert!(
        receiver_final
            .iter()
            .all(|r| r.role == SessionRole::Receiver)
    );
    assert!(
        receiver_final
            .iter()
            .all(|r| r.summary.path_kind == PathKind::DirectLan)
    );
    assert!(
        receiver_final
            .iter()
            .all(|r| r.summary.first_direct_ms.is_some())
    );
    assert!(
        receiver_final
            .iter()
            .any(|r| !r.summary.paths.is_empty() && !r.summary.events.is_empty())
    );
    // 被踢的发送方会话终态带错误。
    assert!(
        sender_final
            .iter()
            .any(|r| r.summary.error.as_deref() == Some("session killed"))
    );
    // 报告 JSON 形状：camelCase 且 final 字段名。
    let json = serde_json::to_value(receiver_final[0]).unwrap();
    assert!(json.get("final").is_some() && json.get("sessionId").is_some());
    assert!(
        json["summary"].get("startedAt").is_some() && json["summary"].get("connectMs").is_some()
    );
}

#[tokio::test]
async fn rate_limited_transfer_emits_progress_samples_only_while_active() {
    let mut rig = rig("share-rate", 6 * MIB).await;
    // 分享限速 3 MiB/s：取 6 MiB 约 1.5–2 s，上报间隔 1 s → 至少一个进行中的采样。
    rig.sender
        .registry
        .set_rate_limit("share-rate", Some(3 * MIB as u64));
    let url = rig.url();

    // 传输开始前：没有任何会话报告（空闲静默）。
    assert!(rig.sender.sink.sessions.lock().unwrap().is_empty());

    let started = Instant::now();
    let body = http()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(blake3::hash(&body), blake3::hash(&rig.file.data));
    assert!(
        secs > 1.0,
        "rate limit must slow the transfer down, took {secs:.2}s"
    );
    assert!(secs < 6.0, "but not stall it, took {secs:.2}s");

    let live: Vec<_> = rig
        .sender
        .sink
        .sessions
        .lock()
        .unwrap()
        .iter()
        .filter(|r| !r.is_final)
        .cloned()
        .collect();
    assert!(!live.is_empty(), "expected in-flight session reports");
    assert!(
        live.iter()
            .any(|r| r.samples.iter().any(|s| s.bytes > 0 && s.rate_bps > 0.0))
    );

    // 传输结束后：最多再补一个周期的尾部采样，随后定时器停表，安静期内不再有新的进行中报告。
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let before = rig.sender.sink.sessions.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let after = rig.sender.sink.sessions.lock().unwrap().len();
    assert_eq!(before, after, "no reports while idle");

    let mut progress = 0;
    while let Ok(ev) = rig.receiver.events.try_recv() {
        if matches!(ev, SessionEvent::Progress { .. }) {
            progress += 1;
        }
    }
    assert!(progress >= 1, "receiver emitted progress events");
}

/// 回归：守卫计数在块与块之间归零（每块一次取数、中间有空档）时，采样定时器不能被重置，
/// 两端仍应按上报间隔产出进行中的会话报告。
#[tokio::test]
async fn live_reports_survive_gaps_between_chunk_fetches() {
    const CHUNK: usize = 4 * MIB;
    let rig = rig("share-gaps", 40 * MIB).await;
    let url = rig.url();
    // 每 350 ms 取一个新块的头 1 KiB：每次取数只持续几十毫秒，其余时间守卫计数为 0；上报间隔 1 s。
    for i in 0..10usize {
        let start = i * CHUNK;
        let resp = http()
            .get(&url)
            .header("Range", format!("bytes={start}-{}", start + 1023))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            &resp.bytes().await.unwrap()[..],
            &rig.file.data[start..start + 1024]
        );
        tokio::time::sleep(Duration::from_millis(350)).await;
    }
    for (who, sink) in [
        ("sender", &rig.sender.sink),
        ("receiver", &rig.receiver.sink),
    ] {
        let live = sink
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|r| !r.is_final)
            .count();
        assert!(
            live >= 2,
            "{who}: expected in-flight reports across gaps, got {live}"
        );
    }
}

/// 新会话、空缓存下取整个 20 MiB：只用来观察回环吞吐（含 BLAKE3 校验与 HTTP 往返），不设吞吐断言。
#[tokio::test]
async fn loopback_throughput_uncached() {
    let rig = rig("share-speed", 20 * MIB).await;
    let started = Instant::now();
    let resp = http().get(rig.url()).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.bytes().await.unwrap();
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(blake3::hash(&body), blake3::hash(&rig.file.data));
    eprintln!(
        "loopback throughput (uncached, includes connect + handshake): 20 MiB in {:.0} ms = {:.1} MiB/s",
        secs * 1000.0,
        20.0 / secs
    );
}

/// 端点关闭 / 会话句柄被丢弃时，双方会话都会收尾并各发出一条终态报告。
#[tokio::test]
async fn sessions_are_finalized_on_shutdown_and_drop() {
    let Rig {
        sender,
        receiver,
        session,
        adapter,
        token,
        file,
        signer: _signer,
    } = rig("share-final", 1024 * 1024).await;
    let url = adapter.url(&token, 0);
    assert_eq!(
        http().get(&url).send().await.unwrap().status(),
        StatusCode::OK
    );
    assert!(
        sender
            .sink
            .sessions
            .lock()
            .unwrap()
            .iter()
            .all(|r| !r.is_final)
    );

    // 分享方端点关闭：发送方会话收尾。
    sender.ep.shutdown().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let finals = |p: &crate::common::Peer| {
        p.sink
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.is_final)
            .count()
    };
    assert_eq!(
        finals(&sender),
        1,
        "sender session finalized on endpoint shutdown"
    );
    assert!(sender.ep.sender().sessions().is_empty());

    // 接收方没有调用 close()，直接丢弃所有句柄（适配器持有一份，先注销再丢弃）。
    assert_eq!(finals(&receiver), 0);
    adapter.unregister(&token);
    drop(session);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(finals(&receiver), 1, "receiver session finalized on drop");
    drop(file);
}
