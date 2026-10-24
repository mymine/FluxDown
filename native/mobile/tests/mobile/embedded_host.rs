//! 嵌入式宿主接缝：同一进程内运行真实 daemon（`run_with`，临时端口）与嵌入式 agent，
//! 经 `LocalConnection` 走完整的 hello → snapshot → 事件链路。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fluxdown_agent::{
    AgentStartError, EmbeddedAgent, EmbeddedConfig, LocalConnection, LocalEventError,
    start_embedded,
};
use fluxdown_daemon::config::DaemonConfig;
use fluxdown_daemon::runtime::{DaemonReady, run_with};
use fluxdown_protocol::{
    ApplicationErrorCode, ErrorReason, PROTOCOL_VERSION, RequestId, RpcRequest, RpcResponse,
    ServiceEvent, ServiceRole, method,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const STEP: Duration = Duration::from_secs(30);

type DaemonOutcome = Result<(), Box<dyn Error + Send + Sync>>;

/// 进程内 daemon（临时 loopback 端口）及其数据目录。
struct Daemon {
    root: PathBuf,
    cancel: CancellationToken,
    task: JoinHandle<DaemonOutcome>,
    ready: DaemonReady,
}

impl Daemon {
    async fn start(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "fluxdown_embedded_{label}_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let cancel = CancellationToken::new();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let config = DaemonConfig::embedded(
            root.join("engine"),
            Some(root.join("downloads").display().to_string()),
        );
        let task = tokio::spawn(run_with(config, cancel.clone(), Some(ready_tx)));
        let ready = tokio::time::timeout(STEP, ready_rx)
            .await
            .expect("daemon ready timeout")
            .expect("daemon exited before reporting ready");
        Self {
            root,
            cancel,
            task,
            ready,
        }
    }

    fn agent_config(&self, agent_dir: &str) -> EmbeddedConfig {
        EmbeddedConfig {
            agent_data_dir: self.root.join(agent_dir),
            engine_data_dir: self.root.join("engine"),
            daemon_url: format!("ws://{}/rpc", self.ready.addr),
            daemon_token: self.ready.token.clone(),
            client_platform: "android".to_owned(),
            client_device_name: None,
            enable_link: false,
        }
    }

    async fn stop(self) {
        self.cancel.cancel();
        tokio::time::timeout(STEP, self.task)
            .await
            .expect("daemon shutdown timeout")
            .expect("daemon task join")
            .expect("daemon shutdown result");
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!("test cleanup failed for {}: {error}", self.root.display());
        }
    }
}

fn hello_request(id: &str, params: Value) -> RpcRequest {
    RpcRequest::new(
        RequestId::String(id.to_owned()),
        method::SYSTEM_HELLO,
        Some(params),
    )
}

fn valid_hello() -> RpcRequest {
    hello_request(
        "hello",
        json!({
            "clientName": "embedded-host-test",
            "clientVersion": "test",
            "minProtocolVersion": PROTOCOL_VERSION,
            "maxProtocolVersion": PROTOCOL_VERSION,
            "requestedRole": "agent",
            "capabilities": [method::CAPABILITY_CLIENT_SELECTIONS],
        }),
    )
}

fn call_request(id: RequestId, method_name: &str, params: Option<Value>) -> RpcRequest {
    RpcRequest::new(id, method_name, params)
}

fn success(response: RpcResponse) -> (RequestId, Value) {
    match response {
        RpcResponse::Success(success) => (success.id, success.result),
        RpcResponse::Failure(failure) => panic!("expected success, got {failure:?}"),
    }
}

async fn connect(agent: &EmbeddedAgent) -> LocalConnection {
    agent
        .connect(valid_hello())
        .await
        .expect("embedded hello accepted")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn embedded_agent_serves_an_agent_snapshot_over_a_local_connection() {
    let daemon = Daemon::start("snapshot").await;
    assert!(daemon.ready.addr.ip().is_loopback());
    assert_ne!(daemon.ready.addr.port(), 0);
    assert!(!daemon.ready.token.is_empty());

    let cancel = CancellationToken::new();
    let agent = start_embedded(daemon.agent_config("agent"), cancel.clone())
        .await
        .expect("start embedded agent");
    let connection = connect(&agent).await;
    assert_eq!(connection.hello().role, ServiceRole::Agent);
    assert_eq!(connection.hello().protocol_version, PROTOCOL_VERSION);

    let response = connection
        .call(call_request(
            RequestId::String("snap".to_owned()),
            method::SYSTEM_SNAPSHOT,
            None,
        ))
        .await;
    let (id, snapshot) = success(response);
    assert_eq!(id, RequestId::String("snap".to_owned()));
    assert_eq!(snapshot["body"]["role"], json!("agent"));
    let body = &snapshot["body"]["snapshot"];
    assert_eq!(body["daemonConnected"], json!(true));
    assert_eq!(
        body["daemon"]["config"]["values"]["default_save_dir"],
        json!(daemon.root.join("downloads").display().to_string())
    );

    let (_, pong) = success(
        connection
            .call(call_request(
                RequestId::Integer(7),
                method::SYSTEM_PING,
                None,
            ))
            .await,
    );
    assert_eq!(pong, json!({ "ok": true }));

    drop(connection);
    agent.shutdown().await;
    cancel.cancel();
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_rejects_hello_that_a_websocket_client_would_be_refused() {
    let daemon = Daemon::start("bad_hello").await;
    let agent = start_embedded(daemon.agent_config("agent"), CancellationToken::new())
        .await
        .expect("start embedded agent");

    let rejections = [
        // 角色错误：agent 网关只接受 `agent` 角色。
        hello_request(
            "wrong-role",
            json!({
                "clientName": "t",
                "clientVersion": "t",
                "minProtocolVersion": PROTOCOL_VERSION,
                "maxProtocolVersion": PROTOCOL_VERSION,
                "requestedRole": "daemon",
            }),
        ),
        // 协议区间与服务端不相交。
        hello_request(
            "future-protocol",
            json!({
                "clientName": "t",
                "clientVersion": "t",
                "minProtocolVersion": PROTOCOL_VERSION + 1,
                "maxProtocolVersion": PROTOCOL_VERSION + 1,
                "requestedRole": "agent",
            }),
        ),
        // 首个请求不是 `system.hello`。
        call_request(RequestId::Integer(1), method::SYSTEM_SNAPSHOT, None),
        // 缺少参数。
        RpcRequest::new(RequestId::Integer(2), method::SYSTEM_HELLO, None),
    ];
    for request in rejections {
        let id = request.id.clone();
        let error = agent
            .connect(request)
            .await
            .err()
            .unwrap_or_else(|| panic!("hello {id:?} must be rejected"));
        assert!(
            error.data.is_some(),
            "rejection {id:?} carries stable error data: {error:?}"
        );
    }
    let error = agent
        .connect(hello_request(
            "future-protocol",
            json!({
                "clientName": "t",
                "clientVersion": "t",
                "minProtocolVersion": PROTOCOL_VERSION + 1,
                "maxProtocolVersion": PROTOCOL_VERSION + 1,
                "requestedRole": "agent",
            }),
        ))
        .await
        .err()
        .expect("future protocol is refused");
    assert_eq!(
        error.data.map(|data| data.code),
        Some(ApplicationErrorCode::ProtocolIncompatible)
    );

    // 被拒绝的握手不留下连接：之后合法握手照常成功。
    let connection = connect(&agent).await;
    drop(connection);
    agent.shutdown().await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_calls_keep_their_own_ids_and_events_arrive_while_calls_are_in_flight() {
    let daemon = Daemon::start("concurrency").await;
    let agent = start_embedded(daemon.agent_config("agent"), CancellationToken::new())
        .await
        .expect("start embedded agent");
    let connection = Arc::new(connect(&agent).await);

    // 两个调用方用相同的 id 并发调用：响应不得串线。
    let calls = (0..8).map(|index| {
        let connection = Arc::clone(&connection);
        tokio::spawn(async move {
            let method_name = if index % 2 == 0 {
                method::SYSTEM_PING
            } else {
                method::SYSTEM_SNAPSHOT
            };
            let response = connection
                .call(call_request(RequestId::Integer(1), method_name, None))
                .await;
            (index, success(response))
        })
    });
    for call in calls {
        let (index, (id, result)) = tokio::time::timeout(STEP, call)
            .await
            .expect("concurrent call timeout")
            .expect("call task");
        assert_eq!(id, RequestId::Integer(1));
        if index % 2 == 0 {
            assert_eq!(result, json!({ "ok": true }));
        } else {
            assert_eq!(result["body"]["role"], json!("agent"));
        }
    }

    // `next_event(&self)` 与 `call(&self)` 可同时使用同一个连接。
    let events = Arc::clone(&connection);
    let waiter = tokio::spawn(async move {
        loop {
            let frame = events.next_event().await.expect("event stream open");
            if matches!(frame.event, ServiceEvent::Agent(_)) {
                return frame;
            }
        }
    });
    let (_, patched) = success(
        connection
            .call(call_request(
                RequestId::Integer(2),
                method::AGENT_PREFERENCES_PATCH,
                Some(json!({ "values": { "download.keep_awake": true }, "sync": false })),
            ))
            .await,
    );
    assert_eq!(patched["ok"], json!(true));
    tokio::time::timeout(STEP, waiter)
        .await
        .expect("event delivery timeout")
        .expect("event task");

    drop(connection);
    agent.shutdown().await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_closes_event_streams_and_releases_the_agent_data_directory() {
    let daemon = Daemon::start("restart").await;
    let config = daemon.agent_config("agent");

    let agent = start_embedded(config.clone(), CancellationToken::new())
        .await
        .expect("start embedded agent");
    // 同一数据目录同时只允许一个 agent。
    let second = start_embedded(config.clone(), CancellationToken::new()).await;
    assert!(
        matches!(second, Err(AgentStartError::DataDirLocked(_))),
        "second agent on the same directory must be refused"
    );

    let connection = connect(&agent).await;
    let observer = tokio::spawn(async move {
        let event = connection.next_event().await.map(|_| ());
        connection.close().await;
        event
    });
    agent.shutdown().await;
    let ended = tokio::time::timeout(STEP, observer)
        .await
        .expect("event stream must end after shutdown")
        .expect("observer task");
    assert!(
        matches!(ended, Err(LocalEventError::Closed)),
        "unexpected stream end: {ended:?}"
    );

    // 关停且连接已显式关闭后，同一目录可以立即再次启动（锁已释放），并重新拿到快照。
    let restarted = start_embedded(config, CancellationToken::new())
        .await
        .expect("restart embedded agent on the same directories");
    let connection = connect(&restarted).await;
    let (_, snapshot) = success(
        connection
            .call(call_request(
                RequestId::Integer(1),
                method::SYSTEM_SNAPSHOT,
                None,
            ))
            .await,
    );
    assert_eq!(snapshot["body"]["snapshot"]["daemonConnected"], json!(true));
    drop(connection);
    restarted.shutdown().await;
    daemon.stop().await;
}

/// 本机嵌入主机启用互联（手机上局域网配对添加远程主机）：只做出站，带 `agent.deviceLink`
/// 能力，`agent.link.*` 可用；设备身份落在本机 agent 状态里，重启不变；空闲时不写盘。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn embedded_agent_with_link_pairs_outbound_only_and_keeps_its_identity() {
    let daemon = Daemon::start("link").await;
    let mut config = daemon.agent_config("agent");
    config.enable_link = true;
    let agent_dir = config.agent_data_dir.clone();

    let agent = start_embedded(config.clone(), CancellationToken::new())
        .await
        .expect("start embedded agent with link");
    let connection = connect(&agent).await;
    assert!(
        connection
            .hello()
            .capabilities
            .iter()
            .any(|capability| capability == method::CAPABILITY_AGENT_DEVICE_LINK),
        "link-enabled hosts must advertise agent.deviceLink"
    );
    let (_, code) = success(
        connection
            .call(call_request(
                RequestId::Integer(1),
                method::AGENT_LINK_PAIRING_CODE,
                None,
            ))
            .await,
    );
    let fingerprint = code["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_owned();
    assert_eq!(fingerprint.len(), 64);
    // 没有监听端口：没有可被对端直连的地址，也不广播。
    assert_eq!(code["addresses"], json!([]));
    let (_, devices) = success(
        connection
            .call(call_request(
                RequestId::Integer(2),
                method::AGENT_LINK_REFRESH,
                None,
            ))
            .await,
    );
    assert_eq!(devices, json!([]));

    // 出站配对（请求控制）：对端不可达得到稳定的「无法连接对端」原因，而不是 Unsupported。
    let failure = match connection
        .call(call_request(
            RequestId::Integer(3),
            method::AGENT_LINK_PAIR_BEGIN,
            Some(json!({
                "address": "127.0.0.1:1",
                "code": "123456",
                "requestControl": true,
            })),
        ))
        .await
    {
        RpcResponse::Failure(failure) => failure,
        RpcResponse::Success(success) => panic!("pairing a dead address succeeded: {success:?}"),
    };
    assert_eq!(
        failure.error.data.and_then(|data| data.reason),
        Some(ErrorReason::PairingPeerUnreachable)
    );
    connection.close().await;
    agent.shutdown().await;

    // 身份在本机 agent 状态里：同一数据目录重启后指纹不变。
    let restarted = start_embedded(config, CancellationToken::new())
        .await
        .expect("restart embedded agent with link");
    let connection = connect(&restarted).await;
    let (_, again) = success(
        connection
            .call(call_request(
                RequestId::Integer(1),
                method::AGENT_LINK_PAIRING_CODE,
                None,
            ))
            .await,
    );
    assert_eq!(again["fingerprint"], json!(fingerprint));

    // 空闲静默：互联服务的后台轮询（名册为空时无事可做）不写盘。
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = tree_stamp(&agent_dir);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        tree_stamp(&agent_dir),
        before,
        "an idle link-enabled agent must not write its state"
    );

    drop(connection);
    restarted.shutdown().await;
    daemon.stop().await;
}

/// 目录树的（路径、修改时间、长度）快照，用来断言空闲期间没有任何文件被写。
fn tree_stamp(dir: &std::path::Path) -> Vec<(PathBuf, std::time::SystemTime, u64)> {
    let mut stamp = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current).expect("read agent dir") {
            let entry = entry.expect("agent dir entry");
            let metadata = entry.metadata().expect("agent file metadata");
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                stamp.push((
                    entry.path(),
                    metadata.modified().expect("modified time"),
                    metadata.len(),
                ));
            }
        }
    }
    stamp.sort();
    stamp
}

/// 嵌入宿主没有 TCP 网关 / NMH IPC / 兼容 HTTP API，也不跑在机械盘上：这些检查不得出现
/// （否则在手机上恒报异常，或探到同机别的程序的 17800 端口）；daemon 版本取自 hello 的
/// `serviceVersion`；本机日志导出（App 给出的沙盒路径）可用，server 模式的拒绝不适用。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn embedded_diagnostics_skip_gateway_checks_and_export_logs_locally() {
    let daemon = Daemon::start("diagnostics").await;
    let agent = start_embedded(daemon.agent_config("agent"), CancellationToken::new())
        .await
        .expect("start embedded agent");
    let connection = connect(&agent).await;

    let (_, report) = success(
        connection
            .call(call_request(
                RequestId::Integer(1),
                method::AGENT_DIAGNOSTICS_RUN,
                None,
            ))
            .await,
    );
    let checks = report["checks"].as_array().expect("checks array");
    let ids: Vec<&str> = checks
        .iter()
        .filter_map(|check| check["id"].as_str())
        .collect();
    for absent in ["app_listener", "local_server", "disk_sleep"] {
        assert!(!ids.contains(&absent), "{absent} must be omitted: {ids:?}");
    }
    let daemon_check = checks
        .iter()
        .find(|check| check["id"] == json!("daemon"))
        .expect("daemon check present");
    assert_eq!(daemon_check["level"], json!("ok"));
    let detail = daemon_check["detail"].as_str().expect("daemon detail");
    assert!(
        detail.contains(&format!("(v{};", fluxdown_protocol::APP_VERSION)),
        "daemon version comes from serviceVersion: {detail}"
    );

    let target = daemon.root.join("export").join("logs.zip");
    let (_, exported) = success(
        connection
            .call(call_request(
                RequestId::Integer(2),
                method::AGENT_DIAGNOSTICS_EXPORT_LOGS,
                Some(json!({ "targetPath": target.display().to_string() })),
            ))
            .await,
    );
    assert!(exported["bytes"].as_u64().is_some_and(|bytes| bytes > 0));
    let bytes = std::fs::read(exported["path"].as_str().expect("export path")).expect("zip");
    assert!(bytes.starts_with(b"PK"), "export is a zip archive");

    drop(connection);
    agent.shutdown().await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_loopback_daemon_endpoint_is_refused_without_side_effects() {
    let root = std::env::temp_dir().join(format!(
        "fluxdown_embedded_endpoint_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = start_embedded(
        EmbeddedConfig {
            agent_data_dir: root.join("agent"),
            engine_data_dir: root.join("engine"),
            daemon_url: "ws://203.0.113.7:17801/rpc".to_owned(),
            daemon_token: "token".to_owned(),
            client_platform: "android".to_owned(),
            client_device_name: None,
            enable_link: false,
        },
        CancellationToken::new(),
    )
    .await;
    assert!(
        matches!(result, Err(AgentStartError::DaemonEndpoint(_))),
        "non-loopback daemon must be refused"
    );
    assert!(!root.exists(), "no state is created before validation");
}
