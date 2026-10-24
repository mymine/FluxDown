//! 任务表分页载入（握手后 / 连接循环里的重载）的无 GUI 测试：内存 ws 对端扮演 agent。

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use fluxdown_protocol::{
    AgentEvent, DaemonEvent, DaemonSnapshot, EventFrame, ServiceEvent, TablePage, TablePageParams,
    TableSync, TaskDto, apply_daemon_event, method,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use super::submission_tests::{Peer, connected_pair};
use super::{
    AgentClientEvent, ClientCommand, PreferenceWrites, SessionEnd, Socket, SubmissionTransport,
    run_connected,
};

const EPOCH: &str = "e";
const TIMEOUT: Duration = Duration::from_secs(10);

fn task(id: &str, status: i32) -> TaskDto {
    serde_json::from_value(json!({
        "taskId": id, "url": "https://example.test/x", "fileName": "x", "saveDir": "/tmp",
        "status": status, "downloadedBytes": 0, "totalBytes": 100, "createdAt": "1"
    }))
    .expect("task fixture")
}

fn task_id(index: usize) -> String {
    format!("t{index:04}")
}

/// 对端 agent 的投影：页取自它自己，事件序号单调递增。
struct Server {
    sequence: u64,
    snapshot: DaemonSnapshot,
}

impl Server {
    fn with_tasks(count: usize, sequence: u64) -> Self {
        let snapshot = DaemonSnapshot {
            tasks: (0..count).map(|index| task(&task_id(index), 2)).collect(),
            ..DaemonSnapshot::default()
        };
        Self { sequence, snapshot }
    }

    fn frame(&mut self, event: DaemonEvent) -> EventFrame {
        self.sequence += 1;
        EventFrame {
            epoch: EPOCH.to_owned(),
            sequence: self.sequence,
            previous: None,
            event: ServiceEvent::Agent(AgentEvent::Daemon(event)),
        }
    }

    /// 服务端状态变化并广播：先应用到自己的投影，再生成帧。
    fn mutate(&mut self, event: DaemonEvent) -> EventFrame {
        apply_daemon_event(&mut self.snapshot, &event);
        self.frame(event)
    }

    fn page(&self, params: &Value) -> Value {
        let params: TablePageParams = serde_json::from_value(params.clone()).expect("page params");
        let page =
            TablePage::build(&self.snapshot, EPOCH, self.sequence, &params).expect("page cursor");
        serde_json::to_value(page).expect("page json")
    }

    fn statuses(&self) -> BTreeMap<String, i32> {
        self.snapshot
            .tasks
            .iter()
            .map(|task| (task.task_id.clone(), task.status))
            .collect()
    }
}

async fn next_request(peer: &mut Peer) -> Value {
    let message = tokio::time::timeout(TIMEOUT, peer.next())
        .await
        .expect("request in time")
        .expect("request frame")
        .expect("valid frame");
    let Message::Text(text) = message else {
        panic!("expected text request")
    };
    serde_json::from_str(&text).expect("JSON request")
}

async fn next_page_request(peer: &mut Peer) -> Value {
    let request = next_request(peer).await;
    assert_eq!(request["method"], method::SYSTEM_TASK_PAGE);
    request
}

async fn send(peer: &mut Peer, value: Value) {
    peer.send(Message::Text(value.to_string().into()))
        .await
        .expect("send to client");
}

async fn reply(peer: &mut Peer, request: &Value, result: Value) {
    send(
        peer,
        json!({"jsonrpc": "2.0", "id": request["id"], "result": result}),
    )
    .await;
}

async fn publish(peer: &mut Peer, frame: &EventFrame) {
    send(
        peer,
        json!({
            "jsonrpc": "2.0",
            "method": method::SERVICE_EVENT,
            "params": serde_json::to_value(frame).expect("frame json"),
        }),
    )
    .await;
}

struct Client {
    commands: mpsc::Sender<ClientCommand>,
    events: mpsc::Receiver<AgentClientEvent>,
    task: tokio::task::JoinHandle<Result<SessionEnd, ()>>,
}

fn start(
    socket: Socket,
    cursor: u64,
    buffered: Vec<EventFrame>,
    tables: Option<TableSync>,
) -> Client {
    let (commands, mut command_rx) = mpsc::channel(16);
    let (events_tx, events) = mpsc::channel(4096);
    let task = tokio::spawn(async move {
        let mut transport =
            SubmissionTransport::new(Arc::new(crate::downloads_port::SubmissionGate::default()));
        run_connected(
            socket,
            &mut command_rx,
            &events_tx,
            super::Bootstrap {
                cursor: (EPOCH.to_owned(), cursor),
                buffered,
                tables,
            },
            PreferenceWrites::new(0),
            &mut transport,
        )
        .await
    });
    Client {
        commands,
        events,
        task,
    }
}

/// 模拟会话层折叠：载入完成装表，事件走与会话相同的 reducer。返回事件标签（按到达序）。
struct Mirror {
    snapshot: DaemonSnapshot,
    log: Vec<String>,
}

impl Mirror {
    async fn next(&mut self, client: &mut Client) {
        let event = tokio::time::timeout(TIMEOUT, client.events.recv())
            .await
            .expect("client event in time")
            .expect("client event channel open");
        match event {
            AgentClientEvent::TablesLoaded(loaded) => {
                assert_eq!(loaded.epoch, EPOCH);
                if let Some(tasks) = loaded.tasks {
                    self.snapshot.install_tasks(tasks);
                }
                if let Some(queue) = loaded.queue {
                    self.snapshot.install_queue(queue);
                }
                self.log.push(format!("loaded:{}", loaded.sequence));
            }
            AgentClientEvent::Event(frame) => {
                if let ServiceEvent::Agent(AgentEvent::Daemon(event)) = &frame.event {
                    apply_daemon_event(&mut self.snapshot, event);
                }
                self.log.push(format!("event:{}", frame.sequence));
            }
            _ => panic!("unexpected client event"),
        }
    }

    fn assert_matches(&self, server: &Server) {
        assert!(
            !self.snapshot.tables_loading(),
            "loading flag must be cleared"
        );
        let ids: Vec<&str> = self
            .snapshot
            .tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect();
        let unique: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len(), "no task may appear twice");
        let statuses: BTreeMap<String, i32> = self
            .snapshot
            .tasks
            .iter()
            .map(|task| (task.task_id.clone(), task.status))
            .collect();
        assert_eq!(
            statuses,
            server.statuses(),
            "projection must equal the server table"
        );
    }
}

fn header_mirror(total: u64) -> Mirror {
    Mirror {
        snapshot: DaemonSnapshot {
            task_total: Some(total),
            ..DaemonSnapshot::default()
        },
        log: Vec::new(),
    }
}

/// 握手拿到头后翻页：翻页中途到达的新增 / 修改 / 删除一个不丢、一个不重；握手期缓冲里已被
/// 快照覆盖的旧帧不倒灌。
#[tokio::test]
async fn paging_after_handshake_converges_with_changes_arriving_mid_paging() {
    let (socket, mut peer) = connected_pair().await;
    let mut server = Server::with_tasks(2_500, 10);
    // 握手期缓冲：序号 10 的旧内容（已被快照覆盖，必须跳过）与序号 11 的修改。
    let stale = EventFrame {
        epoch: EPOCH.to_owned(),
        sequence: 10,
        previous: None,
        event: ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::TaskChanged(task(
            &task_id(7),
            2,
        )))),
    };
    let changed = server.mutate(DaemonEvent::TaskChanged(task(&task_id(7), 3)));
    assert_eq!(changed.sequence, 11);
    let sync = TableSync::new(EPOCH, 10, Some(2_500), None).expect("tables to load");
    let mut client = start(socket, 10, vec![stale, changed], Some(sync));

    // 第一页（2000 行）。
    let first = next_page_request(&mut peer).await;
    assert_eq!(first["params"]["section"], "tasks");
    assert!(first["params"].get("cursor").is_none_or(Value::is_null));
    reply(&mut peer, &first, server.page(&first["params"])).await;

    // 翻页期间：已翻过的任务被删、新增落在已翻过的区间、第二页里的任务被改 / 被删、
    // 游标之后新增。
    let mid = [
        DaemonEvent::TaskDeleted {
            task_id: task_id(5),
        },
        DaemonEvent::TaskChanged(task("t0001x", 1)),
        DaemonEvent::TaskChanged(task(&task_id(2_400), 3)),
        DaemonEvent::TaskDeleted {
            task_id: task_id(2_300),
        },
        DaemonEvent::TaskChanged(task("t2600", 1)),
    ];
    for event in mid {
        let frame = server.mutate(event);
        publish(&mut peer, &frame).await;
    }
    let second = next_page_request(&mut peer).await;
    assert_eq!(second["params"]["cursor"], "t1999");
    reply(&mut peer, &second, server.page(&second["params"])).await;

    let mut mirror = header_mirror(2_500);
    // 载入完成事件先到，随后按序回放 11..=16（10 的旧帧被丢弃）。
    for _ in 0..7 {
        mirror.next(&mut client).await;
    }
    assert_eq!(
        mirror.log,
        [
            "loaded:10",
            "event:11",
            "event:12",
            "event:13",
            "event:14",
            "event:15",
            "event:16"
        ]
    );
    mirror.assert_matches(&server);
    assert_eq!(mirror.snapshot.tasks.len(), 2_500);
    client.task.abort();
}

/// 连接循环里收到 `TablesReloading`：就地翻页，期间在途命令的应答照常路由，载入中途发出的
/// 命令同样；载入期间到达的事件缓冲到载入完成之后。
#[tokio::test]
async fn reload_in_the_connection_loop_routes_command_replies_and_defers_later_frames() {
    let (socket, mut peer) = connected_pair().await;
    let mut server = Server::with_tasks(2_100, 20);
    let mut client = start(socket, 20, Vec::new(), None);
    let mut mirror = Mirror {
        snapshot: server.snapshot.clone(),
        log: Vec::new(),
    };

    // 在途命令。
    let (ack_one, mut ack_one_rx) = oneshot::channel();
    client
        .commands
        .send(ClientCommand {
            method: method::DAEMON_TASK_PAUSE.to_owned(),
            params: Some(json!({"taskId": task_id(1)})),
            ack: ack_one,
        })
        .await
        .expect("send first command");
    let command_one = next_request(&mut peer).await;
    assert_eq!(command_one["method"], method::DAEMON_TASK_PAUSE);

    // 服务端宣告整表重载（自己的表不动）。
    let reloading = server.frame(DaemonEvent::TablesReloading {
        tasks: Some(2_100),
        queue: None,
    });
    publish(&mut peer, &reloading).await;
    mirror.next(&mut client).await;
    assert!(mirror.snapshot.tables_loading());
    assert!(
        mirror.snapshot.tasks.is_empty(),
        "the half-loaded table must not look complete"
    );
    assert_eq!(mirror.snapshot.task_count(), 2_100);

    let first = next_page_request(&mut peer).await;
    // 载入进行中：先答在途命令，应答必须路由给调用方。
    reply(&mut peer, &command_one, json!({"ok": true})).await;
    let first_ack = tokio::time::timeout(TIMEOUT, &mut ack_one_rx)
        .await
        .expect("first ack in time")
        .expect("ack sender alive");
    assert!(
        first_ack.is_ok(),
        "in-flight command reply must still be routed"
    );

    // 载入中途发出的命令：请求照常发出，应答照常路由。
    let (ack_two, ack_two_rx) = oneshot::channel();
    client
        .commands
        .send(ClientCommand {
            method: method::DAEMON_TASK_PAUSE.to_owned(),
            params: Some(json!({"taskId": task_id(2)})),
            ack: ack_two,
        })
        .await
        .expect("send second command");
    let command_two = next_request(&mut peer).await;
    assert_eq!(command_two["method"], method::DAEMON_TASK_PAUSE);
    reply(&mut peer, &command_two, json!({"ok": true})).await;
    let second_ack = tokio::time::timeout(TIMEOUT, ack_two_rx)
        .await
        .expect("second ack in time")
        .expect("ack sender alive");
    assert!(second_ack.is_ok());

    // 载入期间的事件：必须等表装入后才出现。
    let during = server.mutate(DaemonEvent::TaskChanged(task(&task_id(3), 3)));
    publish(&mut peer, &during).await;
    reply(&mut peer, &first, server.page(&first["params"])).await;
    let second = next_page_request(&mut peer).await;
    assert_eq!(second["params"]["cursor"], "t1999");
    reply(&mut peer, &second, server.page(&second["params"])).await;

    mirror.next(&mut client).await;
    mirror.next(&mut client).await;
    assert_eq!(mirror.log, ["event:21", "loaded:21", "event:22"]);
    mirror.assert_matches(&server);
    client.task.abort();
}

/// 页的 epoch 与基准不同（服务端重启）：放弃本轮，连接循环以断线结束并整体重同步。
#[tokio::test]
async fn page_from_another_epoch_aborts_the_connection_for_resync() {
    let (socket, mut peer) = connected_pair().await;
    let sync = TableSync::new(EPOCH, 5, Some(3), None).expect("tables to load");
    let client = start(socket, 5, Vec::new(), Some(sync));
    let request = next_page_request(&mut peer).await;
    let page = TablePage {
        epoch: "restarted".to_owned(),
        sequence: 1,
        section: fluxdown_protocol::TableSection::Tasks,
        total: 1,
        tasks: vec![task("a", 1)],
        queue: Vec::new(),
        next_cursor: None,
    };
    reply(
        &mut peer,
        &request,
        serde_json::to_value(page).expect("page json"),
    )
    .await;
    let result = tokio::time::timeout(TIMEOUT, client.task)
        .await
        .expect("connection ends")
        .expect("task joined");
    assert!(result.is_err());
}

/// 翻页游标不前进、或页请求被拒：同样放弃本轮重同步，不会死循环。
#[tokio::test]
async fn stalled_cursor_and_rejected_page_request_abort_for_resync() {
    let (socket, mut peer) = connected_pair().await;
    let sync = TableSync::new(EPOCH, 5, Some(4_000), None).expect("tables to load");
    let client = start(socket, 5, Vec::new(), Some(sync));
    let page = |next: Option<&str>| TablePage {
        epoch: EPOCH.to_owned(),
        sequence: 5,
        section: fluxdown_protocol::TableSection::Tasks,
        total: 4_000,
        tasks: vec![task("a", 1)],
        queue: Vec::new(),
        next_cursor: next.map(str::to_owned),
    };
    let first = next_page_request(&mut peer).await;
    reply(
        &mut peer,
        &first,
        serde_json::to_value(page(Some("a"))).expect("page"),
    )
    .await;
    let second = next_page_request(&mut peer).await;
    assert_eq!(second["params"]["cursor"], "a");
    reply(
        &mut peer,
        &second,
        serde_json::to_value(page(Some("a"))).expect("page"),
    )
    .await;
    let result = tokio::time::timeout(TIMEOUT, client.task)
        .await
        .expect("connection ends")
        .expect("task joined");
    assert!(result.is_err());

    let (socket, mut peer) = connected_pair().await;
    let sync = TableSync::new(EPOCH, 5, Some(3), None).expect("tables to load");
    let client = start(socket, 5, Vec::new(), Some(sync));
    let request = next_page_request(&mut peer).await;
    send(
        &mut peer,
        json!({
            "jsonrpc": "2.0", "id": request["id"],
            "error": {"code": -32603, "message": "injected page failure"},
        }),
    )
    .await;
    let result = tokio::time::timeout(TIMEOUT, client.task)
        .await
        .expect("connection ends")
        .expect("task joined");
    assert!(result.is_err());
}
