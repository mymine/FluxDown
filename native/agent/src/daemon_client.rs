//! agent 到 daemon 的单连接 JSON-RPC 客户端与重连快照恢复。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use fluxdown_protocol::handshake::{
    AuthChallengeParams, AuthChallengeResult, AuthProveParams, SYSTEM_AUTH_CHALLENGE,
    SYSTEM_AUTH_PROVE, client_proof, http_credential, verify_server_proof,
};
use fluxdown_protocol::method;
use fluxdown_protocol::{
    ApplicationErrorCode, EventFrame, METHOD_NOT_FOUND_CODE, RequestId, RpcErrorData,
    RpcErrorObject, RpcNotification, RpcRequest, RpcResponse, ServiceHello, ServiceRole, Snapshot,
    SnapshotBody,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::supervisor::DaemonSupervisor;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// daemon 客户端配置。
#[derive(Clone)]
pub struct DaemonClientConfig {
    pub rpc_url: String,
    /// `daemon.token` 的内容：只用于握手 MAC 的密钥，永不发送，也不当作 HTTP 凭据。
    token: Arc<str>,
    http: HttpSession,
}

impl DaemonClientConfig {
    #[must_use]
    pub fn new(rpc_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            token: Arc::from(token.into()),
            http: HttpSession::default(),
        }
    }

    /// 与本配置的 daemon 连接绑定的 HTTP 凭据句柄（`/blobs`、`/files`、`/exports` 用）。
    #[must_use]
    pub fn http_session(&self) -> HttpSession {
        self.http.clone()
    }

    /// 拒绝非 loopback daemon URL。
    pub fn validate(&self) -> Result<(), DaemonClientError> {
        let url = reqwest::Url::parse(&self.rpc_url)
            .map_err(|error| DaemonClientError::Configuration(error.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| DaemonClientError::Configuration("daemon URL has no host".to_owned()))?;
        let loopback = host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if loopback {
            Ok(())
        } else {
            Err(DaemonClientError::Configuration(
                "daemon URL must be loopback".to_owned(),
            ))
        }
    }
}

/// daemon 专用 HTTP 端点使用的会话级凭据。
///
/// 只在 WebSocket 握手证实对端持有 `daemon.token` 之后才存在，随连接断开而撤销；
/// 没有会话时为 `None`，调用方必须当作 daemon 不可用，而不是退回长期 token。
#[derive(Clone, Default)]
pub struct HttpSession {
    credential: Arc<RwLock<Option<String>>>,
}

impl HttpSession {
    /// 当前会话的 HTTP 凭据。
    #[must_use]
    pub fn credential(&self) -> Option<String> {
        self.credential
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn install(&self, credential: String) {
        *self
            .credential
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(credential);
    }

    fn clear(&self) {
        *self
            .credential
            .write()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// daemon 连接产生的有序状态流。
pub enum DaemonClientEvent {
    Snapshot(Snapshot),
    Event(Box<EventFrame>),
    Stale,
    Fatal(RpcErrorData),
}

struct ClientCommand {
    method: String,
    params: Option<Value>,
    ack: oneshot::Sender<Result<Value, RpcErrorObject>>,
}

/// 首次连上 daemon 之前（agent 冷启动，Gateway 已先行服务）的调用等待上限，与 agent 启动
/// 就绪预算一致：期间到达的捕获 / 兼容 API / 迁移等调用等 daemon 就绪后送达，而不是直接失败。
const STARTUP_CALL_WAIT: Duration = Duration::from_secs(30);

/// 可克隆的 daemon 调用入口。
#[derive(Clone)]
pub struct DaemonClient {
    commands: mpsc::Sender<ClientCommand>,
    connected: Arc<AtomicBool>,
    /// 首次连接已有结论（连上过，或重连任务已终止）：此后断线期间的调用立即失败。
    settled: Arc<AtomicBool>,
    ready: Arc<Notify>,
    cancel: CancellationToken,
}

impl DaemonClient {
    /// 启动重连任务与有界事件流；`cancel` 触发后不再为首次连接等待。
    pub fn start(
        config: DaemonClientConfig,
        supervisor: Arc<DaemonSupervisor>,
        cancel: CancellationToken,
    ) -> Result<(Self, mpsc::Receiver<DaemonClientEvent>), DaemonClientError> {
        Self::spawn(config, supervisor, cancel, false)
    }

    /// 嵌入式宿主：与 [`Self::start`] 相同，但 `cancel` 触发时重连任务一并结束（进程不随
    /// agent 退出，不能留下继续重连的后台任务）。
    pub(crate) fn start_scoped(
        config: DaemonClientConfig,
        supervisor: Arc<DaemonSupervisor>,
        cancel: CancellationToken,
    ) -> Result<(Self, mpsc::Receiver<DaemonClientEvent>), DaemonClientError> {
        Self::spawn(config, supervisor, cancel, true)
    }

    fn spawn(
        config: DaemonClientConfig,
        supervisor: Arc<DaemonSupervisor>,
        cancel: CancellationToken,
        stop_on_cancel: bool,
    ) -> Result<(Self, mpsc::Receiver<DaemonClientEvent>), DaemonClientError> {
        config.validate()?;
        let (commands, command_rx) = mpsc::channel(64);
        let (events, event_rx) = mpsc::channel(1024);
        let connected = Arc::new(AtomicBool::new(false));
        let settled = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(Notify::new());
        let http = config.http.clone();
        let client = run_client(
            config,
            supervisor,
            command_rx,
            events,
            connected.clone(),
            SettleOnExit {
                settled: settled.clone(),
                ready: ready.clone(),
            },
        );
        if stop_on_cancel {
            let stop = cancel.clone();
            let connected = connected.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = stop.cancelled() => {
                        connected.store(false, Ordering::Release);
                        http.clear();
                    }
                    () = client => {}
                }
            });
        } else {
            tokio::spawn(client);
        }
        Ok((
            Self {
                commands,
                connected,
                settled,
                ready,
                cancel,
            },
            event_rx,
        ))
    }

    /// 当前是否已连上 daemon（不等待）。
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// 提交类型化 RPC 调用。首次连上之前等待就绪（有上限）；之后断线期间立即失败。
    /// 错误只保留稳定应用错误详情；需要 daemon 错误消息（稳定错误码字符串）的调用用
    /// [`Self::call_detailed`]。
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<R, RpcErrorData> {
        self.call_detailed(method, params)
            .await
            .map_err(|error| error.data.unwrap_or_else(internal_error))
    }

    /// 同 [`Self::call`]，但失败时保留 daemon 返回的完整 JSON-RPC 错误对象（含 `message`）。
    pub async fn call_detailed<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<R, RpcErrorObject> {
        if !self.is_connected() && !self.settled.load(Ordering::Acquire) {
            tokio::select! {
                _ = self.cancel.cancelled() => {}
                _ = self.wait_until(STARTUP_CALL_WAIT, || {
                    self.is_connected() || self.settled.load(Ordering::Acquire)
                }) => {}
            }
        }
        if !self.is_connected() {
            return Err(unavailable_object());
        }
        let params = match params {
            Some(params) => Some(
                serde_json::to_value(params)
                    .map_err(|_| RpcErrorObject::application("invalid params", internal_error()))?,
            ),
            None => None,
        };
        let (ack, response) = oneshot::channel();
        self.commands
            .send(ClientCommand {
                method: method.to_owned(),
                params,
                ack,
            })
            .await
            .map_err(|_| unavailable_object())?;
        let value = response.await.map_err(|_| unavailable_object())??;
        serde_json::from_value(value)
            .map_err(|_| RpcErrorObject::application("invalid daemon result", internal_error()))
    }

    pub async fn wait_ready(&self, timeout: Duration) -> Result<(), RpcErrorData> {
        self.wait_until(timeout, || self.is_connected()).await
    }

    /// 等 `done` 成立；先登记唤醒再检查条件，连接恰在检查与等待之间建立也不会漏掉通知。
    async fn wait_until(
        &self,
        timeout: Duration,
        done: impl Fn() -> bool,
    ) -> Result<(), RpcErrorData> {
        tokio::time::timeout(timeout, async {
            loop {
                let notified = self.ready.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if done() {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| unavailable_error())
    }
}

/// 重连任务退出（协议不兼容 / 致命错误）时结束首次连接等待，调用方不再空等到上限。
struct SettleOnExit {
    settled: Arc<AtomicBool>,
    ready: Arc<Notify>,
}

impl SettleOnExit {
    fn settle(&self) {
        self.settled.store(true, Ordering::Release);
        self.ready.notify_waiters();
    }
}

impl Drop for SettleOnExit {
    fn drop(&mut self) {
        self.settle();
    }
}

/// 测试用记录客户端收到的 `(method, params)` 序列。
#[cfg(test)]
pub(crate) type RecordedCalls = Arc<tokio::sync::Mutex<Vec<(String, Option<Value>)>>>;

#[cfg(test)]
impl DaemonClient {
    /// 已断线（首次连接已有结论）的客户端：调用立即失败。
    pub(crate) fn disconnected() -> Self {
        let (commands, receiver) = mpsc::channel(1);
        drop(receiver);
        Self {
            commands,
            connected: Arc::new(AtomicBool::new(false)),
            settled: Arc::new(AtomicBool::new(true)),
            ready: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
        }
    }

    /// 已连接的客户端：记录每次调用的方法与参数并回 `{}`，供断言 agent 发给 daemon 的命令。
    pub(crate) fn recording() -> (Self, RecordedCalls) {
        let (commands, mut receiver) = mpsc::channel::<ClientCommand>(16);
        let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let sink = calls.clone();
        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                sink.lock().await.push((command.method, command.params));
                if command
                    .ack
                    .send(Ok(Value::Object(serde_json::Map::new())))
                    .is_err()
                {
                    tracing::trace!("recorded daemon caller dropped its response receiver");
                }
            }
        });
        let client = Self {
            commands,
            connected: Arc::new(AtomicBool::new(true)),
            settled: Arc::new(AtomicBool::new(true)),
            ready: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
        };
        (client, calls)
    }

    /// 已连接的客户端：每次调用都按 `reply` 现算应答，供断言 agent 对 daemon 应答的处理。
    pub(crate) fn answering<F>(reply: F) -> Self
    where
        F: Fn(&str) -> Result<Value, RpcErrorObject> + Send + 'static,
    {
        let (commands, mut receiver) = mpsc::channel::<ClientCommand>(16);
        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                if command.ack.send(reply(&command.method)).is_err() {
                    tracing::trace!("scripted daemon caller dropped its response receiver");
                }
            }
        });
        Self {
            commands,
            connected: Arc::new(AtomicBool::new(true)),
            settled: Arc::new(AtomicBool::new(true)),
            ready: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
        }
    }
}

/// 本进程刚拉起、仍存活但尚未监听的 daemon 按短间隔轮询（约 10s）：冷启动 daemon 通常
/// 数百毫秒内就绪，若按 1s 起步的退避等待，整条启动链（界面首个快照）会被白白拖长约 1s。
/// 只对同一个子进程代际生效；子进程退出后被重新拉起则回到指数退避，避免崩溃循环高频拉起。
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);
const STARTUP_POLL_ATTEMPTS: usize = 200;
/// daemon 启动即崩溃循环时的重拉间隔：避免每 ≤30s 完整初始化一次引擎。
const CRASH_LOOP_RETRY_SECS: u64 = 120;

/// 握手阶段的请求 ID（连接建立后的调用从 10 起）。
const AUTH_CHALLENGE_REQUEST_ID: i64 = 1;
const AUTH_PROVE_REQUEST_ID: i64 = 2;
const HELLO_REQUEST_ID: i64 = 3;
const SNAPSHOT_REQUEST_ID: i64 = 4;
/// 已认证后、`system.hello` 之前的 `system.shutdown`。
const SHUTDOWN_REQUEST_ID: i64 = 3;

async fn run_client(
    config: DaemonClientConfig,
    supervisor: Arc<DaemonSupervisor>,
    mut commands: mpsc::Receiver<ClientCommand>,
    events: mpsc::Sender<DaemonClientEvent>,
    connected: Arc<AtomicBool>,
    settle: SettleOnExit,
) {
    let backoff = [1_u64, 2, 5, 15, 30];
    let mut attempt = 0_usize;
    // 本轮断连中快速轮询的 daemon 子进程代际与已用次数；连上即清零。
    let mut polled_child: Option<u64> = None;
    let mut startup_polls = 0_usize;
    // 每个 agent 进程只尝试替换一次协议不兼容的 daemon，避免同目录二进制错配时反复互杀。
    let mut replaced_incompatible = false;
    // 被旧版 daemon（或冒充者）占着端口时已尝试结束同名进程的次数，见 `LEGACY_REPLACE_ATTEMPTS`。
    let mut legacy_attempts = 0_u32;
    loop {
        match connect(&config).await {
            Ok((socket, snapshot, buffered)) => {
                attempt = 0;
                supervisor.clear_crash_streak();
                polled_child = None;
                startup_polls = 0;
                connected.store(true, Ordering::Release);
                settle.settle();
                let snapshot_cursor = (snapshot.epoch.clone(), snapshot.sequence);
                if events
                    .send(DaemonClientEvent::Snapshot(snapshot))
                    .await
                    .is_err()
                {
                    connected.store(false, Ordering::Release);
                    config.http.clear();
                    return;
                }
                if run_connected(socket, &mut commands, &events, snapshot_cursor, buffered)
                    .await
                    .is_err()
                    && events.send(DaemonClientEvent::Stale).await.is_err()
                {
                    tracing::debug!("daemon event consumer closed during disconnect");
                    connected.store(false, Ordering::Release);
                    config.http.clear();
                    fail_queued_commands(&mut commands);
                    return;
                }
                connected.store(false, Ordering::Release);
                config.http.clear();
                fail_queued_commands(&mut commands);
            }
            Err(ConnectError::Refused) => match supervisor.ensure_running().await {
                Ok(Some(child))
                    if startup_polls < STARTUP_POLL_ATTEMPTS
                        && *polled_child.get_or_insert(child) == child =>
                {
                    startup_polls += 1;
                    tokio::time::sleep(STARTUP_POLL_INTERVAL).await;
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "could not supervise fluxdownd");
                }
            },
            Err(ConnectError::Incompatible) if !replaced_incompatible => {
                replaced_incompatible = true;
                match request_shutdown(&config).await {
                    Ok(()) => {
                        tracing::info!("asked protocol-incompatible fluxdownd to exit");
                        wait_until_stopped(&config, Duration::from_secs(30)).await;
                        attempt = 0;
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "protocol-incompatible fluxdownd refused shutdown");
                        if events
                            .send(DaemonClientEvent::Fatal(protocol_error()))
                            .await
                            .is_err()
                        {
                            tracing::debug!(
                                "daemon event consumer closed before fatal protocol error"
                            );
                        }
                        connected.store(false, Ordering::Release);
                        return;
                    }
                }
            }
            Err(ConnectError::Incompatible) => {
                if events
                    .send(DaemonClientEvent::Fatal(protocol_error()))
                    .await
                    .is_err()
                {
                    tracing::debug!("daemon event consumer closed before fatal protocol error");
                }
                connected.store(false, Ordering::Release);
                return;
            }
            Err(ConnectError::LegacyPeer) => match next_legacy_step(legacy_attempts) {
                LegacyStep::Replace => {
                    legacy_attempts += 1;
                    tracing::warn!(
                        attempt = legacy_attempts,
                        "the process on the daemon port does not speak the challenge-response handshake (legacy fluxdownd); no credentials are sent to it, stopping the stale fluxdownd instead"
                    );
                    let stopped = supervisor.terminate_stale_daemon().await;
                    if stopped > 0 {
                        tracing::info!(
                            stopped,
                            "signalled stale fluxdownd; waiting for it to release the port"
                        );
                        wait_until_stopped(&config, Duration::from_secs(30)).await;
                        attempt = 0;
                        continue;
                    }
                    tracing::warn!(
                        "no stale fluxdownd process could be identified; retrying with backoff"
                    );
                }
                LegacyStep::GiveUp => {
                    tracing::error!(
                        attempts = legacy_attempts,
                        "the daemon port is still held by a process that does not speak the handshake; stop the old fluxdownd manually and restart FluxDown"
                    );
                    if events
                        .send(DaemonClientEvent::Fatal(protocol_error()))
                        .await
                        .is_err()
                    {
                        tracing::debug!("daemon event consumer closed before legacy-peer failure");
                    }
                    connected.store(false, Ordering::Release);
                    return;
                }
            },
            Err(ConnectError::Fatal(error)) => {
                if events.send(DaemonClientEvent::Fatal(error)).await.is_err() {
                    tracing::debug!("daemon event consumer closed before fatal connection error");
                }
                connected.store(false, Ordering::Release);
                return;
            }
            Err(ConnectError::Transient(error)) => {
                tracing::warn!(%error, "daemon connection failed");
            }
        }
        connected.store(false, Ordering::Release);
        let mut delay = backoff[attempt.min(backoff.len() - 1)];
        if supervisor.in_crash_loop() {
            delay = CRASH_LOOP_RETRY_SECS;
            tracing::error!(
                retry_secs = delay,
                "fluxdownd keeps exiting right after launch (port 17801 unavailable or startup failure?); see fluxdownd.stderr.log"
            );
        }
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

fn fail_queued_commands(commands: &mut mpsc::Receiver<ClientCommand>) {
    while let Ok(command) = commands.try_recv() {
        if command.ack.send(Err(unavailable_object())).is_err() {
            tracing::trace!("queued daemon caller dropped its response receiver");
        }
    }
}

async fn open_socket(config: &DaemonClientConfig) -> Result<Socket, ConnectError> {
    // 升级请求不带任何凭据：对端在证明持有 token 之前，agent 不交出任何可复用的东西。
    let request = config
        .rpc_url
        .clone()
        .into_client_request()
        .map_err(|error| ConnectError::Fatal(invalid_argument(error.to_string())))?;
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(classify_connect_error)?;
    Ok(socket)
}

/// 双向挑战应答：先校验 daemon 对 token 持有权的证明，通过后才提交自己的证明。
/// 返回本会话派生出的 HTTP 凭据（从不经 WebSocket 传输）。`token` 本身不会离开本进程。
async fn authenticate(
    socket: &mut Socket,
    token: &str,
    buffered: &mut Vec<EventFrame>,
) -> Result<String, ConnectError> {
    let client_nonce = fresh_nonce();
    let params = serde_json::to_value(AuthChallengeParams {
        client_nonce: client_nonce.clone(),
    })
    .map_err(|error| ConnectError::Transient(error.to_string()))?;
    let challenge = call_on_socket(
        socket,
        AUTH_CHALLENGE_REQUEST_ID,
        SYSTEM_AUTH_CHALLENGE,
        Some(params),
        buffered,
    )
    .await
    .map_err(|error| match error {
        ConnectError::Fatal(data) if is_unrecognized_handshake(&data) => ConnectError::LegacyPeer,
        other => other,
    })?;
    let challenge = serde_json::from_value::<AuthChallengeResult>(challenge)
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    if !verify_server_proof(
        token,
        &client_nonce,
        &challenge.server_nonce,
        &challenge.server_proof,
    ) {
        tracing::error!(
            "the process on the daemon port could not prove it holds daemon.token; refusing to authenticate"
        );
        return Err(ConnectError::Fatal(unauthorized_error()));
    }
    let (Some(proof), Some(credential)) = (
        client_proof(token, &client_nonce, &challenge.server_nonce),
        http_credential(token, &client_nonce, &challenge.server_nonce),
    ) else {
        return Err(ConnectError::Fatal(protocol_error()));
    };
    let params = serde_json::to_value(AuthProveParams {
        client_proof: proof,
    })
    .map_err(|error| ConnectError::Transient(error.to_string()))?;
    call_on_socket(
        socket,
        AUTH_PROVE_REQUEST_ID,
        SYSTEM_AUTH_PROVE,
        Some(params),
        buffered,
    )
    .await?;
    Ok(credential)
}

fn describe_connect_error(error: ConnectError) -> String {
    match error {
        ConnectError::Fatal(data) => format!("{:?}", data.code),
        ConnectError::Transient(message) => message,
        ConnectError::Refused | ConnectError::Incompatible => "daemon unreachable".to_owned(),
        ConnectError::LegacyPeer => "legacy daemon".to_owned(),
    }
}

/// 已认证后、`system.hello` 之前的 `system.shutdown`：daemon 只在握手通过后受理，
/// 协议版本不兼容的新旧进程替换与完全退出都走这里。
pub(crate) async fn request_shutdown(config: &DaemonClientConfig) -> Result<(), String> {
    let mut socket = open_socket(config)
        .await
        .map_err(|_| "daemon unreachable".to_owned())?;
    let mut buffered = Vec::new();
    authenticate(&mut socket, &config.token, &mut buffered)
        .await
        .map_err(describe_connect_error)?;
    call_on_socket(
        &mut socket,
        SHUTDOWN_REQUEST_ID,
        method::SYSTEM_SHUTDOWN,
        None,
        &mut buffered,
    )
    .await
    .map(|_| ())
    .map_err(describe_connect_error)
}

/// 等旧 daemon 关闭监听（连接被拒）；超时后交回重连循环自愈。
async fn wait_until_stopped(config: &DaemonClientConfig, timeout: Duration) {
    let Ok(url) = reqwest::Url::parse(&config.rpc_url) else {
        return;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return;
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match TcpStream::connect((host.as_str(), port)).await {
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => return,
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// 建立到 daemon 的已认证、已握手连接；任何一步失败都撤销会话 HTTP 凭据。
async fn connect(
    config: &DaemonClientConfig,
) -> Result<(Socket, Snapshot, Vec<EventFrame>), ConnectError> {
    let result = establish(config).await;
    if result.is_err() {
        config.http.clear();
    }
    result
}

async fn establish(
    config: &DaemonClientConfig,
) -> Result<(Socket, Snapshot, Vec<EventFrame>), ConnectError> {
    let mut socket = open_socket(config).await?;
    let mut buffered = Vec::new();
    let credential = authenticate(&mut socket, &config.token, &mut buffered).await?;
    config.http.install(credential);
    let hello = serde_json::json!({
        "clientName": "fluxdown-agent",
        "clientVersion": fluxdown_protocol::APP_VERSION,
        "minProtocolVersion": fluxdown_protocol::MIN_PROTOCOL_VERSION,
        "maxProtocolVersion": fluxdown_protocol::PROTOCOL_VERSION,
        "requestedRole": "daemon",
        "capabilities": []
    });
    let hello_value = call_on_socket(
        &mut socket,
        HELLO_REQUEST_ID,
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
    if service.role != ServiceRole::Daemon {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    if service.protocol_version != fluxdown_protocol::PROTOCOL_VERSION {
        return Err(ConnectError::Incompatible);
    }
    let snapshot_value = call_on_socket(
        &mut socket,
        SNAPSHOT_REQUEST_ID,
        method::SYSTEM_SNAPSHOT,
        None,
        &mut buffered,
    )
    .await?;
    let snapshot = serde_json::from_value::<Snapshot>(snapshot_value)
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    if !matches!(snapshot.body, SnapshotBody::Daemon(_)) {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    Ok((socket, snapshot, buffered))
}

async fn run_connected(
    mut socket: Socket,
    commands: &mut mpsc::Receiver<ClientCommand>,
    events: &mpsc::Sender<DaemonClientEvent>,
    snapshot_cursor: (String, u64),
    buffered: Vec<EventFrame>,
) -> Result<(), ()> {
    let mut next_id = 10_i64;
    let mut pending = HashMap::<i64, oneshot::Sender<Result<Value, RpcErrorObject>>>::new();
    let mut cursor = snapshot_cursor;
    for frame in buffered {
        if frame.epoch == cursor.0 {
            forward_event(frame, &mut cursor, events).await?;
        }
    }
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()); };
                let id = next_id;
                next_id = next_id.saturating_add(1);
                let request = RpcRequest::new(RequestId::Integer(id), command.method, command.params);
                let text = serde_json::to_string(&request).map_err(|_| ())?;
                pending.insert(id, command.ack);
                if socket.send(Message::Text(text.into())).await.is_err() { break; }
            }
            incoming = socket.next() => {
                let Some(Ok(Message::Text(text))) = incoming else { break; };
                if let Ok(notification) = serde_json::from_str::<RpcNotification>(&text)
                    && notification.method == method::SERVICE_EVENT
                {
                    let Some(params) = notification.params else { break; };
                    let Ok(frame) = serde_json::from_value::<EventFrame>(params) else { break; };
                    forward_event(frame, &mut cursor, events).await?;
                    continue;
                }
                let Ok(response) = serde_json::from_str::<RpcResponse>(&text) else { break; };
                match response {
                    RpcResponse::Success(success) => {
                        if let RequestId::Integer(id) = success.id
                            && let Some(ack) = pending.remove(&id)
                            && ack.send(Ok(success.result)).is_err() {
                                tracing::trace!(id, "daemon caller dropped its successful response receiver");
                            }
                    }
                    RpcResponse::Failure(failure) => {
                        if let Some(RequestId::Integer(id)) = failure.id
                            && let Some(ack) = pending.remove(&id)
                            && ack.send(Err(failure.error)).is_err() {
                                tracing::trace!(id, "daemon caller dropped its error response receiver");
                            }
                    }
                }
            }
        }
    }
    for (id, ack) in pending {
        if ack.send(Err(unavailable_object())).is_err() {
            tracing::trace!(
                id,
                "pending daemon caller dropped its response receiver during disconnect"
            );
        }
    }
    Err(())
}

/// 快照以前的通知可跳过；快照以后的缺口或 epoch 更换必须重新握手。
async fn forward_event(
    frame: EventFrame,
    cursor: &mut (String, u64),
    events: &mpsc::Sender<DaemonClientEvent>,
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
        .send(DaemonClientEvent::Event(Box::new(frame)))
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
        .map_err(|error| ConnectError::Transient(error.to_string()))?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| ConnectError::Transient(error.to_string()))?;
    while let Some(message) = socket.next().await {
        let message = message.map_err(|error| ConnectError::Transient(error.to_string()))?;
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
                return Err(ConnectError::Transient(
                    "handshake event backlog".to_owned(),
                ));
            }
            buffered.push(frame);
            continue;
        }
        let response = serde_json::from_str::<RpcResponse>(&text)
            .map_err(|error| ConnectError::Transient(error.to_string()))?;
        match response {
            RpcResponse::Success(success) if success.id == RequestId::Integer(id) => {
                return Ok(success.result);
            }
            RpcResponse::Failure(failure) if failure.id == Some(RequestId::Integer(id)) => {
                // method-not-found 不带应用错误详情：归一为 `Unsupported`，调用方据此识别
                // 「对端不认识这个方法」。
                let data = failure.error.data.unwrap_or_else(|| {
                    if failure.error.code == METHOD_NOT_FOUND_CODE {
                        RpcErrorData::new(ApplicationErrorCode::Unsupported, false)
                    } else {
                        internal_error()
                    }
                });
                return Err(ConnectError::Fatal(data));
            }
            _ => {}
        }
    }
    Err(ConnectError::Transient("daemon socket closed".to_owned()))
}

fn classify_connect_error(error: tokio_tungstenite::tungstenite::Error) -> ConnectError {
    match &error {
        tokio_tungstenite::tungstenite::Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            ConnectError::Refused
        }
        // agent 的升级请求不带任何凭据：只有仍要求升级头 Bearer 的旧版 daemon（或根本不是
        // fluxdownd 的进程）会回 401。无论哪种，都不会拿到 token。
        tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 401 => {
            ConnectError::LegacyPeer
        }
        _ => ConnectError::Transient(format!("{error:#}")),
    }
}

/// 对端是否不认识挑战方法：旧版 daemon 的首帧门禁只认 `system.hello`，对其它方法回
/// 「`method` 字段无效」；更旧 / 其它实现回 method-not-found（已归一为 `Unsupported`）。
fn is_unrecognized_handshake(data: &RpcErrorData) -> bool {
    data.code == ApplicationErrorCode::Unsupported
        || (data.code == ApplicationErrorCode::InvalidArgument
            && data.field.as_deref() == Some("method"))
}

/// 每个 agent 进程最多尝试结束旧 daemon 的次数：之后仍被旧 daemon（或冒充者）占着端口就
/// 停止并报协议不兼容，避免反复互杀。
const LEGACY_REPLACE_ATTEMPTS: u32 = 3;

#[derive(Debug, PartialEq, Eq)]
enum LegacyStep {
    /// 结束占着端口的旧 daemon，随后由监管拉起同版本 daemon。
    Replace,
    GiveUp,
}

fn next_legacy_step(attempts_so_far: u32) -> LegacyStep {
    if attempts_so_far < LEGACY_REPLACE_ATTEMPTS {
        LegacyStep::Replace
    } else {
        LegacyStep::GiveUp
    }
}

enum ConnectError {
    Refused,
    Transient(String),
    /// 对端协议版本不兼容：可尝试让旧进程退出后由监管拉起同版本 daemon。
    Incompatible,
    /// 对端是不认识挑战应答握手的旧版 daemon（或冒充者）：不得向它发送 token，只能结束
    /// 同名旧进程后由监管拉起新 daemon。
    LegacyPeer,
    Fatal(RpcErrorData),
}

/// daemon 客户端启动错误。
#[derive(Debug, thiserror::Error)]
pub enum DaemonClientError {
    #[error("daemon client configuration is invalid: {0}")]
    Configuration(String),
}

fn unavailable_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Unavailable, true)
}

fn unavailable_object() -> RpcErrorObject {
    RpcErrorObject::application("daemon unavailable", unavailable_error())
}

fn internal_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Internal, false)
}

fn protocol_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::ProtocolIncompatible, false)
}

fn unauthorized_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Unauthorized, false)
}

/// 每次握手一组新的 256 位随机数（两个 v4 UUID 的 244 位随机部分，十六进制 64 字符）。
fn fresh_nonce() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn invalid_argument(_message: String) -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fluxdown_protocol::handshake::{
        AuthChallengeParams, AuthChallengeResult, AuthProveParams, SYSTEM_AUTH_CHALLENGE,
        SYSTEM_AUTH_PROVE, http_credential, server_proof, verify_client_proof,
    };
    use fluxdown_protocol::{
        ApplicationErrorCode, METHOD_NOT_FOUND_CODE, RpcErrorData, RpcErrorObject, RpcRequest,
        RpcResponse, method,
    };
    use serde_json::json;
    use tokio_tungstenite::tungstenite::Message;

    use super::DaemonClient;

    #[tokio::test]
    async fn disconnected_client_rejects_without_queueing_for_replay() {
        let client = DaemonClient::disconnected();
        let result = client
            .call::<serde_json::Value, serde_json::Value>(
                fluxdown_protocol::method::DAEMON_TASK_CREATE,
                Some(serde_json::json!({})),
            )
            .await;
        let error = result.expect_err("disconnected command must fail");
        assert_eq!(error.code, ApplicationErrorCode::Unavailable);
        assert!(error.retryable);
    }

    fn pending_client() -> (
        DaemonClient,
        tokio::sync::mpsc::Receiver<super::ClientCommand>,
        super::SettleOnExit,
    ) {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        let settled = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(tokio::sync::Notify::new());
        let client = DaemonClient {
            commands,
            connected: Arc::new(AtomicBool::new(false)),
            settled: settled.clone(),
            ready: ready.clone(),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        (client, receiver, super::SettleOnExit { settled, ready })
    }

    #[tokio::test]
    async fn call_before_first_connection_is_delivered_once_connected() {
        let (client, mut commands, settle) = pending_client();
        let caller = client.clone();
        let call = tokio::spawn(async move {
            caller
                .call::<serde_json::Value, serde_json::Value>("task.create", None)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !call.is_finished(),
            "call must wait for the first connection"
        );
        client
            .connected
            .store(true, std::sync::atomic::Ordering::Release);
        settle.settle();
        let command = commands.recv().await.expect("command delivered");
        assert_eq!(command.method, "task.create");
        command
            .ack
            .send(Ok(serde_json::json!({"ok": true})))
            .expect("waiting caller receives acknowledgement");
        let result = call.await.expect("join").expect("call succeeds");
        assert_eq!(result, serde_json::json!({"ok": true}));
    }

    #[tokio::test]
    async fn call_fails_fast_when_client_gives_up_or_agent_cancels() {
        let (client, _commands, settle) = pending_client();
        let caller = client.clone();
        let call = tokio::spawn(async move {
            caller
                .call::<serde_json::Value, serde_json::Value>("task.create", None)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(settle);
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), call)
            .await
            .expect("must not wait for the startup budget")
            .expect("join")
            .expect_err("never connected");
        assert_eq!(error.code, ApplicationErrorCode::Unavailable);

        let (client, _commands, _settle) = pending_client();
        client.cancel.cancel();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.call::<serde_json::Value, serde_json::Value>("task.create", None),
        )
        .await
        .expect("cancelled agent must not wait")
        .expect_err("never connected");
        assert_eq!(error.code, ApplicationErrorCode::Unavailable);
    }

    const TOKEN: &str = "agent-daemon-client-test-token";

    type ServerSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    async fn read_request(ws: &mut ServerSocket) -> RpcRequest {
        use futures_util::StreamExt;
        loop {
            let message = ws.next().await.expect("client frame").expect("frame ok");
            if let Message::Text(text) = message {
                return serde_json::from_str(&text).expect("json-rpc request");
            }
        }
    }

    async fn send_response(ws: &mut ServerSocket, response: RpcResponse) {
        use futures_util::SinkExt;
        ws.send(Message::Text(
            serde_json::to_string(&response).unwrap().into(),
        ))
        .await
        .unwrap();
    }

    fn fresh_server_nonce() -> String {
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    }

    /// daemon 一侧的握手：持有 `token`，先证明自己，再校验客户端。返回 (clientNonce, serverNonce)。
    async fn daemon_handshake(ws: &mut ServerSocket, token: &str) -> (String, String) {
        let challenge = read_request(ws).await;
        assert_eq!(challenge.method, SYSTEM_AUTH_CHALLENGE);
        let params: AuthChallengeParams =
            serde_json::from_value(challenge.params.expect("challenge params")).unwrap();
        let server_nonce = fresh_server_nonce();
        let proof = server_proof(token, &params.client_nonce, &server_nonce).expect("proof");
        send_response(
            ws,
            RpcResponse::success(
                challenge.id,
                serde_json::to_value(AuthChallengeResult {
                    server_nonce: server_nonce.clone(),
                    server_proof: proof,
                })
                .unwrap(),
            ),
        )
        .await;
        let prove = read_request(ws).await;
        assert_eq!(prove.method, SYSTEM_AUTH_PROVE);
        let proved: AuthProveParams =
            serde_json::from_value(prove.params.expect("prove params")).unwrap();
        assert!(verify_client_proof(
            token,
            &params.client_nonce,
            &server_nonce,
            &proved.client_proof
        ));
        send_response(
            ws,
            RpcResponse::success(prove.id, json!({ "authenticated": true })),
        )
        .await;
        (params.client_nonce, server_nonce)
    }

    async fn loopback_listener() -> (tokio::net::TcpListener, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/rpc", listener.local_addr().unwrap());
        (listener, url)
    }

    #[tokio::test]
    async fn handshake_buffers_notifications_filters_old_frames_and_reconnects_after_gap() {
        use fluxdown_protocol::{
            DaemonEvent, EventFrame, RpcNotification, ServiceEvent, ServiceHello, ServiceRole,
            Snapshot, SnapshotBody, TaskRuntimeDto,
        };
        use futures_util::SinkExt;

        fn notification(epoch: &str, sequence: u64) -> Message {
            let frame = EventFrame {
                epoch: epoch.into(),
                sequence,
                event: ServiceEvent::Daemon(DaemonEvent::TaskRuntimeChanged(TaskRuntimeDto {
                    task_id: "task".into(),
                    active_transfers: Some(sequence as u32),
                    ..Default::default()
                })),
            };
            let notification = RpcNotification::new(
                fluxdown_protocol::method::SERVICE_EVENT,
                Some(serde_json::to_value(frame).unwrap()),
            );
            Message::Text(serde_json::to_string(&notification).unwrap().into())
        }

        async fn serve_once(
            listener: &tokio::net::TcpListener,
            epoch: &str,
            sequence: u64,
            gap: bool,
        ) {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            daemon_handshake(&mut ws, TOKEN).await;
            let hello_request = read_request(&mut ws).await;
            assert_eq!(hello_request.method, method::SYSTEM_HELLO);
            if gap {
                ws.send(notification("previous", 3)).await.unwrap();
            }
            let hello =
                ServiceHello::new(ServiceRole::Daemon, "daemon", "test", "instance", vec![]);
            send_response(
                &mut ws,
                RpcResponse::success(hello_request.id, serde_json::to_value(hello).unwrap()),
            )
            .await;
            let snapshot_request = read_request(&mut ws).await;
            assert_eq!(snapshot_request.method, method::SYSTEM_SNAPSHOT);
            ws.send(notification(epoch, sequence.saturating_sub(1)))
                .await
                .unwrap();
            ws.send(notification(epoch, sequence + 1)).await.unwrap();
            let snapshot = Snapshot {
                epoch: epoch.into(),
                sequence,
                body: SnapshotBody::Daemon(Box::default()),
            };
            send_response(
                &mut ws,
                RpcResponse::success(snapshot_request.id, serde_json::to_value(snapshot).unwrap()),
            )
            .await;
            if gap {
                ws.send(notification(epoch, sequence + 2)).await.unwrap();
                ws.send(notification(epoch, sequence + 4)).await.unwrap();
            }
            ws.close(None).await.unwrap();
        }

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (listener, url) = loopback_listener().await;
            let server = tokio::spawn(async move {
                serve_once(&listener, "a", 5, true).await;
                serve_once(&listener, "b", 20, false).await;
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            let (tx, mut rx) = tokio::sync::mpsc::channel(16);
            let (command_tx, mut commands) = tokio::sync::mpsc::channel(1);
            let (socket, snapshot, buffered) = super::connect(&config)
                .await
                .unwrap_or_else(|_| panic!("connect"));
            assert!(
                config.http_session().credential().is_some(),
                "an authenticated session carries an HTTP credential"
            );
            assert_eq!((snapshot.epoch.as_str(), snapshot.sequence), ("a", 5));
            assert!(
                super::run_connected(
                    socket,
                    &mut commands,
                    &tx,
                    (snapshot.epoch, snapshot.sequence),
                    buffered
                )
                .await
                .is_err()
            );
            let first = rx.recv().await.expect("buffered next event");
            let second = rx.recv().await.expect("live next event");
            let [
                super::DaemonClientEvent::Event(first),
                super::DaemonClientEvent::Event(second),
            ] = [first, second]
            else {
                panic!("events")
            };
            assert_eq!((first.sequence, second.sequence), (6, 7));
            assert!(rx.try_recv().is_err(), "gap event must not be delivered");
            let (socket, snapshot, buffered) = super::connect(&config)
                .await
                .unwrap_or_else(|_| panic!("reconnect"));
            assert_eq!((snapshot.epoch.as_str(), snapshot.sequence), ("b", 20));
            super::run_connected(
                socket,
                &mut commands,
                &tx,
                (snapshot.epoch, snapshot.sequence),
                buffered,
            )
            .await
            .expect_err("daemon closes the recovered connection");
            let super::DaemonClientEvent::Event(recovered) =
                rx.recv().await.expect("recovered event")
            else {
                panic!("event")
            };
            assert_eq!((recovered.epoch.as_str(), recovered.sequence), ("b", 21));
            drop(command_tx);
            server.await.unwrap();
        })
        .await
        .expect("websocket handshake and recovery");
    }

    /// 插件设置保存失败时 daemon 以 `field` 标出出错的设置项；经 agent 转发给界面时必须原样保留。
    #[tokio::test]
    async fn daemon_error_keeps_field_and_reason_when_forwarded() {
        use fluxdown_protocol::{
            ErrorReason, RpcErrorData, ServiceHello, ServiceRole, Snapshot, SnapshotBody,
        };

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (listener, url) = loopback_listener().await;
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                daemon_handshake(&mut ws, TOKEN).await;
                let hello = read_request(&mut ws).await;
                let reply =
                    ServiceHello::new(ServiceRole::Daemon, "daemon", "test", "instance", vec![]);
                send_response(
                    &mut ws,
                    RpcResponse::success(hello.id, serde_json::to_value(reply).unwrap()),
                )
                .await;
                let snapshot_request = read_request(&mut ws).await;
                let snapshot = Snapshot {
                    epoch: "e".into(),
                    sequence: 1,
                    body: SnapshotBody::Daemon(Box::default()),
                };
                send_response(
                    &mut ws,
                    RpcResponse::success(
                        snapshot_request.id,
                        serde_json::to_value(snapshot).unwrap(),
                    ),
                )
                .await;
                let call = read_request(&mut ws).await;
                assert_eq!(call.method, method::DAEMON_PLUGIN_UPDATE_SETTINGS);
                send_response(
                    &mut ws,
                    RpcResponse::failure(
                        call.id,
                        RpcErrorObject::application(
                            "invalid setting",
                            RpcErrorData {
                                field: Some("maxItems".to_owned()),
                                reason: Some(ErrorReason::PluginPackageInvalid),
                                ..RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false)
                            },
                        ),
                    ),
                )
                .await;
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            let (socket, snapshot, buffered) = super::connect(&config)
                .await
                .unwrap_or_else(|_| panic!("connect"));
            let (tx, _rx) = tokio::sync::mpsc::channel(16);
            let (command_tx, mut commands) = tokio::sync::mpsc::channel(1);
            let runner = tokio::spawn(async move {
                super::run_connected(
                    socket,
                    &mut commands,
                    &tx,
                    (snapshot.epoch, snapshot.sequence),
                    buffered,
                )
                .await
            });
            let (ack, response) = tokio::sync::oneshot::channel();
            command_tx
                .send(super::ClientCommand {
                    method: method::DAEMON_PLUGIN_UPDATE_SETTINGS.to_owned(),
                    params: Some(json!({ "pluginId": "p", "entries": {} })),
                    ack,
                })
                .await
                .unwrap();
            let error = response.await.unwrap().unwrap_err();
            let data = error.data.expect("application error data");
            assert_eq!(data.code, ApplicationErrorCode::InvalidArgument);
            assert_eq!(data.field.as_deref(), Some("maxItems"));
            assert_eq!(data.reason, Some(ErrorReason::PluginPackageInvalid));
            runner
                .await
                .expect("join daemon connection")
                .expect_err("daemon closes after returning its error");
            drop(command_tx);
            server.await.unwrap();
        })
        .await
        .expect("daemon error forwarding");
    }

    #[tokio::test]
    async fn authentication_derives_the_session_credential_from_a_daemon_that_proves_the_token() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (listener, url) = loopback_listener().await;
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                daemon_handshake(&mut ws, TOKEN).await
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            let mut socket = super::open_socket(&config)
                .await
                .unwrap_or_else(|_| panic!("open"));
            let mut buffered = Vec::new();
            let credential = super::authenticate(&mut socket, TOKEN, &mut buffered)
                .await
                .unwrap_or_else(|_| panic!("handshake"));
            let (client_nonce, server_nonce) = server.await.unwrap();
            assert_eq!(
                Some(credential),
                http_credential(TOKEN, &client_nonce, &server_nonce)
            );
        })
        .await
        .expect("handshake completes");
    }

    #[tokio::test]
    // tungstenite 的升级回调签名固定返回 `Result<Response, ErrorResponse>`。
    #[allow(clippy::result_large_err)]
    async fn impostor_daemon_learns_no_credential_and_never_sees_the_token() {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let (listener, url) = loopback_listener().await;
            let upgrade_headers = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let frames = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let server = tokio::spawn({
                let upgrade_headers = upgrade_headers.clone();
                let frames = frames.clone();
                async move {
                    // 一次给 `connect`，一次给 `request_shutdown`：两条路径都不能漏凭据。
                    for _ in 0..2 {
                        let (tcp, _) = listener.accept().await.unwrap();
                        let headers = upgrade_headers.clone();
                        let mut ws = tokio_tungstenite::accept_hdr_async(
                            tcp,
                            move |request: &Request, response: Response| {
                                for (name, value) in request.headers() {
                                    headers.lock().unwrap().push(format!(
                                        "{name}: {}",
                                        value.to_str().unwrap_or_default()
                                    ));
                                }
                                Ok(response)
                            },
                        )
                        .await
                        .unwrap();
                        let challenge = read_request(&mut ws).await;
                        frames
                            .lock()
                            .unwrap()
                            .push(serde_json::to_string(&challenge).unwrap());
                        let params: AuthChallengeParams =
                            serde_json::from_value(challenge.params.expect("params")).unwrap();
                        // 冒充者不知道 token，只能拿猜测值算证明。
                        let server_nonce = fresh_server_nonce();
                        let forged = server_proof(
                            "a-guess-of-the-token",
                            &params.client_nonce,
                            &server_nonce,
                        )
                        .unwrap();
                        send_response(
                            &mut ws,
                            RpcResponse::success(
                                challenge.id,
                                serde_json::to_value(AuthChallengeResult {
                                    server_nonce,
                                    server_proof: forged,
                                })
                                .unwrap(),
                            ),
                        )
                        .await;
                        // 客户端若继续发帧，全部记录下来。
                        while let Ok(Some(Ok(message))) =
                            tokio::time::timeout(std::time::Duration::from_millis(300), ws.next())
                                .await
                        {
                            if let Message::Text(text) = message {
                                frames.lock().unwrap().push(text.to_string());
                            }
                        }
                    }
                }
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            let Err(super::ConnectError::Fatal(data)) = super::connect(&config).await else {
                panic!("an endpoint that cannot prove the token must be rejected");
            };
            assert_eq!(data.code, ApplicationErrorCode::Unauthorized);
            assert!(config.http_session().credential().is_none());
            assert!(super::request_shutdown(&config).await.is_err());
            server.await.unwrap();

            let frames = frames.lock().unwrap();
            assert_eq!(
                frames.len(),
                2,
                "only the challenges reach the impostor: {frames:?}"
            );
            for frame in frames.iter() {
                assert!(frame.contains(SYSTEM_AUTH_CHALLENGE), "{frame}");
                assert!(!frame.contains(TOKEN), "{frame}");
                assert!(!frame.contains("clientProof"), "{frame}");
                assert!(!frame.contains(method::SYSTEM_SHUTDOWN), "{frame}");
            }
            let headers = upgrade_headers.lock().unwrap();
            assert!(
                !headers
                    .iter()
                    .any(|header| header.to_ascii_lowercase().starts_with("authorization")),
                "{headers:?}"
            );
            assert!(!headers.iter().any(|header| header.contains(TOKEN)));
        })
        .await
        .expect("impostor is rejected without leaking credentials");
    }

    #[tokio::test]
    async fn daemon_proof_recorded_from_another_session_is_not_accepted() {
        use futures_util::StreamExt;

        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let (listener, url) = loopback_listener().await;
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let (client_nonce, server_nonce) = daemon_handshake(&mut ws, TOKEN).await;
                let recorded = server_proof(TOKEN, &client_nonce, &server_nonce).unwrap();

                // 第二条连接：攻击者回放录下来的（serverNonce, serverProof），客户端的新随机数不同。
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let challenge = read_request(&mut ws).await;
                send_response(
                    &mut ws,
                    RpcResponse::success(
                        challenge.id,
                        serde_json::to_value(AuthChallengeResult {
                            server_nonce,
                            server_proof: recorded,
                        })
                        .unwrap(),
                    ),
                )
                .await;
                let mut later_frames = 0;
                while let Ok(Some(Ok(_))) =
                    tokio::time::timeout(std::time::Duration::from_millis(300), ws.next()).await
                {
                    later_frames += 1;
                }
                later_frames
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            let mut buffered = Vec::new();
            let mut first = super::open_socket(&config)
                .await
                .unwrap_or_else(|_| panic!("open first"));
            assert!(
                super::authenticate(&mut first, TOKEN, &mut buffered)
                    .await
                    .is_ok()
            );
            let mut second = super::open_socket(&config)
                .await
                .unwrap_or_else(|_| panic!("open second"));
            let Err(super::ConnectError::Fatal(data)) =
                super::authenticate(&mut second, TOKEN, &mut buffered).await
            else {
                panic!("replayed proof must be rejected");
            };
            assert_eq!(data.code, ApplicationErrorCode::Unauthorized);
            assert_eq!(
                server.await.unwrap(),
                0,
                "no client proof after a bad server proof"
            );
        })
        .await
        .expect("replay is rejected");
    }

    #[tokio::test]
    async fn shutdown_request_is_sent_only_after_the_daemon_is_authenticated() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (listener, url) = loopback_listener().await;
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                daemon_handshake(&mut ws, TOKEN).await;
                let shutdown = read_request(&mut ws).await;
                send_response(
                    &mut ws,
                    RpcResponse::success(shutdown.id, json!({ "ok": true })),
                )
                .await;
                shutdown.method
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            assert!(super::request_shutdown(&config).await.is_ok());
            assert_eq!(server.await.unwrap(), method::SYSTEM_SHUTDOWN);
        })
        .await
        .expect("shutdown after handshake");
    }

    #[tokio::test]
    async fn legacy_daemon_rejecting_the_upgrade_is_identified_without_sending_credentials() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (listener, url) = loopback_listener().await;
            let server = tokio::spawn(async move {
                let (mut tcp, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !received.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = tcp.read(&mut chunk).await.unwrap();
                    assert!(
                        read > 0,
                        "client closed before finishing the upgrade request"
                    );
                    received.extend_from_slice(&chunk[..read]);
                }
                tcp.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
                String::from_utf8_lossy(&received).to_ascii_lowercase()
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            let Err(super::ConnectError::LegacyPeer) = super::connect(&config).await else {
                panic!("an upgrade rejected with 401 marks a legacy daemon");
            };
            let request = server.await.unwrap();
            assert!(!request.contains("authorization"), "{request}");
            assert!(!request.contains(TOKEN), "{request}");
            assert!(config.http_session().credential().is_none());
        })
        .await
        .expect("legacy daemon is identified");
    }

    #[tokio::test]
    async fn daemon_that_does_not_know_the_handshake_is_identified_as_legacy() {
        use futures_util::StreamExt;

        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let (listener, url) = loopback_listener().await;
            let server = tokio::spawn(async move {
                let mut methods = Vec::new();
                for error in [
                    // 旧版 daemon 的首帧门禁：只认 hello，对其它方法回「method 字段无效」。
                    RpcErrorObject::application(
                        "hello rejected",
                        RpcErrorData {
                            field: Some("method".to_owned()),
                            ..RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false)
                        },
                    ),
                    RpcErrorObject {
                        code: METHOD_NOT_FOUND_CODE,
                        message: "method not found".to_owned(),
                        data: None,
                    },
                ] {
                    let (tcp, _) = listener.accept().await.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                    let challenge = read_request(&mut ws).await;
                    methods.push(challenge.method);
                    send_response(&mut ws, RpcResponse::failure(challenge.id, error)).await;
                    while let Ok(Some(Ok(message))) =
                        tokio::time::timeout(std::time::Duration::from_millis(200), ws.next()).await
                    {
                        if let Message::Text(text) = message {
                            methods.push(text.to_string());
                        }
                    }
                }
                methods
            });
            let config = super::DaemonClientConfig::new(url, TOKEN);
            for _ in 0..2 {
                let Err(super::ConnectError::LegacyPeer) = super::connect(&config).await else {
                    panic!("a peer that does not know the challenge is a legacy daemon");
                };
            }
            // 对端只看到过挑战，没有任何证明、token 或后续帧。
            assert_eq!(
                server.await.unwrap(),
                vec![SYSTEM_AUTH_CHALLENGE, SYSTEM_AUTH_CHALLENGE]
            );
            assert!(config.http_session().credential().is_none());
        })
        .await
        .expect("unrecognized handshake is identified");
    }

    #[test]
    fn legacy_peer_detection_is_narrow_and_replacement_is_bounded() {
        use tokio_tungstenite::tungstenite::{Error, http::Response};

        let rejected = |status: u16| {
            Error::Http(Box::new(
                Response::builder().status(status).body(None).unwrap(),
            ))
        };
        assert!(matches!(
            super::classify_connect_error(rejected(401)),
            super::ConnectError::LegacyPeer
        ));
        for status in [403, 404, 500, 503] {
            assert!(
                matches!(
                    super::classify_connect_error(rejected(status)),
                    super::ConnectError::Transient(_)
                ),
                "{status}"
            );
        }

        let field = |field: &str| RpcErrorData {
            field: Some(field.to_owned()),
            ..RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false)
        };
        assert!(super::is_unrecognized_handshake(&field("method")));
        assert!(super::is_unrecognized_handshake(&RpcErrorData::new(
            ApplicationErrorCode::Unsupported,
            false
        )));
        // 本版 daemon 自己的拒绝（坏随机数 / 证明不符）不是旧版 daemon。
        assert!(!super::is_unrecognized_handshake(&field("clientNonce")));
        assert!(!super::is_unrecognized_handshake(&RpcErrorData::new(
            ApplicationErrorCode::Unauthorized,
            false
        )));
        assert!(!super::is_unrecognized_handshake(&RpcErrorData::new(
            ApplicationErrorCode::InvalidArgument,
            false
        )));

        for attempts in 0..super::LEGACY_REPLACE_ATTEMPTS {
            assert_eq!(
                super::next_legacy_step(attempts),
                super::LegacyStep::Replace
            );
        }
        assert_eq!(
            super::next_legacy_step(super::LEGACY_REPLACE_ATTEMPTS),
            super::LegacyStep::GiveUp
        );
    }
}
