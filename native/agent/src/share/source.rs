//! 接收会话的 HTTP 区间来源：云端暂存副本（CDN / 对象存储）与网关桥接。
//!
//! 两者都实现 `fluxdown_share::ChunkSource`：会话按块来取，**块的 BLAKE3 校验由会话完成**，
//! 这里只负责「签发地址 → 带 `Range` 取回恰好 `len` 字节」，并把失败翻译成会话能处理的
//! [`SourceError`]（换来源 / 拉黑 / 额度耗尽 / 分享已结束）。
//!
//! - **云端副本**：每个块一次 `POST /share/download-ticket`（云端按签发区间预占流量，所以区间与块严格一致），
//!   再对签名 URL 发 `Range`，不带任何额外头；
//! - **桥接**：一次 `POST /share/bridge-ticket` 换得网关地址与令牌，之后每块 `GET <url>/<fileIndex>`
//!   （`Authorization: Bearer <token>` + `Range`），令牌将过期或被拒时重新换取；
//! - 出网统一走 agent 的 [`Egress`]（`verified_payload = false`：不交给插件改写），按出口网络缓存 HTTP 客户端。

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use fluxdown_protocol::{ApplicationErrorCode, ErrorReason, RpcErrorData};
use fluxdown_share::{Bytes, ChunkSource, SourceError, SourceFuture, SourceKind, SourceRequest};
use serde::Deserialize;
use serde_json::json;

use super::cloud::share_error;
use super::time::parse_rfc3339_secs;
use crate::cloud::{CloudApi, CloudError};
use crate::egress::{Egress, EgressClient};

/// 单块 HTTP 请求的连接 / 读停顿上限（块只有 4 MiB，不设整体超时以免慢网误杀）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 副本未就绪后多久内不再向云端要地址（避免每个块都打一次 409）。
const NOT_READY_BACKOFF: Duration = Duration::from_secs(30);
/// 错误响应体最多读多少字节（只为取 `{code}`）。
const ERROR_BODY_LIMIT: usize = 4096;
/// 桥接令牌距离过期不足这么久就重新换取。
const GRANT_REFRESH_MARGIN_SECS: u64 = 30;

/// 出网 HTTP 工具：按 [`Egress`] 方案取客户端，发带 `Range` 的 GET。
pub(super) struct SourceHttp {
    egress: Egress,
    client: EgressClient,
}

/// 一次 HTTP 取块失败的事实（状态码 + 云端 / 网关错误体里的 `code`）。
#[derive(Debug)]
pub(super) struct HttpFailure {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
}

impl HttpFailure {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            status: None,
            code: None,
            message: message.into(),
        }
    }
}

impl SourceHttp {
    pub(super) fn new(egress: Egress) -> Self {
        Self {
            egress,
            client: EgressClient::new(|| {
                reqwest::Client::builder()
                    .connect_timeout(CONNECT_TIMEOUT)
                    .read_timeout(READ_TIMEOUT)
                    .user_agent(concat!("fluxdown-agent/", env!("CARGO_PKG_VERSION")))
            }),
        }
    }

    /// `GET url`，`Range: bytes=offset-(offset+len-1)`，返回恰好 `len` 字节。
    pub(super) async fn get_range(
        &self,
        url: &str,
        bearer: Option<&str>,
        offset: u64,
        len: u64,
    ) -> Result<Bytes, HttpFailure> {
        if len == 0 {
            return Ok(Bytes::new());
        }
        let plan = self.egress.plan(url, false).await;
        let client = self
            .client
            .get(&plan.network)
            .await
            .map_err(|error| HttpFailure::transport(format!("http client: {error}")))?;
        // `verified_payload = false`：方案里只有原地址，不会有插件改写。
        let target = plan.attempts().last().map_or(url, |a| a.url.as_str());
        let mut request = client.get(target).header(
            reqwest::header::RANGE,
            format!("bytes={offset}-{}", offset + len - 1),
        );
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| HttpFailure::transport(format!("request failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(failure_of(response).await);
        }
        // 只接受 206，或（区间恰为整个对象时）200：其余说明服务端忽略了 Range，读下去会取到错的字节。
        let announced = response.content_length();
        if status != reqwest::StatusCode::PARTIAL_CONTENT
            && !(status == reqwest::StatusCode::OK && offset == 0 && announced == Some(len))
        {
            return Err(HttpFailure {
                status: Some(status.as_u16()),
                code: None,
                message: format!("range request answered with {status}"),
            });
        }
        let cap = usize::try_from(len)
            .map_err(|_| HttpFailure::transport("range does not fit in memory"))?;
        let mut body = Vec::with_capacity(cap);
        while let Some(part) = response
            .chunk()
            .await
            .map_err(|error| HttpFailure::transport(format!("body interrupted: {error}")))?
        {
            if body.len() + part.len() > cap {
                return Err(HttpFailure::transport("body is longer than the range"));
            }
            body.extend_from_slice(&part);
        }
        if body.len() != cap {
            return Err(HttpFailure::transport(format!(
                "body ended after {} of {cap} bytes",
                body.len()
            )));
        }
        Ok(Bytes::from(body))
    }
}

async fn failure_of(mut response: reqwest::Response) -> HttpFailure {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while body.len() < ERROR_BODY_LIMIT {
        match response.chunk().await {
            Ok(Some(part)) => body.extend_from_slice(&part),
            Ok(None) | Err(_) => break,
        }
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    let envelope = serde_json::from_slice::<Envelope>(&body).ok();
    HttpFailure {
        status: Some(status),
        code: envelope.as_ref().and_then(|e| e.code.clone()),
        message: envelope
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("http {status}")),
    }
}

// ───────────── 云端副本（CDN） ─────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadTicket {
    url: String,
}

/// 云端暂存副本来源。
pub(super) struct CloudCopySource {
    cloud: CloudApi,
    http: Arc<SourceHttp>,
    share_id: String,
    ticket: String,
    /// 副本未就绪时的退避截止时刻。
    not_ready_until: Mutex<Option<Instant>>,
}

impl CloudCopySource {
    pub(super) fn new(
        cloud: CloudApi,
        http: Arc<SourceHttp>,
        share_id: String,
        ticket: String,
    ) -> Self {
        Self {
            cloud,
            http,
            share_id,
            ticket,
            not_ready_until: Mutex::new(None),
        }
    }

    fn backing_off(&self) -> bool {
        self.not_ready_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|until| Instant::now() < until)
    }

    fn back_off(&self) {
        *self
            .not_ready_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now() + NOT_READY_BACKOFF);
    }

    async fn fetch_chunk(&self, request: SourceRequest) -> Result<Bytes, SourceError> {
        if self.backing_off() {
            return Err(SourceError::Unavailable(
                "the cloud copy is not ready".to_owned(),
            ));
        }
        let body = json!({
            "shareId": self.share_id,
            "ticket": self.ticket,
            "fileIndex": request.file_index,
            "offset": request.offset,
            "len": request.len,
        });
        let ticket: DownloadTicket =
            self.cloud
                .share_download_ticket(&body)
                .await
                .map_err(|error| {
                    if error.code.as_deref() == Some("share_cloud_not_ready") {
                        self.back_off();
                    }
                    ticket_failure(&error)
                })?;
        self.http
            .get_range(&ticket.url, None, request.offset, request.len)
            .await
            .map_err(|failure| match failure.status {
                // 对象不见了（被清理 / 下架）：本会话不再用云端副本。
                Some(404) => SourceError::Refused(failure.message),
                _ => SourceError::Unavailable(failure.message),
            })
    }
}

impl ChunkSource for CloudCopySource {
    fn kind(&self) -> SourceKind {
        SourceKind::Cdn
    }

    fn fetch(&self, request: SourceRequest) -> SourceFuture<'_> {
        Box::pin(self.fetch_chunk(request))
    }
}

/// 云端「签发地址」失败 → 来源错误。
fn ticket_failure(error: &CloudError) -> SourceError {
    match (error.code.as_deref(), error.status) {
        (Some("traffic_exhausted"), _) => SourceError::TrafficExhausted,
        (Some("share_file_changed"), _) => SourceError::Changed,
        (Some("share_ended" | "share_takedown"), _) | (_, Some(410)) => SourceError::Ended,
        (Some("share_cloud_not_ready"), _) => {
            SourceError::Unavailable("the cloud copy is not ready".to_owned())
        }
        (Some("share_guest_disabled"), _) | (_, Some(401 | 403 | 404)) if !error.unreachable => {
            SourceError::Refused(error.message.clone())
        }
        _ => SourceError::Unavailable(error.message.clone()),
    }
}

// ───────────── 桥接 ─────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeGrantBody {
    url: String,
    token: String,
    #[serde(default)]
    expires_at: Option<String>,
}

/// 网关桥接来源。
pub(super) struct BridgeSource {
    cloud: CloudApi,
    http: Arc<SourceHttp>,
    share_id: String,
    ticket: String,
    grant: tokio::sync::Mutex<BridgeGrantBody>,
}

impl BridgeSource {
    /// 换取第一份桥接授权（失败即无法桥接，错误原样交给调用方翻译）。
    pub(super) async fn connect(
        cloud: CloudApi,
        http: Arc<SourceHttp>,
        share_id: String,
        ticket: String,
    ) -> Result<Arc<Self>, CloudError> {
        let grant = request_grant(&cloud, &share_id, &ticket).await?;
        Ok(Arc::new(Self {
            cloud,
            http,
            share_id,
            ticket,
            grant: tokio::sync::Mutex::new(grant),
        }))
    }

    /// 当前可用的授权；快过期（或 `force`）就重新换取。
    async fn current(&self, force: bool) -> Result<BridgeGrantBody, CloudError> {
        let mut grant = self.grant.lock().await;
        let now = fluxdown_share::timefmt::now_unix_secs();
        let stale = grant
            .expires_at
            .as_deref()
            .and_then(parse_rfc3339_secs)
            .is_some_and(|at| now + GRANT_REFRESH_MARGIN_SECS >= at);
        if force || stale {
            *grant = request_grant(&self.cloud, &self.share_id, &self.ticket).await?;
        }
        Ok(grant.clone())
    }

    async fn fetch_chunk(&self, request: SourceRequest) -> Result<Bytes, SourceError> {
        let grant = self.current(false).await.map_err(|e| ticket_failure(&e))?;
        let url = format!("{}/{}", grant.url.trim_end_matches('/'), request.file_index);
        match self
            .http
            .get_range(&url, Some(&grant.token), request.offset, request.len)
            .await
        {
            Ok(bytes) => Ok(bytes),
            Err(failure) => {
                if failure.status == Some(401) {
                    // 令牌被拒：换新的，让会话的下一次尝试带新令牌。
                    self.current(true).await.map_err(|e| ticket_failure(&e))?;
                }
                Err(bridge_failure(&failure))
            }
        }
    }
}

impl ChunkSource for BridgeSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Bridge
    }

    fn fetch(&self, request: SourceRequest) -> SourceFuture<'_> {
        Box::pin(self.fetch_chunk(request))
    }
}

async fn request_grant(
    cloud: &CloudApi,
    share_id: &str,
    ticket: &str,
) -> Result<BridgeGrantBody, CloudError> {
    cloud
        .share_bridge_ticket(&json!({ "shareId": share_id, "ticket": ticket }))
        .await
}

/// 网关的 HTTP 失败 → 来源错误。
fn bridge_failure(failure: &HttpFailure) -> SourceError {
    match (failure.code.as_deref(), failure.status) {
        (Some("traffic_exhausted"), _) => SourceError::TrafficExhausted,
        (Some("share_file_changed"), _) | (_, Some(409)) => SourceError::Changed,
        (Some("share_ended" | "share_takedown"), _) | (_, Some(410)) => SourceError::Ended,
        // 分享方不在线 / 网关暂时不可用：保持可用，稍后重试。
        (Some("share_sender_offline"), _) | (_, Some(502..=504) | None) => {
            SourceError::Unavailable(failure.message.clone())
        }
        (_, Some(403 | 404)) => SourceError::Refused(failure.message.clone()),
        _ => SourceError::Unavailable(failure.message.clone()),
    }
}

/// `agent.share.switchToBridge` 换取桥接授权失败 → 带类型的 RPC 错误。
pub(super) fn bridge_switch_error(error: &CloudError) -> RpcErrorData {
    match (error.code.as_deref(), error.status) {
        (Some("traffic_exhausted"), _) => RpcErrorData::new(ApplicationErrorCode::Conflict, false)
            .with_reason(ErrorReason::ShareTrafficExhausted),
        (Some("share_guest_disabled"), _) => {
            RpcErrorData::new(ApplicationErrorCode::Unavailable, false)
                .with_reason(ErrorReason::ShareBridgeUnavailable)
        }
        (Some("share_bridge_unavailable" | "share_sender_offline"), _) | (_, Some(503))
            if !error.unreachable =>
        {
            RpcErrorData::new(ApplicationErrorCode::Unavailable, true)
                .with_reason(ErrorReason::ShareBridgeUnavailable)
        }
        _ => share_error(error),
    }
}
