//! 会话驱动的端到端行为（脚本化连接 + 暂停的 tokio 时间）：握手校验、快照、缓冲重放、
//! 游标重同步、离线宽限与重连、致命错误。

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::driver::Timing;
use crate::dto::{HostEventDto, HostSignalDto};
use crate::error::{ErrorCodeDto, FluxError, HostErrorDto};
use crate::link::{CloseInfo, ConnectFailure, LinkEvent};
use crate::projection::fixtures::daemon_frame;
use crate::session::HostSession;
use crate::testkit::{LinkHandle, ScriptedConnector, agent_hello, deleted_frame, mock_link, open};

async fn next(session: &HostSession) -> Option<HostSignalDto> {
    timeout(Duration::from_secs(60), session.next_signal())
        .await
        .expect("a signal within the test window")
}

fn snapshot_task_ids(signal: Option<HostSignalDto>) -> Vec<String> {
    match signal {
        Some(HostSignalDto::Snapshot { snapshot }) => snapshot
            .tasks
            .into_iter()
            .map(|task| task.task_id)
            .collect(),
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

fn progress(task_id: &str, speed: i64) -> fluxdown_protocol::DaemonEvent {
    fluxdown_protocol::DaemonEvent::Engine(fluxdown_protocol::WsServerMsg::TaskProgress {
        task_id: task_id.to_owned(),
        status: 1,
        downloaded_bytes: 10,
        total_bytes: 100,
        speed,
        upload_speed: 0,
        file_name: String::new(),
        save_dir: String::new(),
        url: String::new(),
        error_message: String::new(),
        uploaded_bytes: 0,
        seeding_status: 0,
        seeding_message: String::new(),
        seeding_time_secs: 0,
    })
}

/// 单连接会话：首个快照序号 5，含任务 `a`。
async fn single(connector: Arc<ScriptedConnector>) -> (Arc<HostSession>, LinkHandle) {
    let (link, handle) = mock_link(agent_hello());
    handle.send_snapshot(5, &["a"]);
    connector.push(Ok(link));
    let (session, _driver) = open(connector, Timing::default()).await.expect("open");
    (session, handle)
}

#[tokio::test(start_paused = true)]
async fn first_signal_is_the_snapshot_then_accepted_events_follow() {
    let (session, link) = single(Arc::default()).await;
    assert_eq!(snapshot_task_ids(next(&session).await), ["a"]);

    link.send_frame(daemon_frame(6, progress("a", 4321)));
    match next(&session).await {
        Some(HostSignalDto::Event {
            event: HostEventDto::TaskProgress { task_id, speed, .. },
        }) => assert_eq!((task_id.as_str(), speed), ("a", 4321)),
        other => panic!("expected TaskProgress, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn duplicate_and_stale_frames_are_skipped() {
    let (session, link) = single(Arc::default()).await;
    next(&session).await;
    link.send_frame(deleted_frame(5, "a"));
    link.send_frame(deleted_frame(3, "a"));
    link.send_frame(deleted_frame(6, "a"));
    match next(&session).await {
        Some(HostSignalDto::Event {
            event: HostEventDto::TaskDeleted { task_id },
        }) => assert_eq!(task_id, "a"),
        other => panic!("expected the seq-6 delete only, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn frames_buffered_during_snapshot_are_replayed_after_it() {
    let connector = Arc::new(ScriptedConnector::default());
    let (link, handle) = mock_link(agent_hello());
    connector.push(Ok(link));
    let opening = tokio::spawn(open(connector, Timing::default()));

    // 快照应答被扣住时到达的帧：旧 epoch、已含于快照、连续的 6 与 7。
    let mut other_epoch = deleted_frame(6, "a");
    other_epoch.epoch = "older-epoch".to_owned();
    handle.send_frame(other_epoch);
    handle.send_frame(deleted_frame(4, "a"));
    handle.send_frame(daemon_frame(6, progress("a", 100)));
    handle.send_frame(daemon_frame(7, progress("a", 200)));
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        handle.recorder.snapshot_calls(),
        1,
        "snapshot still pending"
    );
    handle.send_snapshot(5, &["a"]);

    let (session, _driver) = opening.await.expect("join").expect("open");
    assert_eq!(snapshot_task_ids(next(&session).await), ["a"]);
    let mut speeds = Vec::new();
    for _ in 0..2 {
        match next(&session).await {
            Some(HostSignalDto::Event {
                event: HostEventDto::TaskProgress { speed, .. },
            }) => speeds.push(speed),
            other => panic!("expected replayed TaskProgress, got {other:?}"),
        }
    }
    assert_eq!(speeds, [100, 200]);
    assert!(
        timeout(Duration::from_millis(50), session.next_signal())
            .await
            .is_err(),
        "nothing else was replayed"
    );
}

#[tokio::test(start_paused = true)]
async fn sequence_gap_refetches_the_snapshot_on_the_same_connection() {
    let (session, link) = single(Arc::default()).await;
    next(&session).await;
    link.send_snapshot(8, &["a", "b"]);
    link.send_frame(deleted_frame(7 + 1, "a"));
    assert_eq!(snapshot_task_ids(next(&session).await), ["a", "b"]);
    assert_eq!(link.recorder.snapshot_calls(), 2);

    // 重同步后的游标从新快照继续。
    link.send_frame(deleted_frame(9, "b"));
    assert!(matches!(
        next(&session).await,
        Some(HostSignalDto::Event {
            event: HostEventDto::TaskDeleted { .. }
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn epoch_change_refetches_the_snapshot() {
    let (session, link) = single(Arc::default()).await;
    next(&session).await;
    link.send_snapshot(1, &["fresh"]);
    let mut other = deleted_frame(6, "a");
    other.epoch = "epoch-b".to_owned();
    link.send_frame(other);
    assert_eq!(snapshot_task_ids(next(&session).await), ["fresh"]);
    assert_eq!(link.recorder.snapshot_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn buffer_overflow_during_snapshot_resyncs() {
    let connector = Arc::new(ScriptedConnector::default());
    let (link, handle) = mock_link(agent_hello());
    connector.push(Ok(link));
    let timing = Timing {
        buffer_cap: 2,
        ..Timing::default()
    };
    let opening = tokio::spawn(open(connector, timing));
    for sequence in 6..=9 {
        handle.send_frame(deleted_frame(sequence, "a"));
    }
    tokio::time::sleep(Duration::from_millis(5)).await;
    handle.send_snapshot(5, &["old"]); // 与溢出的缓冲不一致 → 丢弃并重取
    handle.send_snapshot(9, &["new"]);
    let (session, _driver) = opening.await.expect("join").expect("open");
    assert_eq!(snapshot_task_ids(next(&session).await), ["new"]);
    assert_eq!(handle.recorder.snapshot_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn stale_is_emitted_only_after_the_grace_and_snapshot_follows_reconnect() {
    let connector = Arc::new(ScriptedConnector::default());
    let (first, first_handle) = mock_link(agent_hello());
    first_handle.send_snapshot(5, &["a"]);
    connector.push(Ok(first));
    // 第一次重连失败 → 退避 > 800ms，宽限先到期。
    connector.push(Err(ConnectFailure::Retry("network down".to_owned())));
    let (second, second_handle) = mock_link(agent_hello());
    second_handle.send_snapshot(50, &["a", "b"]);
    connector.push(Ok(second));

    let (session, _driver) = open(connector, Timing::default()).await.expect("open");
    assert_eq!(snapshot_task_ids(next(&session).await), ["a"]);

    let before = tokio::time::Instant::now();
    first_handle.close(CloseInfo::Lost("reset".to_owned()));
    assert_eq!(next(&session).await, Some(HostSignalDto::Stale));
    assert_eq!(before.elapsed(), Duration::from_millis(800));
    assert_eq!(snapshot_task_ids(next(&session).await), ["a", "b"]);
}

#[tokio::test(start_paused = true)]
async fn quick_reconnect_inside_the_grace_never_goes_stale() {
    let connector = Arc::new(ScriptedConnector::default());
    let (first, first_handle) = mock_link(agent_hello());
    first_handle.send_snapshot(5, &["a"]);
    connector.push(Ok(first));
    let (second, second_handle) = mock_link(agent_hello());
    second_handle.send_snapshot(9, &["a"]);
    connector.push(Ok(second));

    let (session, _driver) = open(connector, Timing::default()).await.expect("open");
    next(&session).await;
    first_handle.close(CloseInfo::Lost("reset".to_owned()));
    // 首次退避 500ms ± 20% < 800ms：下一个信号直接是新快照。
    assert_eq!(snapshot_task_ids(next(&session).await), ["a"]);
}

#[tokio::test(start_paused = true)]
async fn event_gap_close_and_lag_reconnect_immediately() {
    for info in [Some(CloseInfo::EventGap), None] {
        let connector = Arc::new(ScriptedConnector::default());
        let (first, first_handle) = mock_link(agent_hello());
        first_handle.send_snapshot(5, &["a"]);
        connector.push(Ok(first));
        let (second, second_handle) = mock_link(agent_hello());
        second_handle.send_snapshot(1, &["z"]);
        connector.push(Ok(second));

        let (session, _driver) = open(Arc::clone(&connector), Timing::default())
            .await
            .expect("open");
        next(&session).await;
        let before = tokio::time::Instant::now();
        match info {
            Some(info) => first_handle.close(info),
            None => first_handle
                .events
                .send(LinkEvent::Lagged)
                .expect("driver alive"),
        }
        assert_eq!(snapshot_task_ids(next(&session).await), ["z"]);
        assert_eq!(before.elapsed(), Duration::ZERO, "no backoff on gap / lag");
        assert_eq!(connector.connects(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn service_quit_is_fatal_and_ends_the_stream() {
    let (session, link) = single(Arc::default()).await;
    next(&session).await;
    link.close(CloseInfo::ServiceQuit);
    match next(&session).await {
        Some(HostSignalDto::Fatal { error }) => {
            assert_eq!(error.code, ErrorCodeDto::Unavailable);
            assert!(!error.retryable);
        }
        other => panic!("expected Fatal, got {other:?}"),
    }
    assert_eq!(next(&session).await, None);
    assert!(matches!(
        session.pause("a".to_owned()).await,
        Err(FluxError::Closed)
    ));
}

#[tokio::test(start_paused = true)]
async fn first_connect_failure_is_returned_not_retried() {
    let connector = Arc::new(ScriptedConnector::default());
    connector.push(Err(ConnectFailure::Retry("refused".to_owned())));
    let error = open(Arc::clone(&connector), Timing::default())
        .await
        .err()
        .expect("open fails");
    assert!(matches!(&error, FluxError::Transport { detail } if detail == "refused"));
    assert_eq!(connector.connects(), 1);
}

#[tokio::test(start_paused = true)]
async fn unauthorized_connect_surfaces_as_rpc_unauthorized() {
    let connector = Arc::new(ScriptedConnector::default());
    connector.push(Err(ConnectFailure::Fatal(HostErrorDto::new(
        ErrorCodeDto::Unauthorized,
        "access key rejected",
    ))));
    let error = open(connector, Timing::default())
        .await
        .err()
        .expect("open fails");
    assert!(matches!(
        error,
        FluxError::Rpc {
            code: ErrorCodeDto::Unauthorized,
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn incompatible_hello_is_fatal_protocol_incompatible() {
    let mut hello = agent_hello();
    hello.protocol_version = 6;
    let connector = Arc::new(ScriptedConnector::default());
    let (link, _handle) = mock_link(hello);
    connector.push(Ok(link));
    let error = open(connector, Timing::default())
        .await
        .err()
        .expect("open fails");
    assert!(matches!(
        error,
        FluxError::Rpc {
            code: ErrorCodeDto::ProtocolIncompatible,
            ..
        }
    ));

    let mut daemon = agent_hello();
    daemon.role = fluxdown_protocol::ServiceRole::Daemon;
    let connector = Arc::new(ScriptedConnector::default());
    let (link, _handle) = mock_link(daemon);
    connector.push(Ok(link));
    assert!(open(connector, Timing::default()).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn non_service_event_frame_resyncs() {
    let (session, link) = single(Arc::default()).await;
    next(&session).await;
    link.send_snapshot(1, &["after"]);
    link.send_frame(fluxdown_protocol::EventFrame {
        epoch: crate::projection::fixtures::EPOCH.to_owned(),
        sequence: 6,
        previous: None,
        event: fluxdown_protocol::ServiceEvent::Daemon(
            fluxdown_protocol::DaemonEvent::SelectionResolved {
                request_id: "r".to_owned(),
            },
        ),
    });
    assert_eq!(snapshot_task_ids(next(&session).await), ["after"]);
}

// ---- 任务表分页载入（protocol v17：快照 / 整表替换只带头） ----

fn snapshot_of(signal: Option<HostSignalDto>) -> crate::dto::HostSnapshotDto {
    match signal {
        Some(HostSignalDto::Snapshot { snapshot }) => snapshot,
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

fn sorted_ids(snapshot: &crate::dto::HostSnapshotDto) -> Vec<String> {
    let mut ids: Vec<String> = snapshot
        .tasks
        .iter()
        .map(|task| task.task_id.clone())
        .collect();
    ids.sort();
    ids
}

/// 载入期间不得有任何信号（帧只缓冲不应用）。
async fn assert_quiet(session: &HostSession) {
    assert!(
        timeout(Duration::from_millis(50), session.next_signal())
            .await
            .is_err(),
        "no signal may be emitted while the tables load"
    );
}

fn changed_frame(sequence: u64, task_id: &str) -> fluxdown_protocol::EventFrame {
    daemon_frame(
        sequence,
        fluxdown_protocol::DaemonEvent::TaskChanged(crate::projection::fixtures::task(task_id, 2)),
    )
}

/// 下一个信号必须是 `TaskChanged` / `TaskDeleted`，返回 `("changed" | "deleted", id)`。
async fn next_task_event(session: &HostSession) -> (&'static str, String) {
    match next(session).await {
        Some(HostSignalDto::Event {
            event: HostEventDto::TaskChanged { task },
        }) => ("changed", task.task_id),
        Some(HostSignalDto::Event {
            event: HostEventDto::TaskDeleted { task_id },
        }) => ("deleted", task_id),
        other => panic!("expected a task event, got {other:?}"),
    }
}

/// 首次连接：快照头带 `taskTotal`，返回会话与句柄（已收下加载态 Snapshot）。
async fn open_loading(
    total: u64,
    timing: Timing,
) -> (Arc<HostSession>, LinkHandle, Arc<ScriptedConnector>) {
    let connector = Arc::new(ScriptedConnector::default());
    let (link, handle) = mock_link(agent_hello());
    handle.send_header(5, total, None);
    connector.push(Ok(link));
    let (session, _driver) = open(Arc::clone(&connector), timing).await.expect("open");
    let loading = snapshot_of(next(&session).await);
    assert!(loading.tasks_loading, "header snapshot is a loading state");
    assert!(loading.tasks.is_empty(), "no half table is exposed");
    assert_eq!(loading.task_total, total);
    (session, handle, connector)
}

#[tokio::test(start_paused = true)]
async fn bootstrap_pages_tasks_and_replays_changes_arriving_during_paging() {
    let (session, link, _connector) = open_loading(3, Timing::default()).await;

    link.send_task_page(3, &["a", "b"], Some("b"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    // 翻页中途到达的变更：已翻过的被改、翻页后新增、已翻过又被删。
    link.send_frame(changed_frame(6, "b"));
    link.send_frame(changed_frame(7, "z"));
    link.send_frame(deleted_frame(8, "a"));
    assert_quiet(&session).await;
    link.send_task_page(3, &["c"], None);

    let full = snapshot_of(next(&session).await);
    assert!(!full.tasks_loading);
    assert_eq!(full.task_total, 3);
    assert_eq!(sorted_ids(&full), ["a", "b", "c"]);
    // 缓冲帧按序回放，各一次，不重复。
    assert_eq!(next_task_event(&session).await, ("changed", "b".to_owned()));
    assert_eq!(next_task_event(&session).await, ("changed", "z".to_owned()));
    assert_eq!(next_task_event(&session).await, ("deleted", "a".to_owned()));
    assert_quiet(&session).await;
    assert_eq!(
        link.recorder.page_requests(),
        [
            serde_json::json!({ "section": "tasks" }),
            serde_json::json!({ "section": "tasks", "cursor": "b" }),
        ]
    );
    assert_eq!(link.recorder.snapshot_calls(), 1, "no resync was needed");

    // 载入后游标就是回放到的最后一帧，后续帧照常接收。
    link.send_frame(deleted_frame(9, "c"));
    assert_eq!(next_task_event(&session).await, ("deleted", "c".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn frames_buffered_during_the_handshake_are_replayed_after_the_load() {
    let connector = Arc::new(ScriptedConnector::default());
    let (link, handle) = mock_link(agent_hello());
    connector.push(Ok(link));
    let opening = tokio::spawn(open(connector, Timing::default()));
    // 握手窗口内的帧：旧 epoch（丢弃）、已含于快照（跳过）、之后的变更。
    let mut other_epoch = deleted_frame(6, "a");
    other_epoch.epoch = "older-epoch".to_owned();
    handle.send_frame(other_epoch);
    handle.send_frame(deleted_frame(4, "a"));
    handle.send_frame(changed_frame(6, "n"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    handle.send_header(5, 1, None);
    let (session, _driver) = opening.await.expect("join").expect("open");
    assert!(snapshot_of(next(&session).await).tasks_loading);

    handle.send_task_page(1, &["a"], None);
    let full = snapshot_of(next(&session).await);
    assert!(!full.tasks_loading);
    assert_eq!(sorted_ids(&full), ["a"]);
    assert_eq!(next_task_event(&session).await, ("changed", "n".to_owned()));
    assert_quiet(&session).await;
}

#[tokio::test(start_paused = true)]
async fn tables_reloading_mid_run_reloads_in_place() {
    let (session, link) = single(Arc::default()).await;
    assert_eq!(snapshot_task_ids(next(&session).await), ["a"]);

    link.send_frame(daemon_frame(
        6,
        fluxdown_protocol::DaemonEvent::TablesReloading {
            tasks: Some(2),
            queue: None,
        },
    ));
    let loading = snapshot_of(next(&session).await);
    assert!(loading.tasks_loading);
    assert!(loading.tasks.is_empty());
    assert_eq!(loading.task_total, 2);

    // 载入期间的增量帧只缓冲，不产生 per-task 信号。
    link.send_frame(changed_frame(7, "n"));
    link.send_frame(deleted_frame(8, "x"));
    assert_quiet(&session).await;
    link.send_task_page(2, &["x", "y"], None);

    let full = snapshot_of(next(&session).await);
    assert!(!full.tasks_loading);
    assert_eq!(sorted_ids(&full), ["x", "y"]);
    assert_eq!(next_task_event(&session).await, ("changed", "n".to_owned()));
    assert_eq!(next_task_event(&session).await, ("deleted", "x".to_owned()));
    assert_eq!(link.recorder.page_requests().len(), 1);
    assert_eq!(link.recorder.snapshot_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn snapshot_replaced_header_reloads_in_place() {
    let (session, link) = single(Arc::default()).await;
    next(&session).await;
    let header = fluxdown_protocol::DaemonSnapshot {
        task_total: Some(1),
        ..Default::default()
    };
    link.send_frame(crate::projection::fixtures::frame(
        6,
        fluxdown_protocol::AgentEvent::DaemonSnapshotReplaced(header),
    ));
    assert!(snapshot_of(next(&session).await).tasks_loading);
    link.send_task_page(1, &["only"], None);
    let full = snapshot_of(next(&session).await);
    assert!(!full.tasks_loading);
    assert_eq!(sorted_ids(&full), ["only"]);
}

#[tokio::test(start_paused = true)]
async fn a_second_reload_announced_during_replay_loads_again() {
    let (session, link, _connector) = open_loading(1, Timing::default()).await;
    // 载入期间先后到达：变更、再一次重载宣告、重载宣告之后的变更。
    link.send_frame(changed_frame(6, "n"));
    link.send_frame(daemon_frame(
        7,
        fluxdown_protocol::DaemonEvent::TablesReloading {
            tasks: Some(1),
            queue: None,
        },
    ));
    link.send_frame(changed_frame(8, "m"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    link.send_task_page(1, &["a"], None);

    // 第一轮完成 → 回放 6（TaskChanged n）→ 7 触发第二轮（加载态 Snapshot）→ 第二轮页 → 回放 8。
    assert!(!snapshot_of(next(&session).await).tasks_loading);
    assert_eq!(next_task_event(&session).await, ("changed", "n".to_owned()));
    let again = snapshot_of(next(&session).await);
    assert!(again.tasks_loading && again.tasks.is_empty());
    link.send_task_page(1, &["b"], None);
    let full = snapshot_of(next(&session).await);
    assert!(!full.tasks_loading);
    assert_eq!(sorted_ids(&full), ["b"]);
    assert_eq!(next_task_event(&session).await, ("changed", "m".to_owned()));
    assert_eq!(link.recorder.snapshot_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn epoch_change_frame_during_load_resyncs() {
    let (session, link, _connector) = open_loading(2, Timing::default()).await;
    link.send_snapshot(1, &["fresh"]);
    let mut other = deleted_frame(6, "a");
    other.epoch = "epoch-b".to_owned();
    link.send_frame(other);
    let fresh = snapshot_of(next(&session).await);
    assert!(!fresh.tasks_loading);
    assert_eq!(sorted_ids(&fresh), ["fresh"]);
    assert_eq!(link.recorder.snapshot_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn epoch_change_in_a_page_resyncs() {
    let (session, link, _connector) = open_loading(2, Timing::default()).await;
    link.send_snapshot(1, &["fresh"]);
    link.send_task_page_in_epoch("epoch-b", &["x"]);
    let fresh = snapshot_of(next(&session).await);
    assert_eq!(sorted_ids(&fresh), ["fresh"]);
    assert_eq!(link.recorder.snapshot_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_cursor_that_does_not_advance_resyncs() {
    let (session, link, _connector) = open_loading(3, Timing::default()).await;
    link.send_snapshot(1, &["fresh"]);
    link.send_task_page(3, &["a", "b"], Some("b"));
    link.send_task_page(3, &["c"], Some("b")); // 请求游标 "b"，应答 nextCursor 仍是 "b"
    let fresh = snapshot_of(next(&session).await);
    assert_eq!(sorted_ids(&fresh), ["fresh"]);
    assert_eq!(link.recorder.snapshot_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn backlog_overflow_during_load_resyncs() {
    let (session, link, _connector) = open_loading(2, Timing::default()).await;
    link.send_snapshot(100_000, &["fresh"]);
    // 载入期间缓冲帧超过 `TABLE_SYNC_BACKLOG_LIMIT`：放弃本轮、整体重同步。
    let limit = u64::try_from(fluxdown_protocol::TABLE_SYNC_BACKLOG_LIMIT).expect("fits");
    for sequence in 6..=(6 + limit) {
        link.send_frame(deleted_frame(sequence, "a"));
    }
    let fresh = snapshot_of(next(&session).await);
    assert!(!fresh.tasks_loading);
    assert_eq!(sorted_ids(&fresh), ["fresh"]);
    assert_eq!(link.recorder.snapshot_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn page_timeout_reconnects_and_never_reports_a_half_table() {
    let connector = Arc::new(ScriptedConnector::default());
    let (first, first_handle) = mock_link(agent_hello());
    first_handle.send_header(5, 100, None);
    connector.push(Ok(first));
    let (second, second_handle) = mock_link(agent_hello());
    second_handle.send_snapshot(50, &["a", "b"]);
    connector.push(Ok(second));
    let timing = Timing {
        page_timeout: Duration::from_secs(1),
        ..Timing::default()
    };
    let (session, _driver) = open(Arc::clone(&connector), timing).await.expect("open");
    assert!(snapshot_of(next(&session).await).tasks_loading);
    // 第一页永不应答 → 页超时 → 重连；期间绝不会出现 `tasks_loading = false` 的半份表。
    let recovered = loop {
        match next(&session).await {
            Some(HostSignalDto::Stale) => {}
            other => break snapshot_of(other),
        }
    };
    assert!(!recovered.tasks_loading);
    assert_eq!(sorted_ids(&recovered), ["a", "b"]);
    assert_eq!(connector.connects(), 2);
}

#[test]
fn loaded_projection_has_unique_tasks_and_applies_replayed_changes() {
    use crate::dto::HostInfoDto;
    use crate::projection::{FrameOutcome, Projection, fixtures, task_dtos};
    use fluxdown_protocol::{TablePage, TableSection};

    let mut projection = Projection::from_snapshot(
        HostInfoDto::from(&agent_hello()),
        fixtures::header(5, 3, None),
    )
    .expect("agent snapshot");
    let mut sync = projection.table_sync().expect("loading");
    for frame in [
        changed_frame(6, "b"),
        changed_frame(7, "z"),
        deleted_frame(8, "a"),
        deleted_frame(8, "a"), // 合并 / 重复帧由游标过滤
    ] {
        sync.push_frame(frame).expect("backlog");
    }
    for (ids, next) in [(&["a", "b"][..], Some("b")), (&["c"][..], None)] {
        let _ = sync.next_request().expect("a page is due");
        sync.accept(TablePage {
            epoch: fixtures::EPOCH.to_owned(),
            sequence: 5,
            section: TableSection::Tasks,
            total: 3,
            tasks: ids.iter().map(|id| fixtures::task(id, 1)).collect(),
            queue: Vec::new(),
            next_cursor: next.map(str::to_owned),
        })
        .expect("page accepted");
    }
    assert!(sync.next_request().is_none());
    let (signal, backlog) = projection.finish_load(sync);
    assert!(matches!(
        signal,
        HostSignalDto::Snapshot { snapshot } if !snapshot.tasks_loading && snapshot.tasks.len() == 3
    ));
    for frame in backlog {
        let outcome = projection.accept(frame);
        assert!(!matches!(outcome, FrameOutcome::Resync(_)), "{outcome:?}");
    }
    let mut ids: Vec<String> = task_dtos(&projection)
        .into_iter()
        .map(|task| task.task_id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["b", "c", "z"], "a deleted, z added, each id once");
}
