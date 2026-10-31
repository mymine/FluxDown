//! 上报出口与会话诊断簿。
//!
//! [`Telemetry`] 实现 `ShareTelemetrySink`：iroh 任务里只入队（有界，溢出丢最旧的非终态报告），
//! 由 [`post_loop`] 在自己的任务里按退避向 FluxCloud 上报；同时把每条会话报告记入
//! [`DiagBook`] 供诊断页读取。空闲时没有任何定时器：队列空就阻塞在 `Notify`。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use fluxdown_protocol::ShareSessionDetailDto;
use fluxdown_share::{NetSnapshot, SessionReport, SessionRole, ShareTelemetrySink};
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::cloud::{NetResponse, SessionResponse};
use super::{ShareService, convert};
use crate::cloud::CloudApi;

const SESSION_QUEUE_CAP: usize = 256;
const SAMPLE_CAP: usize = 120;
const ENDED_KEEP: usize = 20;
const BACKOFF_START: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// 游客会话令牌：登录后不再使用；只在内存里，每次接收各一枚。
#[derive(Default)]
pub(super) struct GuestTokens {
    by_share: HashMap<String, String>,
    latest: Option<String>,
}

impl GuestTokens {
    pub(super) fn insert(&mut self, share_id: &str, token: String) {
        // 令牌数量与同时接收的分享数同量级；超限时整体清掉再放最新一枚。
        if self.by_share.len() >= 64 {
            self.by_share.clear();
        }
        self.latest = Some(token.clone());
        self.by_share.insert(share_id.to_owned(), token);
    }

    fn for_share(&self, share_id: &str) -> Option<String> {
        self.by_share
            .get(share_id)
            .cloned()
            .or_else(|| self.latest.clone())
    }

    fn any(&self) -> Option<String> {
        self.latest.clone()
    }
}

#[derive(Default)]
struct Queue {
    net: Option<NetSnapshot>,
    sessions: VecDeque<SessionReport>,
}

/// 诊断簿：会话报告里的最新摘要与时序采样。
#[derive(Default)]
pub(super) struct DiagBook {
    sessions: HashMap<String, ShareSessionDetailDto>,
    order: VecDeque<String>,
}

impl DiagBook {
    fn record(&mut self, report: &SessionReport) {
        let detail = self
            .sessions
            .entry(report.session_id.clone())
            .or_insert_with(|| ShareSessionDetailDto {
                session_id: report.session_id.clone(),
                share_id: report.share_id.clone(),
                role: convert::role(report.role),
                peer_endpoint_id: report.peer_endpoint_id.clone(),
                peer_device_name: None,
                peer_nat_type: fluxdown_protocol::ShareNatType::Unknown,
                peer_isp: None,
                ended: false,
                summary: convert::summary(&report.summary),
                samples: Vec::new(),
                swarm_files: Vec::new(),
            });
        detail.summary = convert::summary(&report.summary);
        detail.ended = report.is_final;
        // 提供方的服务量是连接内累计值，每份报告整体替换。
        detail.swarm_files = report.swarm_files.iter().map(convert::swarm_file).collect();
        detail
            .samples
            .extend(report.samples.iter().map(convert::sample));
        let excess = detail.samples.len().saturating_sub(SAMPLE_CAP);
        detail.samples.drain(..excess);
        if !self.order.contains(&report.session_id) {
            self.order.push_back(report.session_id.clone());
        }
        // 只保留最近 ENDED_KEEP 个已结束的会话。
        let ended: Vec<String> = self
            .order
            .iter()
            .filter(|id| self.sessions.get(*id).is_some_and(|d| d.ended))
            .cloned()
            .collect();
        for id in ended.iter().take(ended.len().saturating_sub(ENDED_KEEP)) {
            self.sessions.remove(id);
            self.order.retain(|other| other != id);
        }
    }

    pub(super) fn set_peer_name(&mut self, endpoint_id: &str, name: &str) {
        for detail in self.sessions.values_mut() {
            if detail.peer_endpoint_id == endpoint_id {
                detail.peer_device_name = Some(name.to_owned());
            }
        }
    }

    pub(super) fn details(&self, only: Option<&str>) -> Vec<ShareSessionDetailDto> {
        self.order
            .iter()
            .filter(|id| only.is_none_or(|wanted| wanted == id.as_str()))
            .filter_map(|id| self.sessions.get(id).cloned())
            .collect()
    }
}

/// 上报出口（`ShareTelemetrySink`）。
pub(super) struct Telemetry {
    queue: Mutex<Queue>,
    wake: Notify,
    pub(super) diag: Mutex<DiagBook>,
    pub(super) guests: Mutex<GuestTokens>,
}

impl Telemetry {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(Queue::default()),
            wake: Notify::new(),
            diag: Mutex::new(DiagBook::default()),
            guests: Mutex::new(GuestTokens::default()),
        })
    }
}

impl ShareTelemetrySink for Telemetry {
    fn net_snapshot(&self, snapshot: &NetSnapshot) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .net = Some(snapshot.clone());
        self.wake.notify_one();
    }

    fn session_report(&self, report: &SessionReport) {
        self.diag
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(report);
        {
            let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            if queue.sessions.len() >= SESSION_QUEUE_CAP {
                // 先丢最旧的非终态报告（会被后续摘要取代）；全是终态时丢最旧的终态。
                let position = queue
                    .sessions
                    .iter()
                    .position(|queued| !queued.is_final)
                    .unwrap_or(0);
                queue.sessions.remove(position);
            }
            queue.sessions.push_back(report.clone());
        }
        self.wake.notify_one();
    }
}

fn take(queue: &Mutex<Queue>) -> (Option<NetSnapshot>, Vec<SessionReport>) {
    let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
    (queue.net.take(), queue.sessions.drain(..).collect())
}

fn requeue(telemetry: &Telemetry, net: Option<NetSnapshot>, sessions: Vec<SessionReport>) {
    let mut queue = telemetry
        .queue
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(net) = net
        && queue.net.is_none()
    {
        queue.net = Some(net);
    }
    // 失败的终态报告放回队首；过期的进行中摘要不再重试（下一批会取代它们）。
    for report in sessions.into_iter().rev().filter(|r| r.is_final) {
        if queue.sessions.len() < SESSION_QUEUE_CAP {
            queue.sessions.push_front(report);
        }
    }
}

/// 后台上报：入队后按需发送，失败指数退避，取消时结束。
pub(super) async fn post_loop(
    telemetry: Arc<Telemetry>,
    cloud: CloudApi,
    service: Weak<ShareService>,
    cancel: CancellationToken,
) {
    let mut backoff = BACKOFF_START;
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = telemetry.wake.notified() => {}
        }
        let (net, sessions) = take(&telemetry.queue);
        if net.is_none() && sessions.is_empty() {
            continue;
        }
        let mut failed_net = None;
        let mut failed_sessions = Vec::new();
        let mut any_failure = false;
        if let Some(snapshot) = net {
            match post_net(&telemetry, &cloud, &snapshot).await {
                Ok(Some(response)) => {
                    if let Some(service) = service.upgrade() {
                        service.apply_geo(&response).await;
                    }
                }
                Ok(None) => {}
                Err(retry) => {
                    any_failure = true;
                    if retry {
                        failed_net = Some(snapshot);
                    }
                }
            }
        }
        for report in sessions {
            match post_session(&telemetry, &cloud, &report).await {
                Ok(killed) => {
                    if killed
                        && matches!(report.role, SessionRole::Sender | SessionRole::Swarm)
                        && let Some(service) = service.upgrade()
                    {
                        service.kill_session(&report.session_id).await;
                    }
                }
                Err(retry) => {
                    any_failure = true;
                    if retry {
                        failed_sessions.push(report);
                    }
                }
            }
        }
        if any_failure {
            requeue(&telemetry, failed_net, failed_sessions);
            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
            telemetry.wake.notify_one();
        } else {
            backoff = BACKOFF_START;
        }
    }
}

/// `Ok(Some)` 已送达并带回归属；`Ok(None)` 无可用鉴权而丢弃；`Err(retry)` 失败，`retry` 表示值得重试。
async fn post_net(
    telemetry: &Telemetry,
    cloud: &CloudApi,
    snapshot: &NetSnapshot,
) -> Result<Option<NetResponse>, bool> {
    let guest = telemetry
        .guests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .any();
    if !cloud.has_account().await && guest.is_none() {
        return Ok(None);
    }
    let body = json!({ "endpointId": snapshot.endpoint_id, "snapshot": snapshot });
    cloud
        .share_telemetry::<_, NetResponse>("net", &body, guest.as_deref())
        .await
        .map(Some)
        .map_err(|error| failure(&error, "net"))
}

async fn post_session(
    telemetry: &Telemetry,
    cloud: &CloudApi,
    report: &SessionReport,
) -> Result<bool, bool> {
    let guest = telemetry
        .guests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .for_share(&report.share_id);
    if !cloud.has_account().await && guest.is_none() {
        return Ok(false);
    }
    cloud
        .share_telemetry::<_, SessionResponse>("sessions", report, guest.as_deref())
        .await
        .map(|response| response.killed)
        .map_err(|error| failure(&error, "sessions"))
}

/// 记录失败并判断是否值得重试：4xx（除 429）是请求本身被拒绝，重试没有意义。
fn failure(error: &crate::cloud::CloudError, kind: &str) -> bool {
    tracing::debug!(kind, error = %error, "share telemetry post failed");
    error.unreachable || error.retryable || matches!(error.status, Some(429 | 500..=599))
}
