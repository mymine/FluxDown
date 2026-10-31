//! 共享端到端测试：mock FluxCloud + 真实 iroh 回环端点 + 真实回环适配器。
//!
//! 分享方 A 创建（默认加入共享）→ 接收方 B 领取并完成（转为共享、向云端登记）→ A 离线 →
//! 接收方 C 只能从 B 取块。

use std::path::PathBuf;
use std::sync::Arc;

use fluxdown_protocol::{
    AgentEvent, DaemonEvent, ErrorReason, ShareConfigDto, ShareDiagnosticsDto, ShareDto,
    ShareProvideState, ShareReceiveRecordDto, ShareReceiveRecordsPage, ShareReceiveResultDto,
    ShareResolveDto, ShareSessionRole, ShareSwarmBlock, method,
};
use fluxdown_share::{EndpointAddr, EndpointId};
use serde_json::{Value, json};

use super::tests::{
    MockState, Node, call, node, node_at, point_receiver_at, receive_url, sender_with_file,
    start_mock, task, wait_until,
};

const MIB: usize = 1024 * 1024;

/// 接收方通过各提供者当前绑定的回环端口连接它们；不在列表里的 endpoint 没有任何地址（视为离线）。
fn point_at(receiver: &Node, peers: &[&Node]) {
    let peers: Vec<(String, Arc<super::ShareService>)> = peers
        .iter()
        .map(|p| (p.service.secret.public().to_z32(), p.service.clone()))
        .collect();
    *receiver.service.addr_hint.lock().unwrap() = Some(Arc::new(move |z32: &str| {
        let (_, service) = peers.iter().find(|(id, _)| id == z32)?;
        let endpoint = service.with(|i| i.endpoint.clone())?;
        let port = endpoint
            .bound_endpoint()?
            .bound_sockets()
            .iter()
            .find(|a| a.is_ipv4())?
            .port();
        let id = EndpointId::from_z32(z32).ok()?;
        Some(EndpointAddr::new(id).with_ip_addr(([127, 0, 0, 1], port).into()))
    }));
}

async fn fetch(url: &str) -> Vec<u8> {
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http.get(url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    response.bytes().await.unwrap().to_vec()
}

fn unbound(node: &Node) -> bool {
    node.service
        .with(|i| i.endpoint.as_ref().is_none_or(|e| !e.is_bound()))
}

fn provide_state(node: &Node, record_id: &str) -> ShareProvideState {
    node.service.with(|i| {
        i.records
            .iter()
            .find(|r| r.id == record_id)
            .map_or(ShareProvideState::Off, |r| i.record_dto(r).provide_state)
    })
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fluxdown_swarm_{label}_{}_{}",
        std::process::id(),
        fluxdown_share::timefmt::now_unix_secs()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Pair {
    base: String,
    mock: MockState,
    a: Node,
    b: Node,
    /// B 的数据目录（重启 B 时复用）。
    b_data: PathBuf,
    dir_b: PathBuf,
    share: ShareDto,
    payload: Vec<u8>,
    root: String,
    record_id: String,
}

/// A 分享（默认共享）；B 领取并取完全部字节、落盘，任务完成事件发布后 B 开始共享。
async fn receive_and_complete(label: &str, len: usize, auto_provide: bool) -> Pair {
    let (base, mock) = start_mock().await;
    let (a, _dir, payload) = sender_with_file(&base, &format!("swarm_{label}_a"), len).await;
    let share: ShareDto = call(&a, method::AGENT_SHARE_CREATE, json!({ "taskIds": ["t1"] }))
        .await
        .unwrap();
    let root = mock.shares.lock().unwrap()[0].create_body["files"][0]["rootHash"]
        .as_str()
        .unwrap()
        .to_owned();
    let b_data = temp_dir(&format!("{label}_b_data"));
    let b = node_at(
        &base,
        b_data.clone(),
        &format!("swarm_{label}_b"),
        false,
        Vec::new(),
    )
    .await;
    if !auto_provide {
        let config: ShareConfigDto = call(&b, method::AGENT_SHARE_CONFIG, json!({}))
            .await
            .unwrap();
        let mut local = serde_json::to_value(config.local_config).unwrap();
        local["autoProvide"] = json!(false);
        let _: Value = call(&b, method::AGENT_SHARE_SET_LOCAL_CONFIG, local)
            .await
            .unwrap();
    }
    point_receiver_at(&b, &a);
    let dir_b = temp_dir(&format!("{label}_b"));
    let url = receive_url(&b, &share.pickup_code, &dir_b).await;
    assert_eq!(fetch(&url).await, payload);
    std::fs::write(dir_b.join("payload.bin"), &payload).unwrap();
    b.events
        .publish(AgentEvent::Daemon(DaemonEvent::TaskChanged(task(
            "recv-task-1",
            3,
            &dir_b,
            "payload.bin",
        ))));
    let record_id = b.service.with(|i| i.records[0].id.clone());
    Pair {
        base,
        mock,
        a,
        b,
        b_data,
        dir_b,
        share,
        payload,
        root,
        record_id,
    }
}

fn b_registered(pair: &Pair) -> bool {
    let b_id = pair.b.service.secret.public().to_z32();
    pair.mock
        .swarm_rows
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.0 == b_id && r.1 == pair.root && r.2 == pair.share.id && r.3)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swarm_receiver_completes_from_provide_when_sender_is_offline() {
    let pair = receive_and_complete("provide", 12 * MIB, true).await;

    // B 的记录进入「共享中」，云端收到它的完整登记。
    assert_eq!(
        provide_state(&pair.b, &pair.record_id),
        ShareProvideState::Providing
    );
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;
    let page: ShareReceiveRecordsPage =
        call(&pair.b, method::AGENT_SHARE_RECEIVE_RECORDS, json!({}))
            .await
            .unwrap();
    assert_eq!(page.records[0].provide_state, ShareProvideState::Providing);
    assert!(!pair.b.service.content.is_empty());

    // 原分享方下线：它的端点释放，C 只能找 B。
    pair.a.cancel.cancel();
    wait_until("A endpoint released", || {
        pair.a.service.with(|i| i.endpoint.is_none())
    })
    .await;

    let c = node(&pair.base, "swarm_provide_c", false, Vec::new()).await;
    point_at(&c, &[&pair.b]);
    let preview: ShareResolveDto = call(
        &c,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": pair.share.pickup_code }),
    )
    .await
    .unwrap();
    assert_eq!(preview.sources.swarm_peers, 1);
    let dir_c = temp_dir("provide_c");
    let _: ShareReceiveResultDto = call(
        &c,
        method::AGENT_SHARE_RECEIVE,
        json!({ "resolveId": preview.resolve_id, "saveDir": dir_c.to_string_lossy() }),
    )
    .await
    .unwrap();
    let url = c.tasks.requests.lock().unwrap()[0].url.clone();
    assert_eq!(fetch(&url).await, pair.payload);

    let session = c
        .service
        .with(|i| i.receive_sessions.values().next().cloned())
        .unwrap();
    assert_eq!(session.source_bytes().peer, (12 * MIB) as u64);
    let facts = c.service.with(|i| i.state_dto(false));
    assert!(facts.task_facts["recv-task-1"].swarm_peers <= 1);

    c.cancel.cancel();
    pair.b.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_providing_withdraws_and_releases_the_endpoint() {
    let pair = receive_and_complete("stop", 5 * MIB, true).await;
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;
    wait_until("B content offered", || !pair.b.service.content.is_empty()).await;
    // 接收完成后任务已移除，但共享中的记录仍保留其任务事实。
    let facts = pair.b.service.with(|i| i.state_dto(false).task_facts);
    let kept: Vec<_> = facts
        .values()
        .filter(|f| f.record_id.as_deref() == Some(pair.record_id.as_str()))
        .collect();
    assert!(!kept.is_empty(), "providing record keeps task facts");
    assert!(
        kept.iter()
            .all(|f| f.is_share_receive && f.provide_state == ShareProvideState::Providing)
    );

    let record: ShareReceiveRecordDto = call(
        &pair.b,
        method::AGENT_SHARE_STOP_PROVIDING,
        json!({ "recordId": pair.record_id }),
    )
    .await
    .unwrap();
    assert_eq!(record.provide_state, ShareProvideState::Stopped);
    // 任务事实随共享一起消失。
    let facts = pair.b.service.with(|i| i.state_dto(false).task_facts);
    assert!(
        facts
            .values()
            .all(|f| f.record_id.as_deref() != Some(pair.record_id.as_str())),
        "stopped provide must not keep task facts: {facts:?}"
    );
    assert!(pair.b.service.content.is_empty());

    // 云端收到撤销，登记行消失；B 没有任何理由保持端点。
    wait_until("withdraw announced", || {
        pair.mock.announces.lock().unwrap().iter().any(|body| {
            body["withdraw"]
                .as_array()
                .is_some_and(|list| list.iter().any(|r| r.as_str() == Some(pair.root.as_str())))
        })
    })
    .await;
    wait_until("registration row removed", || {
        pair.mock.swarm_rows.lock().unwrap().is_empty()
    })
    .await;
    wait_until("B endpoint released", || unbound(&pair.b)).await;

    // 幂等：再停一次原样返回。
    let again: ShareReceiveRecordDto = call(
        &pair.b,
        method::AGENT_SHARE_STOP_PROVIDING,
        json!({ "recordId": pair.record_id }),
    )
    .await
    .unwrap();
    assert_eq!(again.provide_state, ShareProvideState::Stopped);
    let missing = call::<ShareReceiveRecordDto>(
        &pair.b,
        method::AGENT_SHARE_STOP_PROVIDING,
        json!({ "recordId": "nope" }),
    )
    .await;
    assert_eq!(
        missing.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareNotFound)
    );

    pair.a.cancel.cancel();
    pair.b.cancel.cancel();
}

/// 登记 / 撤销条目超过云端单次上限（1000）时分批发送，一条都不丢。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn announce_splits_large_provides_into_batches() {
    const EXTRA: usize = 2500;
    let pair = receive_and_complete("batch", MIB, true).await;
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;

    let mut roots: Vec<String> = (0..EXTRA).map(|n| format!("{n:064x}")).collect();
    pair.b.service.with(|i| {
        let provide = i
            .provides
            .iter_mut()
            .find(|s| s.record_id == pair.record_id)
            .unwrap();
        for (n, root) in roots.iter().enumerate() {
            provide.files.push(super::store::ProvideFile {
                task_id: format!("extra-{n}"),
                root_hash: root.clone(),
                size: 1,
                mtime_ms: 0,
                path: pair.dir_b.join(format!("extra-{n}")),
            });
        }
    });
    roots.push(pair.root.clone());
    pair.b.service.announce().await;

    let b_id = pair.b.service.secret.public().to_z32();
    let registered: std::collections::HashSet<String> = pair
        .mock
        .swarm_rows
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.0 == b_id)
        .map(|r| r.1.clone())
        .collect();
    assert!(roots.iter().all(|r| registered.contains(r)));

    let _: ShareReceiveRecordDto = call(
        &pair.b,
        method::AGENT_SHARE_STOP_PROVIDING,
        json!({ "recordId": pair.record_id }),
    )
    .await
    .unwrap();
    wait_until("every registration withdrawn", || {
        pair.mock.swarm_rows.lock().unwrap().is_empty()
    })
    .await;
    for body in pair.mock.announces.lock().unwrap().iter() {
        let len = |key: &str| body[key].as_array().map_or(0, Vec::len);
        assert!(len("entries") <= 1000 && len("withdraw") <= 1000);
    }

    pair.a.cancel.cancel();
    pair.b.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_provide_off_never_announces() {
    let pair = receive_and_complete("noprovide", 5 * MIB, false).await;
    wait_until("record completed", || {
        pair.b
            .service
            .with(|i| i.records[0].status == fluxdown_protocol::ShareReceiveStatus::Completed)
    })
    .await;
    // 留出登记计时器本该触发的时间。
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    assert_eq!(
        provide_state(&pair.b, &pair.record_id),
        ShareProvideState::Off
    );
    assert!(pair.mock.swarm_rows.lock().unwrap().is_empty());
    assert!(pair.mock.announces.lock().unwrap().is_empty());
    assert!(pair.b.service.content.is_empty());

    pair.a.cancel.cancel();
    pair.b.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn share_swarm_flag_follows_default_and_update() {
    let (base, mock) = start_mock().await;
    let (a, _dir, _payload) = sender_with_file(&base, "swarm_flag_a", MIB).await;

    // 缺省：加入共享。
    let first: ShareDto = call(&a, method::AGENT_SHARE_CREATE, json!({ "taskIds": ["t1"] }))
        .await
        .unwrap();
    assert!(first.swarm);
    assert_eq!(
        mock.shares.lock().unwrap()[0].create_body["swarm"],
        json!(true)
    );

    // 「新建分享默认」关闭后新建的分享不加入；显式参数优先于默认。
    let config: ShareConfigDto = call(&a, method::AGENT_SHARE_CONFIG, json!({}))
        .await
        .unwrap();
    let mut local = serde_json::to_value(config.local_config).unwrap();
    local["defaultSwarm"] = json!(false);
    let _: Value = call(&a, method::AGENT_SHARE_SET_LOCAL_CONFIG, local)
        .await
        .unwrap();
    let second: ShareDto = call(&a, method::AGENT_SHARE_CREATE, json!({ "taskIds": ["t1"] }))
        .await
        .unwrap();
    assert!(!second.swarm);
    assert_eq!(
        mock.shares.lock().unwrap()[1].create_body["swarm"],
        json!(false)
    );
    let third: ShareDto = call(
        &a,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"], "swarm": true }),
    )
    .await
    .unwrap();
    assert!(third.swarm);

    // 单个分享改开关：PATCH 体带 swarm，返回的 DTO 随云端更新。
    let updated: ShareDto = call(
        &a,
        method::AGENT_SHARE_UPDATE,
        json!({ "id": second.id, "swarm": true }),
    )
    .await
    .unwrap();
    assert!(updated.swarm);
    assert_eq!(
        mock.shares.lock().unwrap()[1].create_body["swarm"],
        json!(true)
    );
    let off: ShareDto = call(
        &a,
        method::AGENT_SHARE_UPDATE,
        json!({ "id": first.id, "swarm": false }),
    )
    .await
    .unwrap();
    assert!(!off.swarm);

    a.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn announce_ended_marks_the_provide_ended() {
    let pair = receive_and_complete("ended", 5 * MIB, true).await;
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;
    wait_until("B content offered", || !pair.b.service.content.is_empty()).await;

    // 原分享停止后，云端对 B 的登记回 `ended`：共享结束，内容不再提供。
    let _: ShareDto = call(
        &pair.a,
        method::AGENT_SHARE_STOP,
        json!({ "id": pair.share.id }),
    )
    .await
    .unwrap();
    pair.b.service.announce().await;

    assert_eq!(
        provide_state(&pair.b, &pair.record_id),
        ShareProvideState::Ended
    );
    assert!(pair.b.service.content.is_empty());

    pair.a.cancel.cancel();
    pair.b.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_the_provideed_task_withdraws_the_file() {
    let pair = receive_and_complete("delete", 5 * MIB, true).await;
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;

    pair.b
        .events
        .publish(AgentEvent::Daemon(DaemonEvent::TaskDeleted {
            task_id: "recv-task-1".to_owned(),
        }));
    wait_until("registration row removed", || {
        pair.mock.swarm_rows.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        provide_state(&pair.b, &pair.record_id),
        ShareProvideState::Ended
    );
    assert!(pair.b.service.content.is_empty());

    pair.a.cancel.cancel();
    pair.b.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_auto_provide_off_stops_current_providing() {
    let pair = receive_and_complete("toggle", 5 * MIB, true).await;
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;

    let config: ShareConfigDto = call(&pair.b, method::AGENT_SHARE_CONFIG, json!({}))
        .await
        .unwrap();
    let mut local = serde_json::to_value(config.local_config).unwrap();
    local["autoProvide"] = json!(false);
    let _: Value = call(&pair.b, method::AGENT_SHARE_SET_LOCAL_CONFIG, local)
        .await
        .unwrap();

    assert_eq!(
        provide_state(&pair.b, &pair.record_id),
        ShareProvideState::Stopped
    );
    assert!(pair.b.service.content.is_empty());
    wait_until("registration row removed", || {
        pair.mock.swarm_rows.lock().unwrap().is_empty()
    })
    .await;

    pair.a.cancel.cancel();
    pair.b.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_resumes_providing_only_for_intact_files() {
    let pair = receive_and_complete("restart", 5 * MIB, true).await;
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;
    pair.b.cancel.cancel();
    wait_until("B endpoint released", || {
        pair.b.service.with(|i| i.endpoint.is_none())
    })
    .await;
    drop(pair.b);

    // 文件没动：重启后继续共享并重新提供内容。
    let b2 = node_at(
        &pair.base,
        pair.b_data.clone(),
        "swarm_restart_b2",
        false,
        Vec::new(),
    )
    .await;
    wait_until("providing resumed", || {
        provide_state(&b2, &pair.record_id) == ShareProvideState::Providing
            && !b2.service.content.is_empty()
    })
    .await;
    b2.cancel.cancel();
    wait_until("B2 endpoint released", || {
        b2.service.with(|i| i.endpoint.is_none())
    })
    .await;
    drop(b2);

    // 文件被改：重启后不再提供，共享结束并撤销登记。
    std::fs::write(pair.dir_b.join("payload.bin"), b"changed").unwrap();
    let b3 = node_at(
        &pair.base,
        pair.b_data.clone(),
        "swarm_restart_b3",
        false,
        Vec::new(),
    )
    .await;
    wait_until("provide ended", || {
        provide_state(&b3, &pair.record_id) == ShareProvideState::Ended
    })
    .await;
    assert!(b3.service.content.is_empty());
    wait_until("registration row removed", || {
        pair.mock.swarm_rows.lock().unwrap().is_empty()
    })
    .await;

    pair.a.cancel.cancel();
    b3.cancel.cancel();
}

/// B 作为共享提供方向 C 发数据：B 的诊断里出现 role=swarm 的会话（按内容累计的服务量 + 文件名），
/// 同一份数据上报云端；C 断开后会话进入终态。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swarm_provider_session_is_shown_in_diagnostics_and_reported() {
    let pair = receive_and_complete("diag", 12 * MIB, true).await;
    wait_until("B announced as a complete holder", || b_registered(&pair)).await;
    wait_until("B content offered", || !pair.b.service.content.is_empty()).await;
    pair.a.cancel.cancel();
    wait_until("A endpoint released", || {
        pair.a.service.with(|i| i.endpoint.is_none())
    })
    .await;

    let c = node(&pair.base, "swarm_diag_c", false, Vec::new()).await;
    point_at(&c, &[&pair.b]);
    let preview: ShareResolveDto = call(
        &c,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": pair.share.pickup_code }),
    )
    .await
    .unwrap();
    let dir_c = temp_dir("diag_c");
    let _: ShareReceiveResultDto = call(
        &c,
        method::AGENT_SHARE_RECEIVE,
        json!({ "resolveId": preview.resolve_id, "saveDir": dir_c.to_string_lossy() }),
    )
    .await
    .unwrap();
    let url = c.tasks.requests.lock().unwrap()[0].url.clone();
    assert_eq!(fetch(&url).await, pair.payload);

    // C 走掉：B 一侧的共享连接结束，终态报告记入诊断簿。
    c.cancel.cancel();
    wait_until("B's swarm session ended", || {
        pair.b
            .service
            .telemetry
            .diag
            .lock()
            .unwrap()
            .details(None)
            .iter()
            .any(|d| d.role == ShareSessionRole::Swarm && d.ended)
    })
    .await;

    let diag: ShareDiagnosticsDto = call(&pair.b, method::AGENT_SHARE_DIAGNOSTICS, json!({}))
        .await
        .unwrap();
    let session = diag
        .sessions
        .iter()
        .find(|s| s.role == ShareSessionRole::Swarm)
        .unwrap();
    assert_eq!(session.share_id, pair.share.id);
    assert_eq!(session.peer_endpoint_id, c.service.secret.public().to_z32());
    assert_eq!(session.swarm_files.len(), 1);
    let served = &session.swarm_files[0];
    assert_eq!(served.root_hash, pair.root);
    let name = pair.mock.shares.lock().unwrap()[0].create_body["files"][0]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(served.file_name.as_deref(), Some(name.as_str()));
    assert!(served.bytes >= (12 * MIB) as u64 && served.chunks >= 3);
    // 其他角色的会话没有服务量。
    assert!(
        diag.sessions
            .iter()
            .filter(|s| s.role != ShareSessionRole::Swarm)
            .all(|s| s.swarm_files.is_empty())
    );

    // 展示即上报：云端收到同一个会话的 swarm 终态报告，内容与诊断页一致。
    wait_until("swarm report posted", || {
        pair.mock.sessions.lock().unwrap().iter().any(|body| {
            body["role"] == "swarm"
                && body["final"] == true
                && body["sessionId"] == session.session_id.as_str()
        })
    })
    .await;
    let posted = pair
        .mock
        .sessions
        .lock()
        .unwrap()
        .iter()
        .find(|body| body["role"] == "swarm" && body["final"] == true)
        .cloned()
        .unwrap();
    assert_eq!(posted["shareId"], pair.share.id.as_str());
    assert_eq!(
        posted["swarmFiles"],
        json!([{ "rootHash": pair.root, "bytes": served.bytes, "chunks": served.chunks }])
    );

    pair.b.cancel.cancel();
}

/// 管理员关闭并锁定共享：对账后本机分享 `swarm = false`、`swarmLocked = true`；
/// 再打开被云端拒绝并给出专门的原因，关闭仍允许。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_swarm_lock_is_applied_and_blocks_reenabling() {
    let (base, mock) = start_mock().await;
    let (a, _dir, _payload) = sender_with_file(&base, "swarm_lock_a", MIB).await;
    let share: ShareDto = call(&a, method::AGENT_SHARE_CREATE, json!({ "taskIds": ["t1"] }))
        .await
        .unwrap();
    assert!(share.swarm && !share.swarm_locked);

    {
        let mut shares = mock.shares.lock().unwrap();
        shares[0].create_body["swarm"] = json!(false);
        shares[0].create_body["swarmLocked"] = json!(true);
    }
    // 云端 `share.changed` 触发的就是这条对账路径。
    a.service.sync_shares().await;
    let local = a.service.with(|i| i.shares[0].dto.clone());
    assert!(!local.swarm && local.swarm_locked);

    let denied = call::<ShareDto>(
        &a,
        method::AGENT_SHARE_UPDATE,
        json!({ "id": share.id, "swarm": true }),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.reason, Some(ErrorReason::ShareSwarmLocked));
    let still = a.service.with(|i| i.shares[0].dto.clone());
    assert!(!still.swarm && still.swarm_locked);

    let off: ShareDto = call(
        &a,
        method::AGENT_SHARE_UPDATE,
        json!({ "id": share.id, "swarm": false }),
    )
    .await
    .unwrap();
    assert!(!off.swarm && off.swarm_locked);

    a.cancel.cancel();
}

/// 云端拒绝共享加速（额度用完）：任务行带 `swarmBlock`，但仍从分享方直连完成。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quota_exhausted_blocks_swarm_but_origin_completes() {
    let (base, mock) = start_mock().await;
    let (a, _dir, payload) = sender_with_file(&base, "swarm_quota_a", 5 * MIB).await;
    let share: ShareDto = call(&a, method::AGENT_SHARE_CREATE, json!({ "taskIds": ["t1"] }))
        .await
        .unwrap();
    *mock.peers_error.lock().unwrap() =
        Some((axum::http::StatusCode::FORBIDDEN, "swarm_quota_exhausted"));
    let b = node(&base, "swarm_quota_b", false, Vec::new()).await;
    point_receiver_at(&b, &a);
    let dir_b = temp_dir("quota_b");
    let url = receive_url(&b, &share.pickup_code, &dir_b).await;
    assert_eq!(fetch(&url).await, payload);

    let state = b.service.with(|i| i.state_dto(false));
    assert_eq!(
        state.task_facts["recv-task-1"].swarm_block,
        Some(ShareSwarmBlock::QuotaExhausted)
    );
    b.cancel.cancel();
    a.cancel.cancel();
}
