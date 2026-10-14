//! GPUI composition root 唯一 agent JSON-RPC 会话。

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fluxdown_protocol::method;
use fluxdown_protocol::{
    ApplicationErrorCode, CLOSE_REASON_SERVICE_QUIT, EventFrame, RequestId, RpcErrorData,
    RpcNotification, RpcRequest, RpcResponse, ServiceHello, ServiceRole, Snapshot, SnapshotBody,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::preference_writes::PreferenceWrites;
use crate::service_bootstrap::ServiceBootstrap;

pub type AgentFuture<T> = Pin<Box<dyn Future<Output = Result<T, RpcErrorData>> + Send + 'static>>;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub enum AgentClientEvent {
    /// 连接 / 重连后的全量快照（带 `epoch`/`sequence` 游标）。
    Snapshot(Box<Snapshot>),
    Event(Box<EventFrame>),
    Stale,
    /// 本进程刚拉起了 agent（后台服务冷启动）：首个快照要等 agent 装配完成。
    ServiceStarting,
    Fatal(RpcErrorData),
    /// agent 执行了完全退出（`system.shutdown`）：不再重连，界面随之退出。
    ServiceStopped,
}

#[derive(Clone)]
pub struct AgentClientConfig {
    pub rpc_url: Option<String>,
    pub bearer_path: PathBuf,
}

struct ClientCommand {
    method: String,
    params: Option<Value>,
    ack: oneshot::Sender<Result<Value, RpcErrorData>>,
}

impl ClientCommand {
    fn is_submission(&self) -> bool {
        match self.method.as_str() {
            method::DAEMON_TASK_CREATE
            | method::DAEMON_GROUP_CREATE
            | method::AGENT_CAPTURE_CREATE_GROUP
            | method::AGENT_CAPTURE_SUBMIT_TORRENT_FILE => true,
            method::AGENT_CAPTURE_RESOLVE => {
                self.params
                    .as_ref()
                    .and_then(|params| params.get("accepted"))
                    .and_then(Value::as_bool)
                    == Some(true)
            }
            _ => false,
        }
    }
}

/// Unsent creates survive reconnect without becoming ambiguous requests.
struct SubmissionTransport {
    gate: Arc<crate::downloads_port::SubmissionGate>,
    unsent: VecDeque<ClientCommand>,
}

impl SubmissionTransport {
    fn new(gate: Arc<crate::downloads_port::SubmissionGate>) -> Self {
        Self {
            gate,
            unsent: VecDeque::new(),
        }
    }
}

struct PendingCommand {
    ack: oneshot::Sender<Result<Value, RpcErrorData>>,
    _admission: Option<tokio::sync::OwnedMutexGuard<()>>,
}

pub struct AgentClient {
    commands: mpsc::Sender<ClientCommand>,
    runtime: Arc<tokio::runtime::Runtime>,
    bootstrap: Arc<ServiceBootstrap>,
    pub(crate) submissions: Arc<crate::downloads_port::SubmissionGate>,
}

impl AgentClient {
    pub fn start(
        config: AgentClientConfig,
        bootstrap: Arc<ServiceBootstrap>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<AgentClientEvent>), AgentClientError> {
        if let Some(url) = config.rpc_url.as_deref() {
            validate_url(url)?;
        }
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("fluxdown-agent-client")
                .build()
                .map_err(AgentClientError::Runtime)?,
        );
        let (commands, command_rx) = mpsc::channel(64);
        let (events, event_rx) = mpsc::channel(1024);
        let submissions = Arc::new(crate::downloads_port::SubmissionGate::default());
        runtime.spawn(run_client(
            config,
            bootstrap.clone(),
            command_rx,
            events,
            submissions.clone(),
        ));
        Ok((
            Arc::new(Self {
                commands,
                runtime,
                bootstrap,
                submissions,
            }),
            event_rx,
        ))
    }

    /// 界面发起完全退出后调用：连接断开时不再拉起新的 agent。
    pub fn stop_service_bootstrap(&self) {
        self.bootstrap.stop();
    }

    pub fn call<P, R>(&self, method_name: &str, params: Option<P>) -> AgentFuture<R>
    where
        P: Serialize + Send + 'static,
        R: DeserializeOwned + Send + 'static,
    {
        let commands = self.commands.clone();
        let method_name = method_name.to_owned();
        Box::pin(async move {
            let params = params
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| internal_error())?;
            let (ack, response) = oneshot::channel();
            commands
                .send(ClientCommand {
                    method: method_name,
                    params,
                    ack,
                })
                .await
                .map_err(|_| unavailable_error())?;
            let value = response.await.map_err(|_| unavailable_error())??;
            serde_json::from_value(value).map_err(|_| internal_error())
        })
    }

    /// 在客户端自有 tokio 运行时上运行后台任务（单实例激活监听等）。
    pub fn spawn_background<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future);
    }
}

async fn run_client(
    config: AgentClientConfig,
    bootstrap: Arc<ServiceBootstrap>,
    mut commands: mpsc::Receiver<ClientCommand>,
    events: mpsc::Sender<AgentClientEvent>,
    submissions: Arc<crate::downloads_port::SubmissionGate>,
) {
    let mut attempt = 0_usize;
    // 每个桌面进程只尝试替换一次协议不兼容的 agent，避免同目录二进制错配时反复互杀。
    let mut replaced_incompatible = false;
    let mut link_log = ConnectionLog::default();
    let mut submission_transport = SubmissionTransport::new(submissions.clone());
    loop {
        submissions.set_ready(false);
        let rpc_url = match crate::agent_endpoint::discover_rpc_url(
            config.rpc_url.as_deref(),
            &config.bearer_path,
        )
        .await
        {
            Ok(url) => url,
            Err(error) => {
                log::error!("could not discover local agent endpoint: {error}");
                if events
                    .send(AgentClientEvent::Fatal(internal_error()))
                    .await
                    .is_err()
                {
                    log::debug!("agent event receiver closed during desktop shutdown");
                }
                return;
            }
        };
        match connect(&config, &rpc_url).await {
            Ok((socket, snapshot, buffered)) => {
                attempt = 0;
                link_log.connected();
                let cursor = (snapshot.epoch.clone(), snapshot.sequence);
                let preferences_revision = match &snapshot.body {
                    SnapshotBody::Agent(body) => body.preferences.revision,
                    SnapshotBody::Daemon(_) => 0,
                };
                if events
                    .send(AgentClientEvent::Snapshot(Box::new(snapshot)))
                    .await
                    .is_err()
                {
                    return;
                }
                match run_connected(
                    socket,
                    &mut commands,
                    &events,
                    cursor,
                    buffered,
                    PreferenceWrites::new(preferences_revision),
                    &mut submission_transport,
                )
                .await
                {
                    Ok(SessionEnd::ClientDropped) => return,
                    Ok(SessionEnd::ServiceQuit) => {
                        log::info!("agent closed the connection with service-quit (full shutdown)");
                        bootstrap.stop();
                        if events.send(AgentClientEvent::ServiceStopped).await.is_err() {
                            log::debug!("agent event receiver closed during desktop shutdown");
                        }
                        return;
                    }
                    Err(()) => {
                        log::warn!("agent connection lost; reconnecting");
                        if events.send(AgentClientEvent::Stale).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(error @ (ConnectError::NoBearer | ConnectError::Refused)) => {
                let probe_listener = matches!(error, ConnectError::NoBearer);
                link_log.failed(&error);
                match bootstrap.ensure_running(&rpc_url, probe_listener).await {
                    Ok(true) => {
                        if events
                            .send(AgentClientEvent::ServiceStarting)
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(false) => {}
                    Err(bootstrap_error) => {
                        log::warn!("could not start fluxdown-agent: {bootstrap_error}");
                        if events.send(AgentClientEvent::Stale).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(ConnectError::Incompatible) if !replaced_incompatible => {
                replaced_incompatible = true;
                log::warn!("running agent speaks an incompatible protocol; asking it to shut down");
                if request_shutdown(&config, &rpc_url).await {
                    // 旧 agent 关停 daemon 后退出；连接被拒时由 bootstrap 拉起同级新版本。
                    crate::service_bootstrap::wait_until_stopped(&rpc_url, Duration::from_secs(30))
                        .await;
                    attempt = 0;
                    continue;
                }
                log::error!("incompatible agent refused to shut down; giving up");
                if events
                    .send(AgentClientEvent::Fatal(protocol_error()))
                    .await
                    .is_err()
                {
                    log::debug!("agent event receiver closed during desktop shutdown");
                }
                return;
            }
            Err(ConnectError::Incompatible) => {
                log::error!("agent protocol still incompatible after replacement; giving up");
                if events
                    .send(AgentClientEvent::Fatal(protocol_error()))
                    .await
                    .is_err()
                {
                    log::debug!("agent event receiver closed during desktop shutdown");
                }
                return;
            }
            Err(ConnectError::Fatal(error)) => {
                log::error!("fatal agent connection error: {:?}", error.code);
                if events.send(AgentClientEvent::Fatal(error)).await.is_err() {
                    log::debug!("agent event receiver closed during desktop shutdown");
                }
                return;
            }
            Err(error @ ConnectError::Transient(_)) => {
                link_log.failed(&error);
                if events.send(AgentClientEvent::Stale).await.is_err() {
                    return;
                }
            }
        }
        tokio::time::sleep(retry_delay(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

/// 连接诊断：只记状态迁移（失败原因变化、恢复连接）与按 2 的幂递增的持续失败提醒，
/// 不逐次记录 100ms 一次的快速重试。
#[derive(Default)]
struct ConnectionLog {
    failures: u64,
    since: Option<Instant>,
    last_reason: String,
}

impl ConnectionLog {
    fn failed(&mut self, error: &ConnectError) {
        self.failures = self.failures.saturating_add(1);
        let since = *self.since.get_or_insert_with(Instant::now);
        let reason = error.describe();
        // 冷启动期间「尚无 bearer / 端口未监听」是预期状态，只作 info。
        let expected = matches!(error, ConnectError::NoBearer | ConnectError::Refused);
        if reason != self.last_reason {
            if expected {
                log::info!("agent not reachable yet: {reason}");
            } else {
                log::warn!("agent connection attempt failed: {reason}");
            }
            self.last_reason = reason;
        } else if self.failures >= 16 && self.failures.is_power_of_two() {
            log::warn!(
                "agent still unreachable after {} attempts over {}s: {reason}",
                self.failures,
                since.elapsed().as_secs()
            );
        }
    }

    fn connected(&mut self) {
        match self.since.take() {
            Some(since) => log::info!(
                "connected to agent after {} failed attempts ({}ms)",
                self.failures,
                since.elapsed().as_millis()
            ),
            None => log::info!("connected to agent"),
        }
        self.failures = 0;
        self.last_reason.clear();
    }
}

/// 连续快速重试的次数（100ms 间隔，约 5s）：覆盖回环重连、agent 冷启动与重启，让界面在
/// 连接宽限内恢复而不显示连接态；之后指数退避，避免后台长期不可用时空转。
const FAST_RETRY_ATTEMPTS: usize = 50;
const FAST_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const BACKOFF_SECS: [u64; 5] = [1, 2, 5, 15, 30];

/// 第 `attempt` 次（从 0 起）连接失败后的等待时间。
fn retry_delay(attempt: usize) -> Duration {
    match attempt.checked_sub(FAST_RETRY_ATTEMPTS) {
        None => FAST_RETRY_INTERVAL,
        Some(slow) => Duration::from_secs(BACKOFF_SECS[slow.min(BACKOFF_SECS.len() - 1)]),
    }
}

/// 握手前 `system.shutdown`：只有支持该首帧的 agent（协议 v4 起）会受理。
async fn request_shutdown(config: &AgentClientConfig, rpc_url: &str) -> bool {
    let Ok(mut socket) = open_socket(config, rpc_url).await else {
        return false;
    };
    let mut buffered = Vec::new();
    call_on_socket(&mut socket, 1, method::SYSTEM_SHUTDOWN, None, &mut buffered)
        .await
        .is_ok()
}

async fn open_socket(config: &AgentClientConfig, rpc_url: &str) -> Result<Socket, ConnectError> {
    let bearer = tokio::fs::read_to_string(&config.bearer_path)
        .await
        .map_err(|_| ConnectError::NoBearer)?;
    let bearer = bearer.trim();
    if bearer.is_empty() {
        return Err(ConnectError::NoBearer);
    }
    let mut request = rpc_url
        .into_client_request()
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {bearer}"))
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, authorization);
    // 先自行完成 TCP 连接：回环端口无人监听时按超时判定，不等 Windows 约 2s 的拒绝。
    let stream = match crate::service_bootstrap::connect_listener(rpc_url).await {
        Ok(Some(stream)) => stream,
        Ok(None) => return Err(ConnectError::Refused),
        Err(error) => return Err(ConnectError::Transient(format!("connect: {error}"))),
    };
    let (socket, _) = tokio_tungstenite::client_async(request, MaybeTlsStream::Plain(stream))
        .await
        .map_err(classify_connect_error)?;
    Ok(socket)
}

async fn connect(
    config: &AgentClientConfig,
    rpc_url: &str,
) -> Result<(Socket, Snapshot, Vec<EventFrame>), ConnectError> {
    let mut socket = open_socket(config, rpc_url).await?;
    let mut buffered = Vec::new();
    let hello = serde_json::json!({
        "clientName": "fluxdown-desktop",
        "clientVersion": fluxdown_protocol::APP_VERSION,
        "minProtocolVersion": fluxdown_protocol::MIN_PROTOCOL_VERSION,
        "maxProtocolVersion": fluxdown_protocol::PROTOCOL_VERSION,
        "requestedRole": "agent",
        "capabilities": [method::CAPABILITY_CLIENT_SELECTIONS]
    });
    let hello_value = call_on_socket(
        &mut socket,
        1,
        method::SYSTEM_HELLO,
        Some(hello),
        &mut buffered,
    )
    .await
    .map_err(|error| match error {
        ConnectError::Fatal(data) if data.code == ApplicationErrorCode::ProtocolIncompatible => {
            ConnectError::Incompatible
        }
        other => other,
    })?;
    let service = serde_json::from_value::<ServiceHello>(hello_value)
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    if service.role != ServiceRole::Agent {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    if service.protocol_version != fluxdown_protocol::PROTOCOL_VERSION {
        return Err(ConnectError::Incompatible);
    }
    let snapshot_value =
        call_on_socket(&mut socket, 2, method::SYSTEM_SNAPSHOT, None, &mut buffered).await?;
    let snapshot = serde_json::from_value::<Snapshot>(snapshot_value)
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    if !matches!(snapshot.body, SnapshotBody::Agent(_)) {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    Ok((socket, snapshot, buffered))
}

/// 连接正常结束的原因；`Err(())` 表示断线（需重连）。
enum SessionEnd {
    /// 本进程已丢弃客户端（命令通道关闭）。
    ClientDropped,
    /// agent 以 `service-quit` 关闭：完全退出，不再重连。
    ServiceQuit,
}

async fn run_connected(
    mut socket: Socket,
    commands: &mut mpsc::Receiver<ClientCommand>,
    events: &mpsc::Sender<AgentClientEvent>,
    snapshot_cursor: (String, u64),
    buffered: Vec<EventFrame>,
    mut preference_writes: PreferenceWrites,
    submissions: &mut SubmissionTransport,
) -> Result<SessionEnd, ()> {
    let mut next_id = 10_i64;
    let mut pending = HashMap::<i64, PendingCommand>::new();
    let mut cursor = snapshot_cursor;
    for mut frame in buffered {
        if frame.epoch == cursor.0 {
            preference_writes.overlay_frame(&mut frame);
            forward_event(frame, &mut cursor, events).await?;
        }
    }
    loop {
        tokio::select! {
            admission = submissions.gate.admit(), if !submissions.unsent.is_empty() => {
                let admission = admission.map_err(|error| {
                    log::error!("submission transport admission failed: {:?}", error.code);
                })?;
                let Some(command) = submissions.unsent.pop_front() else { continue; };
                let id = next_id;
                next_id = next_id.saturating_add(1);
                if send_command(&mut socket, &mut pending, &mut preference_writes, id, command, Some(admission)).await.is_err() {
                    break;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return Ok(SessionEnd::ClientDropped); };
                if command.is_submission() {
                    submissions.unsent.push_back(command);
                    continue;
                }
                let id = next_id;
                next_id = next_id.saturating_add(1);
                if send_command(&mut socket, &mut pending, &mut preference_writes, id, command, None).await.is_err() {
                    break;
                }
            }
            incoming = socket.next() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(Some(frame))))
                        if frame.reason.as_str() == CLOSE_REASON_SERVICE_QUIT =>
                    {
                        for (_, command) in pending.drain() {
                            if command.ack.send(Err(unavailable_error())).is_err() {
                                log::trace!("agent request receiver released before service shutdown");
                            }
                        }
                        return Ok(SessionEnd::ServiceQuit);
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                    _ => break,
                };
                if let Ok(notification) = serde_json::from_str::<RpcNotification>(&text)
                    && notification.method == method::SERVICE_EVENT
                {
                    let Some(params) = notification.params else { break; };
                    let Ok(mut frame) = serde_json::from_value::<EventFrame>(params) else { break; };
                    preference_writes.overlay_frame(&mut frame);
                    forward_event(frame, &mut cursor, events).await?;
                    continue;
                }
                let response = serde_json::from_str::<RpcResponse>(&text).map_err(|_| ())?;
                match response {
                    RpcResponse::Success(success) => {
                        if let RequestId::Integer(id) = success.id {
                            preference_writes.settle(id, Some(&success.result));
                            if let Some(command) = pending.remove(&id)
                                && command.ack.send(Ok(success.result)).is_err()
                            {
                                log::trace!("agent request receiver released before success reply");
                            }
                        }
                    }
                    RpcResponse::Failure(failure) => {
                        if let Some(RequestId::Integer(id)) = failure.id {
                            preference_writes.settle(id, None);
                            if let Some(command) = pending.remove(&id)
                                && command.ack.send(Err(failure.error.data.unwrap_or_else(internal_error))).is_err()
                            {
                                log::trace!("agent request receiver released before failure reply");
                            }
                        }
                    }
                }
            }
        }
    }
    for (_, command) in pending {
        if command.ack.send(Err(unavailable_error())).is_err() {
            log::trace!("agent request response receiver already released");
        }
    }
    Err(())
}

async fn send_command(
    socket: &mut Socket,
    pending: &mut HashMap<i64, PendingCommand>,
    preference_writes: &mut PreferenceWrites,
    id: i64,
    command: ClientCommand,
    admission: Option<tokio::sync::OwnedMutexGuard<()>>,
) -> Result<(), ()> {
    preference_writes.stage(id, &command.method, command.params.as_ref());
    let request = RpcRequest::new(RequestId::Integer(id), command.method, command.params);
    let text = serde_json::to_string(&request).map_err(|error| {
        log::error!("could not encode agent command: {error}");
    })?;
    pending.insert(
        id,
        PendingCommand {
            ack: command.ack,
            _admission: admission,
        },
    );
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| {
            log::debug!("agent command send failed: {error}");
        })
}

async fn forward_event(
    frame: EventFrame,
    cursor: &mut (String, u64),
    events: &mpsc::Sender<AgentClientEvent>,
) -> Result<(), ()> {
    if frame.epoch != cursor.0 {
        return Err(());
    }
    if frame.sequence <= cursor.1 {
        return Ok(());
    }
    if frame.sequence != cursor.1.saturating_add(1) {
        return Err(());
    }
    cursor.1 = frame.sequence;
    events
        .send(AgentClientEvent::Event(Box::new(frame)))
        .await
        .map_err(|_| ())
}

async fn call_on_socket(
    socket: &mut Socket,
    id: i64,
    method_name: &str,
    params: Option<Value>,
    buffered: &mut Vec<EventFrame>,
) -> Result<Value, ConnectError> {
    let request = RpcRequest::new(RequestId::Integer(id), method_name, params);
    let text = serde_json::to_string(&request)
        .map_err(|error| ConnectError::Transient(format!("encode {method_name}: {error}")))?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| ConnectError::Transient(format!("send {method_name}: {error}")))?;
    while let Some(message) = socket.next().await {
        let message = message
            .map_err(|error| ConnectError::Transient(format!("receive {method_name}: {error}")))?;
        let Message::Text(text) = message else {
            continue;
        };
        if let Ok(notification) = serde_json::from_str::<RpcNotification>(&text)
            && notification.method == method::SERVICE_EVENT
        {
            let Some(params) = notification.params else {
                return Err(ConnectError::Fatal(protocol_error()));
            };
            let frame = serde_json::from_value::<EventFrame>(params)
                .map_err(|_| ConnectError::Fatal(protocol_error()))?;
            if buffered.len() >= 1024 {
                return Err(ConnectError::Transient(format!(
                    "event backlog overflow during {method_name}"
                )));
            }
            buffered.push(frame);
            continue;
        }
        let response = serde_json::from_str::<RpcResponse>(&text).map_err(|_| {
            ConnectError::Transient(format!("unexpected frame during {method_name}"))
        })?;
        match response {
            RpcResponse::Success(success) if success.id == RequestId::Integer(id) => {
                return Ok(success.result);
            }
            RpcResponse::Failure(failure) if failure.id == Some(RequestId::Integer(id)) => {
                return Err(ConnectError::Fatal(
                    failure.error.data.unwrap_or_else(internal_error),
                ));
            }
            _ => {}
        }
    }
    Err(ConnectError::Transient(format!(
        "connection closed during {method_name}"
    )))
}

fn classify_connect_error(error: tokio_tungstenite::tungstenite::Error) -> ConnectError {
    match &error {
        tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 401 => {
            ConnectError::Fatal(RpcErrorData::new(ApplicationErrorCode::Unauthorized, false))
        }
        _ => ConnectError::Transient(format!("handshake: {error}")),
    }
}

enum ConnectError {
    /// 本机还没有可用 bearer：agent 未启动，或已监听但仍在初始化。
    NoBearer,
    /// 连接 agent 端口被拒：此刻确定无人监听。
    Refused,
    /// 握手 / 传输失败，携带诊断原因。
    Transient(String),
    /// 对端协议版本不兼容：可尝试让旧 agent 退出后由 bootstrap 拉起同级新版本。
    Incompatible,
    Fatal(RpcErrorData),
}

impl ConnectError {
    /// 连接诊断日志用的原因描述。
    fn describe(&self) -> String {
        match self {
            Self::NoBearer => {
                "no bearer token yet (agent not started or still initializing)".to_owned()
            }
            Self::Refused => "connection refused (agent not listening)".to_owned(),
            Self::Transient(reason) => reason.clone(),
            Self::Incompatible => "incompatible protocol version".to_owned(),
            Self::Fatal(error) => format!("fatal: {:?}", error.code),
        }
    }
}

fn validate_url(url: &str) -> Result<(), AgentClientError> {
    crate::agent_endpoint::socket_target(url)
        .map(|_| ())
        .map_err(|error| AgentClientError::Configuration(error.to_string()))
}

fn unavailable_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Unavailable, true)
}

fn internal_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Internal, false)
}

fn protocol_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::ProtocolIncompatible, false)
}

#[derive(Debug, thiserror::Error)]
pub enum AgentClientError {
    #[error("agent client configuration is invalid: {0}")]
    Configuration(String),
    #[error("could not create agent client runtime: {0}")]
    Runtime(std::io::Error),
}

#[cfg(test)]
#[path = "agent_client_submission_tests.rs"]
mod submission_tests;
