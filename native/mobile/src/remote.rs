//! 远端主机传输：`fluxdown-agent --server` 的 WebSocket `/rpc`（与 Web SPA 同一线协议）。
//!
//! - `Authorization: Bearer <access key>`，不发送 `Origin`；
//! - wss 使用 rustls（ring 提供者，显式安装一次）+ 内置 WebPKI 根（Android 无法在无 JNI 的
//!   情况下使用系统验证器）；
//! - 每 25s 发 `system.ping`，10s 无应答视为断线；
//! - 关闭码 4009（事件缺口）→ 立即重连；关闭原因 `service-quit` → 终止；
//!   升级请求 HTTP 401/403 → 不可恢复的 `Unauthorized`。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Once, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use fluxdown_protocol::method::{SERVICE_EVENT, SYSTEM_PING};
use fluxdown_protocol::{
    CLOSE_REASON_SERVICE_QUIT, EventFrame, RequestId, RpcErrorObject, RpcRequest, RpcResponse,
    ServiceHello,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header::AUTHORIZATION};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{Connector, connect_async_tls_with_config};

use crate::error::{ErrorCodeDto, FluxError, HostErrorDto};
use crate::link::{
    Caller, CloseInfo, ConnectFailure, Connector as LinkConnector, Established, EventSource,
    LinkEvent,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_INTERVAL: Duration = Duration::from_secs(25);
const PING_TIMEOUT: Duration = Duration::from_secs(10);
/// 事件缓冲上限；溢出按「事件缺口」处理（立即重连并重拉快照）。
const EVENT_QUEUE: usize = 20_000;
/// 服务端广播队列溢出时使用的关闭码（`gateway.rs`）。
const CLOSE_CODE_EVENT_GAP: u16 = 4009;
const PING_ID: &str = "fluxdown-mobile-ping";

/// 把 `http(s)://host:port[/base]` / `ws(s)://host:port[/base]` 规范化为 `ws(s)://…/rpc`。
///
/// 无 scheme 的 `host:port` 按 `ws://` 处理；`/rpc` 已存在则保持，否则追加到现有 base path
/// （反向代理子路径）；query / fragment 被丢弃。
pub(crate) fn normalize_endpoint(endpoint: &str) -> Result<String, FluxError> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err(FluxError::invalid_argument("endpoint is empty"));
    }
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("ws".to_owned(), trimmed),
    };
    let ws_scheme = match scheme.as_str() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => {
            return Err(FluxError::invalid_argument(format!(
                "unsupported endpoint scheme: {other}"
            )));
        }
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = match rest.find('/') {
        Some(index) => rest.split_at(index),
        None => (rest, ""),
    };
    if authority.is_empty() || authority.contains(char::is_whitespace) || authority.contains('@') {
        return Err(FluxError::invalid_argument("endpoint host is invalid"));
    }
    let base = path.trim_end_matches('/');
    let path = if base.ends_with("/rpc") {
        base.to_owned()
    } else {
        format!("{base}/rpc")
    };
    Ok(format!("{ws_scheme}://{authority}{path}"))
}

static CRYPTO_PROVIDER: Once = Once::new();

/// 进程内只安装一次 rustls 默认加密提供者（依赖图里可能同时存在多个提供者）。
fn install_crypto_provider() {
    CRYPTO_PROVIDER.call_once(|| {
        if rustls::crypto::ring::default_provider()
            .install_default()
            .is_err()
        {
            tracing::debug!("rustls default crypto provider was already installed");
        }
    });
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, FluxError> {
    install_crypto_provider();
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| FluxError::transport(format!("tls setup failed: {error}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

pub(crate) struct RemoteConnector {
    url: String,
    authorization: HeaderValue,
    tls: Arc<rustls::ClientConfig>,
}

impl RemoteConnector {
    pub(crate) fn new(endpoint: &str, access_key: &str) -> Result<Self, FluxError> {
        let url = normalize_endpoint(endpoint)?;
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", access_key.trim()))
            .map_err(|_| FluxError::invalid_argument("access key contains invalid characters"))?;
        authorization.set_sensitive(true);
        Ok(Self {
            url,
            authorization,
            tls: tls_config()?,
        })
    }

    async fn connect_inner(&self, hello: RpcRequest) -> Result<Established, ConnectFailure> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|error| ConnectFailure::Retry(format!("invalid endpoint: {error}")))?;
        request
            .headers_mut()
            .insert(AUTHORIZATION, self.authorization.clone());
        // 快照与整表替换只带头、任务表分页拉取（单页 < 16 MiB）；仍沿用协议统一的入站上限，
        // 帧与消息上限一同放宽，避免大页被默认 16 MiB 限制截断。
        let limit = Some(fluxdown_protocol::MAX_INBOUND_RPC_MESSAGE_BYTES);
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_frame_size(limit)
            .max_message_size(limit);
        let (socket, _response) = connect_async_tls_with_config(
            request,
            Some(config),
            true,
            Some(Connector::Rustls(self.tls.clone())),
        )
        .await
        .map_err(connect_failure)?;

        let (out_tx, out_rx) = mpsc::channel(256);
        let (events_tx, events_rx) = mpsc::channel(EVENT_QUEUE);
        let (close_tx, close_rx) = oneshot::channel();
        let pending = Arc::new(PendingMap::default());
        tokio::spawn(io_loop(
            socket,
            out_rx,
            pending.clone(),
            events_tx,
            close_tx,
        ));
        let caller = Arc::new(RemoteCaller {
            out: out_tx,
            pending,
            next_id: AtomicI64::new(2),
        });

        let response = timeout(HELLO_TIMEOUT, caller.round_trip(hello))
            .await
            .map_err(|_| ConnectFailure::Retry("hello timed out".to_owned()))?
            .map_err(|error| ConnectFailure::Retry(error.to_string()))?;
        let value = match response {
            RpcResponse::Success(success) => success.result,
            RpcResponse::Failure(failure) => {
                return Err(ConnectFailure::Fatal(FluxError::from(failure.error).into()));
            }
        };
        let hello: ServiceHello = serde_json::from_value(value).map_err(|error| {
            ConnectFailure::Fatal(HostErrorDto::new(
                ErrorCodeDto::ProtocolIncompatible,
                format!("not a FluxDown host: {error}"),
            ))
        })?;
        Ok(Established {
            hello,
            caller,
            events: Box::new(RemoteEvents {
                rx: events_rx,
                close: Some(close_rx),
            }),
        })
    }
}

/// 升级失败分类：401/403 不可恢复，其余可重试。
fn connect_failure(error: WsError) -> ConnectFailure {
    match error {
        WsError::Http(response) => {
            let status = response.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                ConnectFailure::Fatal(HostErrorDto::new(
                    ErrorCodeDto::Unauthorized,
                    format!("access key rejected (HTTP {status})"),
                ))
            } else {
                ConnectFailure::Retry(format!("unexpected HTTP status {status}"))
            }
        }
        other => ConnectFailure::Retry(other.to_string()),
    }
}

#[async_trait]
impl LinkConnector for RemoteConnector {
    async fn connect(&self, hello: RpcRequest) -> Result<Established, ConnectFailure> {
        timeout(CONNECT_TIMEOUT, self.connect_inner(hello))
            .await
            .unwrap_or_else(|_| Err(ConnectFailure::Retry("connect timed out".to_owned())))
    }
}

#[derive(Default)]
struct PendingMap {
    inner: Mutex<HashMap<RequestId, oneshot::Sender<RpcResponse>>>,
}

impl PendingMap {
    fn insert(&self, id: RequestId, sender: oneshot::Sender<RpcResponse>) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sender);
    }

    fn take(&self, id: &RequestId) -> Option<oneshot::Sender<RpcResponse>> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
    }

    /// 连接结束：丢弃全部等待者，它们收到 `Closed` 后报告传输错误。
    fn fail_all(&self) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// 调用方被取消（future 被丢弃）时移除等待项，避免泄漏。
struct PendingGuard<'a> {
    pending: &'a PendingMap,
    id: RequestId,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        // 响应已到达时条目早已取走；取消时在此清理。
        drop(self.pending.take(&self.id));
    }
}

struct RemoteCaller {
    out: mpsc::Sender<String>,
    pending: Arc<PendingMap>,
    next_id: AtomicI64,
}

impl RemoteCaller {
    async fn round_trip(&self, request: RpcRequest) -> Result<RpcResponse, FluxError> {
        let text = serde_json::to_string(&request)
            .map_err(|error| FluxError::internal(format!("encode request failed: {error}")))?;
        let (tx, rx) = oneshot::channel();
        self.pending.insert(request.id.clone(), tx);
        let _guard = PendingGuard {
            pending: &self.pending,
            id: request.id,
        };
        self.out
            .send(text)
            .await
            .map_err(|_| FluxError::transport("connection closed"))?;
        rx.await
            .map_err(|_| FluxError::transport("connection closed"))
    }
}

#[async_trait]
impl Caller for RemoteCaller {
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value, FluxError> {
        let id = RequestId::Integer(self.next_id.fetch_add(1, Ordering::Relaxed));
        match self.round_trip(RpcRequest::new(id, method, params)).await? {
            RpcResponse::Success(success) => Ok(success.result),
            RpcResponse::Failure(failure) => Err(failure.error.into()),
        }
    }
}

struct RemoteEvents {
    rx: mpsc::Receiver<Box<EventFrame>>,
    close: Option<oneshot::Receiver<CloseInfo>>,
}

#[async_trait]
impl EventSource for RemoteEvents {
    async fn next(&mut self) -> LinkEvent {
        if let Some(frame) = self.rx.recv().await {
            return LinkEvent::Frame(frame);
        }
        // 缓冲已排空：io 任务已结束，原因随后到达。
        let info = match self.close.take() {
            Some(close) => close
                .await
                .unwrap_or_else(|_| CloseInfo::Lost("connection task ended".to_owned())),
            None => CloseInfo::Lost("connection closed".to_owned()),
        };
        LinkEvent::Closed(info)
    }
}

/// 入站帧的最小信封：响应（`id` + `result`/`error`）与通知（`method` + `params`）。
#[derive(Deserialize)]
struct Incoming {
    #[serde(default)]
    id: Option<RequestId>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<RpcErrorObject>,
}

fn close_info(frame: Option<CloseFrame>) -> CloseInfo {
    let Some(frame) = frame else {
        return CloseInfo::Lost("closed by peer".to_owned());
    };
    let code = u16::from(frame.code);
    if frame.reason.as_str() == CLOSE_REASON_SERVICE_QUIT {
        CloseInfo::ServiceQuit
    } else if code == CLOSE_CODE_EVENT_GAP {
        CloseInfo::EventGap
    } else {
        CloseInfo::Lost(format!("closed by peer ({code} {})", frame.reason))
    }
}

/// 单个连接的 IO 循环：写出队列、读入帧分发、`system.ping` 保活。
async fn io_loop(
    mut socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mut out_rx: mpsc::Receiver<String>,
    pending: Arc<PendingMap>,
    events: mpsc::Sender<Box<EventFrame>>,
    close_tx: oneshot::Sender<CloseInfo>,
) {
    let ping_id = RequestId::String(PING_ID.to_owned());
    let mut ping_due = Instant::now() + PING_INTERVAL;
    let mut ping_deadline: Option<Instant> = None;

    let info = loop {
        let ping_wait = ping_deadline.unwrap_or(ping_due);
        tokio::select! {
            outgoing = out_rx.recv() => {
                let Some(text) = outgoing else {
                    break CloseInfo::Lost("session dropped".to_owned());
                };
                if let Err(error) = socket.send(Message::Text(text.into())).await {
                    break CloseInfo::Lost(format!("write failed: {error}"));
                }
            }
            () = events.closed() => break CloseInfo::Lost("session dropped".to_owned()),
            () = sleep_until(ping_wait) => {
                if ping_deadline.is_some() {
                    break CloseInfo::Lost("ping timed out".to_owned());
                }
                let request = RpcRequest::new(ping_id.clone(), SYSTEM_PING, None);
                let Ok(text) = serde_json::to_string(&request) else {
                    break CloseInfo::Lost("encode ping failed".to_owned());
                };
                if let Err(error) = socket.send(Message::Text(text.into())).await {
                    break CloseInfo::Lost(format!("write failed: {error}"));
                }
                ping_deadline = Some(Instant::now() + PING_TIMEOUT);
            }
            incoming = socket.next() => {
                let message = match incoming {
                    None => break CloseInfo::Lost("stream ended".to_owned()),
                    Some(Err(error)) => break CloseInfo::Lost(format!("read failed: {error}")),
                    Some(Ok(message)) => message,
                };
                match message {
                    Message::Text(text) => {
                        let Ok(frame) = serde_json::from_str::<Incoming>(text.as_str()) else {
                            tracing::warn!("dropping undecodable frame from remote host");
                            continue;
                        };
                        if let Some(id) = &frame.id
                            && *id == ping_id
                        {
                            ping_due = Instant::now() + PING_INTERVAL;
                            ping_deadline = None;
                            continue;
                        }
                        if let Some(overflow) = dispatch(frame, &pending, &events) {
                            break overflow;
                        }
                    }
                    Message::Close(frame) => break close_info(frame),
                    Message::Binary(_)
                    | Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Frame(_) => {}
                }
            }
        }
    };

    pending.fail_all();
    // 尽力发送关闭帧；连接多半已断，失败无需处理。
    if let Err(error) = socket.close(None).await {
        tracing::debug!("closing remote socket: {error}");
    }
    if close_tx.send(info).is_err() {
        tracing::debug!("remote connection owner already gone");
    }
}

/// 分发一帧；事件缓冲溢出时返回应结束连接的原因。
fn dispatch(
    frame: Incoming,
    pending: &PendingMap,
    events: &mpsc::Sender<Box<EventFrame>>,
) -> Option<CloseInfo> {
    if frame.method.as_deref() == Some(SERVICE_EVENT) {
        let params = frame.params?;
        let Ok(event) = serde_json::from_value::<EventFrame>(params) else {
            tracing::warn!("dropping undecodable service.event");
            return None;
        };
        return match events.try_send(Box::new(event)) {
            Ok(()) => None,
            Err(mpsc::error::TrySendError::Full(_)) => Some(CloseInfo::EventGap),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Some(CloseInfo::Lost("session dropped".to_owned()))
            }
        };
    }
    let id = frame.id?;
    let response = match frame.error {
        Some(error) => RpcResponse::failure(id.clone(), error),
        None => RpcResponse::success(id.clone(), frame.result),
    };
    if let Some(waiter) = pending.take(&id)
        && waiter.send(response).is_err()
    {
        tracing::debug!("response arrived after the caller gave up");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::normalize_endpoint;

    fn norm(input: &str) -> String {
        normalize_endpoint(input).expect("valid endpoint")
    }

    #[test]
    fn http_and_https_become_ws_and_wss_with_rpc_path() {
        assert_eq!(norm("http://nas.local:8080"), "ws://nas.local:8080/rpc");
        assert_eq!(norm("https://dl.example.com"), "wss://dl.example.com/rpc");
        assert_eq!(norm("ws://10.0.0.2:9000/"), "ws://10.0.0.2:9000/rpc");
        assert_eq!(norm("WSS://Host:1"), "wss://Host:1/rpc");
    }

    #[test]
    fn existing_rpc_path_is_kept_and_subpath_is_extended() {
        assert_eq!(norm("ws://h:1/rpc"), "ws://h:1/rpc");
        assert_eq!(norm("https://h/fluxdown"), "wss://h/fluxdown/rpc");
        assert_eq!(norm("https://h/fluxdown/rpc/"), "wss://h/fluxdown/rpc");
    }

    #[test]
    fn query_fragment_and_whitespace_are_dropped() {
        assert_eq!(norm("  http://h:1/?token=x#frag "), "ws://h:1/rpc");
    }

    #[test]
    fn bare_host_defaults_to_plain_ws_and_ipv6_is_preserved() {
        assert_eq!(norm("192.168.1.5:8080"), "ws://192.168.1.5:8080/rpc");
        assert_eq!(norm("http://[::1]:8080"), "ws://[::1]:8080/rpc");
    }

    #[test]
    fn lan_pairing_address_shapes_map_to_the_rpc_endpoint() {
        // 局域网配对得到的主机地址（发现的 `host:port`、手输、配对码里的 `http(s)://host:port`、
        // 反代子路径）直接作为远端主机端点：设备控制凭据走同一条 `/rpc` 升级。
        assert_eq!(norm("192.168.1.20:17800"), "ws://192.168.1.20:17800/rpc");
        assert_eq!(
            norm("http://192.168.1.20:17800"),
            "ws://192.168.1.20:17800/rpc"
        );
        assert_eq!(norm("http://nas.local:17800/"), "ws://nas.local:17800/rpc");
        assert_eq!(
            norm("https://nas.example.com/fluxdown"),
            "wss://nas.example.com/fluxdown/rpc"
        );
        assert_eq!(
            norm("https://nas.example.com:8443/fluxdown/"),
            "wss://nas.example.com:8443/fluxdown/rpc"
        );
        assert_eq!(norm("[fd00::5]:17800"), "ws://[fd00::5]:17800/rpc");
        assert_eq!(norm("http://[fd00::5]:17800"), "ws://[fd00::5]:17800/rpc");
    }

    #[test]
    fn rejects_empty_unknown_scheme_and_credentials() {
        for bad in [
            "",
            "   ",
            "ftp://h:1",
            "http://",
            "http://user@h:1",
            "http://a b:1",
        ] {
            assert!(normalize_endpoint(bad).is_err(), "{bad:?} must be rejected");
        }
    }
}
