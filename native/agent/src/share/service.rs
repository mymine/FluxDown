//! FluxMesh服务：装配 `fluxdown_share` 引擎、状态投影、端点生命周期与事件循环。
//!
//! 宿主模式沿用 `LinkService`：`Arc::new_cyclic` 持自身 `Weak`；游离任务只持 `Weak`
//! （否则状态目录锁永远不释放，嵌入式宿主无法同进程重启）；`run` 在宿主关停清单里，取消即结束。
//!
//! 空闲静默：端点只在存在有效分享或进行中的接收任务时绑定；没有任何周期性定时器——
//! 仅有「最近到期时刻」的单次睡眠（且只在有有效分享时存在）与传输期间的一次性打洞提示计时。
//! 传输进度只在内存里、经 `ShareProgress` 事件下发，不写盘。
//!
//! 实验性开关（`ShareLocalConfigDto.enabled`，默认关）：关闭期间事件循环只等开启——不访问 FluxCloud、
//! 不绑定端点、不登记 / 上报，云端推来的FluxMesh事件直接丢弃；开启后按启动流程恢复，再次关闭时收掉
//! 端点 / 适配器 / 会话 / 上传并丢弃运行期状态（持久化的分享表、接收记录与共享状态原样保留）。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use fluxdown_protocol::{
    AgentEvent, ApplicationErrorCode, DaemonEvent, ErrorReason, RpcErrorData, ServiceEvent,
    ShareHashingDto, ShareLocalConfigDto, ShareNetStatusDto, ShareProgressDto,
    ShareReceiveRecordDto, ShareReceiveStatus, ShareResolveDto, ShareStateDto, WsServerMsg,
};
use fluxdown_share::{
    ContentRegistry, KeyRing, ReceiveSession, SessionEvent, SessionRole, ShareEndpoint,
    ShareRegistry,
};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::cloud::{CloudConfig, ResolveResponse};
use super::store::{ShareStore, StoredProvide, StoredShare};
use super::telemetry::{self, Telemetry};
use super::time::parse_rfc3339_secs;
use super::upload::UploadEntry;
use super::upload_job::Timing;
use super::{convert, logs};
use crate::cloud::CloudApi;
use crate::event_hub::AgentEventHub;
use crate::link::LinkTaskCreator;
use crate::state::{AgentState, StateStore};

/// 云端策略缓存的新鲜期：之后下一次需要时做一次 ETag 条件请求。
pub(super) const CONFIG_TTL: Duration = Duration::from_secs(300);
/// 解析句柄（含票据）的有效期与上限。
pub(super) const RESOLVE_TTL: Duration = Duration::from_secs(900);
pub(super) const RESOLVE_CAP: usize = 16;
/// 接收任务在快照里暂未出现时的宽限期（刚建出的任务事件可能还在路上）。
const TASK_GRACE: Duration = Duration::from_secs(15);
/// 已结束会话在内存里保留的个数。
const ENDED_SESSIONS_KEEP: usize = 30;
/// 分享停止 / 过期后保持端点的宽限（让接收方看到「分享已结束」）。
pub(super) const ENDED_GRACE: Duration = Duration::from_secs(120);

pub(super) fn unavailable() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Unavailable, false)
        .with_reason(ErrorReason::ShareUnavailable)
}

pub(super) fn invalid(field: &str) -> RpcErrorData {
    let mut data = RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false);
    data.field = Some(field.to_owned());
    data
}

pub(super) fn internal(error: &impl std::fmt::Display) -> RpcErrorData {
    tracing::warn!(error = %error, "share operation failed");
    RpcErrorData::new(ApplicationErrorCode::Internal, false)
}

/// 构造参数。
pub struct ShareServiceParts {
    pub events: AgentEventHub,
    pub state: Arc<Mutex<AgentState>>,
    pub store: Arc<StateStore>,
    pub cloud: CloudApi,
    pub tasks: Arc<dyn LinkTaskCreator>,
    /// 来自 `/tasks/events` SSE 的FluxMesh事件（`share.claimed` / `share.sessionKill` / `share.changed`）。
    pub cloud_events: mpsc::UnboundedReceiver<Value>,
    /// 统一出口：云端暂存的分片上传、CDN 下载、桥接取块都经它取网络方案（`verified_payload = false`）。
    pub egress: crate::egress::Egress,
}

pub(super) struct CachedConfig {
    pub cfg: CloudConfig,
    pub etag: Option<String>,
    pub fetched: Instant,
}

pub(super) struct ReceiveTask {
    pub share_id: String,
    pub record_id: String,
    pub token: String,
    pub status: i32,
    pub created: Instant,
    /// 该任务对应清单里的文件下标（共享按它取本会话已校验的块位图与块哈希）。
    pub file_index: u32,
}

impl ReceiveTask {
    /// 下载中 / 暂停 / 排队：仍可能取数，端点需要保持绑定。
    pub(super) fn is_active(&self) -> bool {
        matches!(self.status, 0 | 1 | 2 | 5)
    }
}

pub(super) struct Pending {
    pub resp: ResolveResponse,
    pub dto: ShareResolveDto,
    pub created: Instant,
}

pub(super) struct SessionMeta {
    pub share_id: String,
    pub role: SessionRole,
    pub peer: String,
    pub path: fluxdown_share::PathKind,
    pub rate_bps: u64,
    pub bytes: u64,
    pub total_bytes: u64,
    pub started_at: String,
    pub device_name: String,
    pub ended: bool,
    pub relay_stuck: bool,
}

#[derive(Default)]
pub(super) struct Inner {
    pub config: Option<CachedConfig>,
    pub local: ShareLocalConfigDto,
    pub endpoint: Option<ShareEndpoint>,
    pub endpoint_dirty: bool,
    pub registered_for: Option<String>,
    pub adapter: Option<fluxdown_share::LocalAdapter>,
    pub shares: Vec<StoredShare>,
    pub resolves: HashMap<String, Pending>,
    pub pending_receive: Option<String>,
    pub receive_tasks: HashMap<String, ReceiveTask>,
    /// 适配器令牌 → 接收会话（同一次接收的多个文件任务共用一个）。
    pub receive_sessions: HashMap<String, ReceiveSession>,
    /// 任务状态变化后需要重算状态的接收记录。
    pub dirty_records: std::collections::HashSet<String>,
    /// 正在进行的 `receive` 调用数：期间新建的会话 / 适配器不得被收尾逻辑回收。
    pub receiving: usize,
    pub records: Vec<ShareReceiveRecordDto>,
    pub sessions: HashMap<String, SessionMeta>,
    pub net: ShareNetStatusDto,
    pub hashing: BTreeMap<String, ShareHashingDto>,
    pub device_names: HashMap<String, String>,
    pub last_published: Option<ShareStateDto>,
    pub keys_fetched: Option<Instant>,
    /// 进行中 / 失败待重试的云端暂存上传（`upload.rs`）。
    pub uploads: HashMap<String, UploadEntry>,
    pub upload_timing: Timing,
    /// 游客发送方令牌缓存（`guest.rs`）。
    pub guest_session: Option<super::guest::GuestSession>,
    /// 已安排一次「接收方连上来后」的分享对账（`schedule_share_sync`），期间新会话不再重复安排。
    pub sync_pending: bool,
    /// 本机对分享表的修改计数（建 / 改 / 停 / 到期 / 云端副本变化 / 对账落地都加一）：对账在请求前记下，
    /// 落地时不一致就作废这次列表重来，避免请求期间的本机修改被旧列表覆盖（停掉的分享复活、新建的被判停止）。
    pub share_epoch: u64,
    /// 各次接收的共享状态（`provides.json`）。
    pub provides: Vec<StoredProvide>,
    /// 云端不认识共享接口（404）：本次运行内不再查询与登记。
    pub swarm_unsupported: bool,
    /// 下一次共享登记的时刻；`None` = 没有待办。
    pub announce_at: Option<Instant>,
    /// 待向云端撤销的共享登记（文件根哈希）。
    pub withdraw: Vec<String>,
    /// 最近一次成功拉取的额度与贡献（`usage.rs`）；未登录为 `None`。
    pub usage: Option<fluxdown_protocol::ShareUsageDto>,
    pub usage_error: Option<fluxdown_protocol::ErrorReason>,
    pub usage_fetched_at: Option<Instant>,
    /// 已安排一次防抖刷新，期间不再重复安排。
    pub usage_debounce: bool,
    /// 各分享的共享加速受限原因（`swarm.rs`）；查找成功即清除。
    pub swarm_blocks: HashMap<String, fluxdown_protocol::ShareSwarmBlock>,
    /// 因「需要登录」而停止的对端查找（适配器令牌 → 重试所需参数），登录后重新开始。
    pub swarm_login_waiting: HashMap<String, super::swarm::LoginWaiting>,
    /// 最近一次落盘的当日共享上传量。
    pub daily_saved: Option<super::store::ProvideDaily>,
}

/// FluxMesh服务。
pub struct ShareService {
    pub(super) weak: Weak<Self>,
    pub(super) cloud: CloudApi,
    pub(super) events: AgentEventHub,
    pub(super) egress: crate::egress::Egress,
    /// 云端副本 / 桥接来源共用的出网 HTTP 工具。
    pub(super) http: Arc<super::source::SourceHttp>,
    pub(super) state: Arc<Mutex<AgentState>>,
    pub(super) tasks: Arc<dyn LinkTaskCreator>,
    pub(super) store: ShareStore,
    pub(super) secret: fluxdown_share::SecretKey,
    pub(super) keys: KeyRing,
    pub(super) registry: ShareRegistry,
    pub(super) telemetry: Arc<Telemetry>,
    pub(super) session_tx: mpsc::UnboundedSender<SessionEvent>,
    session_rx: StdMutex<Option<mpsc::UnboundedReceiver<SessionEvent>>>,
    cloud_rx: StdMutex<Option<mpsc::UnboundedReceiver<Value>>>,
    /// 唤醒事件循环重算最近到期时刻。
    pub(super) wake: Notify,
    /// 一次 `run` 只允许一个：第二次调用直接返回。
    started: AtomicBool,
    pub(super) inner: StdMutex<Inner>,
    /// 同一时刻只有一个端点生命周期调整在跑。
    pub(super) lifecycle: Mutex<()>,
    /// 分享表 / 接收记录落盘串行化：快照与写入在同一把锁内，避免并发写共用临时文件撕裂文件、
    /// 或旧快照晚于新快照落盘。
    pub(super) persist: Mutex<()>,
    /// 本机能向别人提供的内容（端点的共享服务端只读它）；由 `sync_content` 整体重算。
    pub(super) content: ContentRegistry,
    #[cfg(test)]
    pub(super) addr_hint: StdMutex<Option<TestAddrHint>>,
}

#[cfg(test)]
pub(super) type TestAddrHint =
    Arc<dyn Fn(&str) -> Option<fluxdown_share::EndpointAddr> + Send + Sync>;

impl ShareService {
    /// 装配服务并读取本机持久化状态（私钥缺失时生成）。不访问网络、不绑定端点。
    ///
    /// # Errors
    ///
    /// 私钥文件无法读写。
    pub async fn new(parts: ShareServiceParts) -> Result<Arc<Self>, super::store::StoreError> {
        let store = ShareStore::new(parts.store.data_dir());
        let secret = store.load_or_create_secret().await?;
        let local = store.load_config().await;
        let shares = store.load_shares().await;
        let records = store.load_records().await;
        let provides = store.load_provides().await;
        logs::set_level(local.iroh_log_level);
        let (session_tx, session_rx) = mpsc::unbounded_channel();
        let inner = Inner {
            local,
            shares,
            records,
            provides,
            ..Inner::default()
        };
        let http = Arc::new(super::source::SourceHttp::new(parts.egress.clone()));
        Ok(Arc::new_cyclic(|weak| Self {
            weak: weak.clone(),
            cloud: parts.cloud,
            egress: parts.egress,
            http,
            events: parts.events,
            state: parts.state,
            tasks: parts.tasks,
            store,
            secret,
            keys: KeyRing::new(),
            registry: ShareRegistry::new(),
            telemetry: Telemetry::new(),
            session_tx,
            session_rx: StdMutex::new(Some(session_rx)),
            cloud_rx: StdMutex::new(Some(parts.cloud_events)),
            wake: Notify::new(),
            started: AtomicBool::new(false),
            inner: StdMutex::new(inner),
            lifecycle: Mutex::new(()),
            persist: Mutex::new(()),
            content: ContentRegistry::new(),
            #[cfg(test)]
            addr_hint: StdMutex::new(None),
        }))
    }

    pub(super) fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    // ───────────── 事件循环 ─────────────

    /// 事件循环：会话事件、daemon 任务事件、云端 SSE FluxMesh事件、到期计时；取消即结束。
    ///
    /// 本机开关关闭（默认）期间只等开启：会话事件与云端事件直接丢弃，不订阅任务事件、不访问网络。
    pub async fn run(self: Arc<Self>, cancel: CancellationToken) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(mut session_rx) = self
            .session_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let Some(mut cloud_rx) = self
            .cloud_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let mut cloud_open = true;
        // 先发布一次：快照里要有本机设置（含开关），UI 以它为底稿整体提交。
        self.publish().await;
        loop {
            while !self.is_enabled() {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = self.wake.notified() => {}
                    event = session_rx.recv() => if event.is_none() {
                        return;
                    },
                    event = cloud_rx.recv(), if cloud_open => cloud_open = event.is_some(),
                }
            }
            let disabled = self
                .run_enabled(&mut session_rx, &mut cloud_rx, &mut cloud_open, &cancel)
                .await;
            if !disabled {
                self.shutdown_runtime().await;
                return;
            }
            self.deactivate().await;
        }
    }

    /// 本机开关是否开启（`ShareLocalConfigDto.enabled`）。
    pub(super) fn is_enabled(&self) -> bool {
        self.with(|i| i.local.enabled)
    }

    /// 开启期间的事件循环。开关被关闭返回 `true`；取消或通道关闭返回 `false`。
    async fn run_enabled(
        self: &Arc<Self>,
        session_rx: &mut mpsc::UnboundedReceiver<SessionEvent>,
        cloud_rx: &mut mpsc::UnboundedReceiver<Value>,
        cloud_open: &mut bool,
        cancel: &CancellationToken,
    ) -> bool {
        let mut frames = self.events.subscribe();
        let poster_cancel = cancel.child_token();
        let poster = tokio::spawn(telemetry::post_loop(
            self.telemetry.clone(),
            self.cloud.clone(),
            self.weak.clone(),
            poster_cancel.clone(),
        ));
        // 启动检查只针对上次运行（或上次关闭前）留下的分享：之后新建的分享不属于「重启恢复」。
        let persisted: Vec<String> = self.with(|i| {
            i.shares
                .iter()
                .filter(|s| s.dto.status == fluxdown_protocol::ShareStatus::Active)
                .map(|s| s.dto.id.clone())
                .collect()
        });
        self.initial_sync(&persisted).await;
        let disabled = loop {
            if !self.is_enabled() {
                break true;
            }
            let deadline = self.next_expiry_delay();
            let announce_deadline = self.next_announce_delay();
            tokio::select! {
                () = cancel.cancelled() => break false,
                () = self.wake.notified() => {}
                () = async {
                    match deadline {
                        Some(delay) => tokio::time::sleep(delay).await,
                        None => std::future::pending().await,
                    }
                } => self.on_expiry().await,
                () = async {
                    match announce_deadline {
                        Some(delay) => tokio::time::sleep(delay).await,
                        None => std::future::pending().await,
                    }
                } => self.announce().await,
                event = session_rx.recv() => match event {
                    Some(event) => self.handle_session_event(event).await,
                    None => break false,
                },
                event = cloud_rx.recv(), if *cloud_open => match event {
                    Some(event) => self.handle_cloud_event(&event).await,
                    None => *cloud_open = false,
                },
                frame = frames.recv() => match frame {
                    Ok(frame) => self.handle_frame(frame.event).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        self.reconcile_tasks().await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break false,
                },
            }
        };
        poster_cancel.cancel();
        if let Err(error) = poster.await
            && !error.is_cancelled()
        {
            tracing::error!(error = %error, "share telemetry task panicked");
        }
        disabled
    }

    /// 开关被关闭：收掉端点 / 适配器 / 会话 / 上传，丢弃全部运行期状态（含云端策略缓存、待确认接收、
    /// 额度）。分享表、接收记录与共享状态的持久化文件原样保留，再次开启时按启动流程恢复（与重启等价）。
    async fn deactivate(&self) {
        self.shutdown_runtime().await;
        self.with(|i| {
            i.config = None;
            i.endpoint_dirty = false;
            i.registered_for = None;
            i.resolves.clear();
            i.pending_receive = None;
            i.receive_tasks.clear();
            i.dirty_records.clear();
            i.sessions.clear();
            i.net = ShareNetStatusDto::default();
            i.hashing.clear();
            i.keys_fetched = None;
            i.uploads.clear();
            i.guest_session = None;
            i.announce_at = None;
            i.swarm_unsupported = false;
            i.swarm_blocks.clear();
            i.swarm_login_waiting.clear();
            i.usage = None;
            i.usage_error = None;
            i.usage_fetched_at = None;
        });
        self.sync_content();
        self.publish().await;
    }

    /// 启动时的一次性同步：载入云端策略、重新登记仍有效的分享。没有本机分享时不做任何网络请求之外的事。
    async fn initial_sync(self: &Arc<Self>, persisted_active: &[String]) {
        if let Err(error) = self.config(false).await {
            tracing::debug!(reason = ?error.reason, "share policy unavailable at startup");
        }
        let has_active = self.with(|i| {
            i.shares
                .iter()
                .any(|s| s.dto.status == fluxdown_protocol::ShareStatus::Active)
        });
        if has_active {
            self.restore_shares(persisted_active).await;
        }
        self.restore_provides().await;
        self.publish().await;
    }

    async fn shutdown_runtime(&self) {
        self.stop_uploads();
        let (endpoint, adapter, sessions) = self.with(|i| {
            let sessions: Vec<ReceiveSession> =
                i.receive_sessions.drain().map(|(_, s)| s).collect();
            (i.endpoint.take(), i.adapter.take(), sessions)
        });
        for session in sessions {
            session.close().await;
        }
        if let Some(adapter) = adapter {
            adapter.shutdown().await;
        }
        if let Some(endpoint) = endpoint {
            self.persist_daily().await;
            endpoint.shutdown().await;
        }
    }

    // ───────────── 到期 ─────────────

    fn next_expiry_delay(&self) -> Option<Duration> {
        let now = fluxdown_share::timefmt::now_unix_secs();
        let next = self.with(|i| {
            let shares = i
                .shares
                .iter()
                .filter(|s| s.dto.status == fluxdown_protocol::ShareStatus::Active)
                .filter_map(|s| parse_rfc3339_secs(&s.dto.expires_at));
            let provides = i
                .provides
                .iter()
                .filter(|s| s.state == fluxdown_protocol::ShareProvideState::Providing)
                .filter_map(|s| parse_rfc3339_secs(s.share_expires_at.as_deref()?));
            shares.chain(provides).min()
        })?;
        Some(Duration::from_secs(next.saturating_sub(now)))
    }

    async fn on_expiry(&self) {
        let now = fluxdown_share::timefmt::now_unix_secs();
        let changed = self.with(|i| {
            let mut changed = false;
            for share in &mut i.shares {
                if share.dto.status == fluxdown_protocol::ShareStatus::Active
                    && parse_rfc3339_secs(&share.dto.expires_at).is_some_and(|t| t <= now)
                {
                    share.dto.status = fluxdown_protocol::ShareStatus::Expired;
                    changed = true;
                }
            }
            if changed {
                i.share_epoch += 1;
            }
            changed
        });
        let provides_ended = self.with(|i| i.end_expired_provides(now));
        if provides_ended {
            self.persist_provides().await;
        }
        if changed {
            self.persist_shares().await;
        }
        if changed || provides_ended {
            self.refresh_endpoint_later();
            self.publish().await;
        }
    }

    // ───────────── 会话事件 ─────────────

    async fn handle_session_event(&self, event: SessionEvent) {
        match event {
            SessionEvent::Opened {
                session_id,
                share_id,
                role,
                peer_endpoint_id,
            } => {
                // 共享提供方会话只在诊断簿里（遥测出口记入），不进分享 / 接收的界面状态。
                if role == SessionRole::Swarm {
                    return;
                }
                let opened_role = self.with(|i| {
                    let total_bytes = i
                        .shares
                        .iter()
                        .find(|s| s.dto.id == share_id)
                        .map(|s| s.dto.total_bytes)
                        .or_else(|| {
                            i.records
                                .iter()
                                .find(|r| r.share_id.as_deref() == Some(share_id.as_str()))
                                .map(|r| r.total_bytes)
                        })
                        .unwrap_or(0);
                    let device_name = i
                        .device_names
                        .get(&peer_endpoint_id)
                        .cloned()
                        .unwrap_or_default();
                    i.sessions.insert(
                        session_id.clone(),
                        SessionMeta {
                            share_id,
                            role,
                            peer: peer_endpoint_id,
                            path: fluxdown_share::PathKind::Unknown,
                            rate_bps: 0,
                            bytes: 0,
                            total_bytes,
                            started_at: fluxdown_share::timefmt::now_rfc3339(),
                            device_name,
                            ended: false,
                            relay_stuck: false,
                        },
                    );
                    role
                });
                match opened_role {
                    SessionRole::Receiver => self.schedule_relay_check(&session_id),
                    SessionRole::Sender => self.schedule_share_sync(),
                    SessionRole::Swarm => {}
                }
                self.publish().await;
            }
            SessionEvent::PathChanged {
                session_id, kind, ..
            } => {
                let relay = self.with(|i| {
                    if let Some(meta) = i.sessions.get_mut(&session_id) {
                        meta.path = kind;
                        if kind.is_direct() {
                            meta.relay_stuck = false;
                        }
                    }
                    kind == fluxdown_share::PathKind::Relay
                        && i.sessions
                            .get(&session_id)
                            .is_some_and(|m| m.role == SessionRole::Receiver)
                });
                if relay {
                    self.schedule_relay_check(&session_id);
                }
                self.publish_progress(&session_id);
            }
            SessionEvent::Progress {
                session_id,
                bytes,
                rate_bps,
                path_kind,
            } => {
                self.with(|i| {
                    if let Some(meta) = i.sessions.get_mut(&session_id) {
                        meta.bytes = bytes;
                        meta.rate_bps = rate_bps.max(0.0) as u64;
                        meta.path = path_kind;
                    }
                });
                self.publish_progress(&session_id);
            }
            SessionEvent::Closed { session_id, .. } => {
                let tracked = self.with(|i| {
                    let Some(meta) = i.sessions.get_mut(&session_id) else {
                        return false;
                    };
                    meta.ended = true;
                    meta.rate_bps = 0;
                    let mut ended: Vec<(String, String)> = i
                        .sessions
                        .iter()
                        .filter(|(_, m)| m.ended)
                        .map(|(id, m)| (m.started_at.clone(), id.clone()))
                        .collect();
                    ended.sort();
                    let excess = ended.len().saturating_sub(ENDED_SESSIONS_KEEP);
                    for (_, id) in ended.into_iter().take(excess) {
                        i.sessions.remove(&id);
                    }
                    true
                });
                // 共享提供方会话不进 `sessions`，只在诊断簿里；没有界面状态要刷新。
                if tracked {
                    self.publish().await;
                }
            }
            SessionEvent::SourceChanged { share_id } => self.on_source_changed(&share_id).await,
            // 共享提供的内容被改 / 删：停止提供并撤销登记。
            SessionEvent::ContentChanged { root_hash } => self.on_content_changed(&root_hash).await,
        }
    }

    fn publish_progress(&self, session_id: &str) {
        let dto = self.with(|i| {
            i.sessions.get(session_id).map(|meta| {
                let progress = if meta.total_bytes == 0 {
                    0.0
                } else {
                    (meta.bytes as f64 / meta.total_bytes as f64).clamp(0.0, 1.0)
                };
                ShareProgressDto {
                    session_id: session_id.to_owned(),
                    path_kind: convert::path_kind(meta.path),
                    rate_bps: meta.rate_bps,
                    bytes: meta.bytes,
                    progress,
                }
            })
        });
        if let Some(dto) = dto {
            self.events.publish(AgentEvent::ShareProgress(dto));
        }
    }

    /// 打洞提示：会话开始（或回落中继）`holePunchHintSecs` 后仍走中继则置 `relayStuck`。一次性计时，传输结束即无。
    fn schedule_relay_check(&self, session_id: &str) {
        let secs = self.with(|i| {
            i.config
                .as_ref()
                .map_or(0, |c| c.cfg.network.hole_punch_hint_secs)
        });
        if secs == 0 {
            return;
        }
        let weak = self.weak.clone();
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            let stuck = service.with(|i| {
                i.sessions.get_mut(&session_id).is_some_and(|meta| {
                    let stuck = !meta.ended && meta.path == fluxdown_share::PathKind::Relay;
                    meta.relay_stuck = stuck;
                    stuck
                })
            });
            if stuck {
                service.publish().await;
            }
        });
    }

    // ───────────── daemon 任务事件 ─────────────

    async fn handle_frame(&self, event: ServiceEvent) {
        let ServiceEvent::Agent(event) = event else {
            return;
        };
        match event {
            AgentEvent::Daemon(DaemonEvent::Engine(WsServerMsg::TaskProgress {
                task_id,
                status,
                ..
            })) => self.task_status(&task_id, status).await,
            AgentEvent::Daemon(DaemonEvent::TaskChanged(task)) => {
                self.task_status(&task.task_id, task.status).await;
            }
            AgentEvent::Daemon(DaemonEvent::TasksChanged(tasks)) => {
                for task in tasks {
                    self.task_status(&task.task_id, task.status).await;
                }
            }
            AgentEvent::Daemon(DaemonEvent::TaskDeleted { task_id }) => {
                self.task_gone(&[task_id]).await;
            }
            AgentEvent::Daemon(DaemonEvent::TasksDeleted { task_ids }) => {
                self.task_gone(&task_ids).await;
            }
            AgentEvent::DaemonSnapshotReplaced(_)
            | AgentEvent::Daemon(DaemonEvent::SnapshotReplaced(_)) => {
                self.reconcile_tasks().await;
            }
            AgentEvent::SessionChanged(_) => self.on_account_changed().await,
            _ => {}
        }
    }

    async fn task_status(&self, task_id: &str, status: i32) {
        let changed = self.with(|i| {
            let Some(task) = i.receive_tasks.get_mut(task_id) else {
                return false;
            };
            if task.status == status {
                return false;
            }
            task.status = status;
            let record = task.record_id.clone();
            i.dirty_records.insert(record);
            true
        });
        if changed {
            self.after_task_change().await;
        }
    }

    async fn task_gone(&self, ids: &[String]) {
        let any = self.with(|i| {
            let mut any = false;
            for id in ids {
                any |= i.drop_task(id);
            }
            any
        });
        let provides_changed = self.drop_provide_tasks(ids).await;
        if any || provides_changed {
            self.after_task_change().await;
        }
    }

    /// 与 daemon 当前任务表对账（事件丢失 / 重连 / 刚建出任务时）。
    pub(super) async fn reconcile_tasks(&self) {
        let known: HashMap<String, i32> = self.events.inspect(|snapshot| {
            snapshot
                .daemon
                .tasks
                .iter()
                .map(|t| (t.task_id.clone(), t.status))
                .collect()
        });
        let any = self.with(|i| {
            let mut changed = false;
            let mut gone = Vec::new();
            let mut dirty = Vec::new();
            for (id, task) in &mut i.receive_tasks {
                match known.get(id) {
                    Some(status) => {
                        if task.status != *status {
                            task.status = *status;
                            dirty.push(task.record_id.clone());
                            changed = true;
                        }
                    }
                    None if task.created.elapsed() > TASK_GRACE => gone.push(id.clone()),
                    None => {}
                }
            }
            i.dirty_records.extend(dirty);
            for id in gone {
                i.drop_task(&id);
                changed = true;
            }
            changed
        });
        if any {
            self.after_task_change().await;
        }
    }

    /// 任务状态变化后的收尾：移除已完成任务、关闭无人使用的会话、更新记录、调整端点。
    async fn after_task_change(&self) {
        let (closing, adapter, records_changed, completed, finished) = self.with(|i| {
            let done: Vec<String> = i
                .receive_tasks
                .iter()
                .filter(|(_, t)| t.status == 3)
                .map(|(id, _)| id.clone())
                .collect();
            // 完成的任务在移除前取出它的清单文件：共享的记录转为提供整个文件。
            let finished = done
                .iter()
                .filter_map(|id| i.finished_provide_file(id))
                .collect::<Vec<_>>();
            for id in done {
                i.drop_task(&id);
            }
            let dirty: Vec<String> = i.dirty_records.drain().collect();
            let mut records_changed = false;
            let mut completed = false;
            for record_id in dirty {
                let remaining: Vec<&ReceiveTask> = i
                    .receive_tasks
                    .values()
                    .filter(|t| t.record_id == record_id)
                    .collect();
                let status = if remaining.is_empty() {
                    ShareReceiveStatus::Completed
                } else if remaining.iter().any(|t| t.is_active()) {
                    ShareReceiveStatus::Active
                } else {
                    ShareReceiveStatus::Failed
                };
                if let Some(record) = i.records.iter_mut().find(|r| r.id == record_id)
                    && record.status != status
                {
                    record.status = status;
                    completed |= status == ShareReceiveStatus::Completed;
                    records_changed = true;
                }
            }
            // 没有任务再引用的令牌：注销并关闭会话。
            let live: std::collections::HashSet<String> =
                i.receive_tasks.values().map(|t| t.token.clone()).collect();
            let stale: Vec<String> = i
                .receive_sessions
                .keys()
                .filter(|token| i.receiving == 0 && !live.contains(*token))
                .cloned()
                .collect();
            let mut closing = Vec::with_capacity(stale.len());
            for token in stale {
                if let Some(adapter) = &i.adapter {
                    adapter.unregister(&token);
                }
                if let Some(session) = i.receive_sessions.remove(&token) {
                    closing.push(session);
                }
            }
            let adapter = if i.receive_tasks.is_empty() && i.receiving == 0 {
                i.adapter.take()
            } else {
                None
            };
            (closing, adapter, records_changed, completed, finished)
        });
        for session in closing {
            session.close().await;
        }
        if let Some(adapter) = adapter {
            adapter.shutdown().await;
        }
        if completed {
            self.schedule_usage_refresh();
        }
        if records_changed {
            self.persist_records().await;
        }
        self.add_provide_files(finished).await;
        self.refresh_endpoint().await;
        self.publish().await;
    }
}

impl Inner {
    /// 移除一个接收任务并标记其记录需要重算状态。
    pub(super) fn drop_task(&mut self, task_id: &str) -> bool {
        match self.receive_tasks.remove(task_id) {
            Some(task) => {
                self.dirty_records.insert(task.record_id);
                true
            }
            None => false,
        }
    }
}
