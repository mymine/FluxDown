//! 接收方：连接分享方、出示票据、按 4 MiB 块取数并做 BLAKE3 校验。
//!
//! [`ReceiveSession`] 是“某个分享方 + 某张票据”的逻辑会话：内部按需建立 / 重建 iroh 连接，
//! 块取数失败（流错误、校验失败）自动换一条流重试，连不上分享方时报 [`FetchError::Offline`]。
//!
//! # 多来源
//!
//! 宿主可以给会话挂上 [`ChunkSource`]（云端暂存副本 / 网关桥接）。取块时按路线依次尝试：
//! 默认直连优先，直连失败（分享方离线、传输反复出错）改走来源；宿主把偏好设为
//! [`SourcePreference::Sources`]（用户点了「切换桥接」）则来源优先、直连兜底。
//! **每个块无论来自哪条路线都按清单里的 BLAKE3 校验**；校验失败的来源被拉黑、换路线重取。
//! 路线只在块与块之间切换，已校验的块不会重新下载。
//!
//! 直连刚判定离线时进入一段冷却期：冷却期内、且有可用来源时不再为每个块白等连接超时，
//! 直接走来源；冷却结束（或来源全部失败）后再试一次直连，恢复后自动切回。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use iroh::endpoint::Connection;
use iroh::{EndpointAddr, EndpointId};
use tokio::sync::watch;

use crate::endpoint::ShareEndpoint;
use crate::hashing::{ChunkError, verify_chunk};
use crate::observe::SessionEvent;
use crate::session::{MonitorConfig, SessionMonitor, SourceAccount, close_error};
use crate::source::{
    ChunkSource, SourceBytes, SourceCounters, SourceError, SourceKind, SourcePreference,
    SourceRequest,
};
use crate::swarm::{
    ChunkSet, InFlight, PeerSlot, READY_WAIT, ROUTE_SATURATION, SwarmFile, SwarmState,
};
use crate::timefmt::{now_unix_ms, rfc3339_ms};
use crate::types::{
    PathKind, SessionBytes, SessionReport, SessionRole, SessionSummary, ShareFile, ShareManifest,
};
use crate::wire::{
    ALPN, Header, Hello, HelloReply, PROTOCOL_VERSION, Request, Status, read_frame, write_frame,
};

/// 同一个块最多尝试的次数（含首次）。
const MAX_ATTEMPTS: usize = 3;
/// 同一个来源对同一个块的尝试次数（含首次）：来源是 HTTP，一次快速重试足够。
const SOURCE_ATTEMPTS: usize = 2;
/// 保留在内存里的最近块数（让相邻的小区间请求不重复取整块）。
const CACHE_CHUNKS: usize = 4;
/// 握手回复的等待上限。
const HELLO_TIMEOUT: Duration = Duration::from_secs(20);
/// 直连判定离线后，有来源可用时多久内不再尝试直连。
const DIRECT_COOLDOWN: Duration = Duration::from_secs(20);
/// 有来源可回落时，一次直连尝试最多等多久：回落是常态路径，不该为一个离线的分享方卡住下载
/// （真实栈冒烟里 8 s 的预算让切换时首个请求停顿约 8 s，打洞 / 中继建连通常在 2–3 s 内完成）。
const SOURCE_CONNECT_BUDGET: Duration = Duration::from_secs(4);

/// 取数失败原因。
#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    /// 连不上分享方（离线、地址解析失败、超时）。
    #[error("sender is offline: {0}")]
    Offline(String),
    /// 分享方报告源文件已被修改或删除。
    #[error("the shared file was changed by the sender")]
    Changed,
    /// 分享已被停止。
    #[error("the share was stopped")]
    Stopped,
    /// 分享或票据已过期。
    #[error("the share has expired")]
    Expired,
    /// 分享或文件不存在。
    #[error("the share or file was not found")]
    NotFound,
    /// 票据被拒绝（无效、对端不符等）。
    #[error("the sender rejected the ticket: {0}")]
    Rejected(String),
    /// 同一块多次校验失败。
    #[error("chunk verification failed: {0}")]
    Corrupt(ChunkError),
    /// 传输层错误（流被重置、连接中断等），已用尽重试。
    #[error("transfer failed: {0}")]
    Transfer(String),
    /// 云端流量额度耗尽，所有可用路线都取不到数据。
    #[error("the cloud traffic quota is exhausted")]
    TrafficExhausted,
    /// 会话已被本机关闭。
    #[error("the receive session was closed")]
    Closed,
    /// 请求的文件下标或块下标不在清单里。
    #[error("no such file or chunk in the manifest")]
    OutOfRange,
}

impl FetchError {
    /// 换一条流 / 换一次连接重试可能成功。
    fn retryable(&self) -> bool {
        matches!(self, Self::Transfer(_) | Self::Corrupt(_))
    }

    /// 换路线也没有意义：分享本身已变 / 已结束，或会话已关闭。
    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Changed
                | Self::Stopped
                | Self::Expired
                | Self::NotFound
                | Self::Rejected(_)
                | Self::Closed
                | Self::OutOfRange
        )
    }

    /// 多条路线都失败时报告哪一个：额度耗尽（需要用户知道）> 离线 > 其余。
    fn rank(&self) -> u8 {
        match self {
            Self::TrafficExhausted => 2,
            Self::Offline(_) => 1,
            _ => 0,
        }
    }
}

/// 建立接收会话的参数。
#[derive(Debug, Clone)]
pub struct ReceiveParams {
    /// 清单（来自 `POST /share/resolve`，块哈希是校验的唯一依据）。
    pub manifest: ShareManifest,
    /// 票据原文。
    pub ticket: String,
    /// 分享方 endpoint ID；缺省地址时通过 DNS 地址查找解析。
    pub sender: EndpointId,
    /// 可选的地址提示（局域网发现、测试等）；提供时优先使用。
    pub addr_hint: Option<EndpointAddr>,
    /// 单次连接 + 握手的总超时。
    pub connect_timeout: Duration,
}

impl ReceiveParams {
    /// 只按 endpoint ID 连接（靠地址查找）。
    pub fn new(manifest: ShareManifest, ticket: String, sender: EndpointId) -> Self {
        Self {
            manifest,
            ticket,
            sender,
            addr_hint: None,
            connect_timeout: Duration::from_secs(25),
        }
    }
}

struct Live {
    conn: Connection,
    monitor: SessionMonitor,
}

#[derive(Default)]
struct Cache {
    chunks: VecDeque<((u32, u64), Bytes)>,
}

/// 一个已挂载的来源及其「本会话内已拉黑」标记。
#[derive(Clone)]
struct SourceSlot {
    source: Arc<dyn ChunkSource>,
    banned: Arc<AtomicBool>,
}

/// 来源相关的会话状态。
#[derive(Default)]
struct SourceState {
    slots: Vec<SourceSlot>,
    preference: SourcePreference,
    /// 直连冷却截止时刻（判定离线后设置，成功取块后清除）。
    direct_down_until: Option<Instant>,
}

struct Inner {
    endpoint: ShareEndpoint,
    params: ReceiveParams,
    live: tokio::sync::Mutex<Option<Live>>,
    kind: Arc<watch::Sender<PathKind>>,
    cache: Mutex<Cache>,
    closed: AtomicBool,
    counter: AtomicU64,
    last_summary: Mutex<Option<SessionSummary>>,
    /// 来源路线状态。
    sources: Mutex<SourceState>,
    /// 各来源取回并通过校验的字节（会话级，跨重连）。
    source_bytes: Arc<SourceCounters>,
    /// 已经计入某个终态报告的来源字节。
    reported: Mutex<SourceBytes>,
    /// 已经计入某个终态报告的逐对端字节。
    reported_peers: Mutex<HashMap<String, u64>>,
    /// 来源块的校验失败次数（直连的失败由连接监测器统计）。
    source_verify_failures: AtomicU64,
    /// 最近一个交付块的来源：`None` = iroh（直连 / 中继）。
    active: watch::Sender<Option<SourceKind>>,
    started_unix_ms: u64,
    /// 共享对端状态与调度。
    swarm: Arc<SwarmState>,
    /// 正在走直连的块请求数（与对端的负载一起决定路线顺序）。
    direct_inflight: AtomicUsize,
    /// 原分享方已不再提供的文件（改动 / 删除）：之后不再走直连。
    origin_lacks: Mutex<HashSet<u32>>,
}

impl Drop for Inner {
    /// 最后一个句柄被丢弃而宿主没有调用 `close()` 时，仍然收尾并发出终态报告。
    fn drop(&mut self) {
        self.swarm.shutdown();
        if let Some(live) = self.live.get_mut().take() {
            let summary = live.monitor.finish(None);
            self.mark_reported(&summary);
            live.conn.close(0u32.into(), b"dropped");
        }
        self.flush_source_report();
    }
}

impl Inner {
    fn mark_reported(&self, summary: &SessionSummary) {
        *self
            .reported_peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self.source_bytes.per_peer_all();
        let mut reported = self.reported.lock().unwrap_or_else(PoisonError::into_inner);
        reported.cdn += summary.bytes.cdn;
        reported.bridge += summary.bytes.bridge;
        reported.peer += summary.bytes.peer;
    }

    /// 没有（或不再有）连接监测器承载的来源字节：补发一条终态会话报告，保证计量不丢。
    fn flush_source_report(&self) {
        let total = self.source_bytes.snapshot();
        let peer_bytes = {
            let mut rp = self
                .reported_peers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let delta = self.source_bytes.per_peer_delta(&rp);
            *rp = self.source_bytes.per_peer_all();
            delta
        };
        let pending = {
            let mut reported = self.reported.lock().unwrap_or_else(PoisonError::into_inner);
            let pending = SourceBytes {
                cdn: total.cdn.saturating_sub(reported.cdn),
                bridge: total.bridge.saturating_sub(reported.bridge),
                peer: total.peer.saturating_sub(reported.peer),
            };
            if pending == SourceBytes::default() {
                return;
            }
            *reported = total;
            pending
        };
        let session_id = format!(
            "{}-{:x}-src{:x}",
            self.endpoint.id().fmt_short(),
            now_unix_ms(),
            self.counter.fetch_add(1, Ordering::Relaxed)
        );
        let report = SessionReport {
            session_id,
            share_id: self.params.manifest.share_id.clone(),
            role: SessionRole::Receiver,
            peer_endpoint_id: self.params.sender.to_z32(),
            summary: SessionSummary {
                started_at: rfc3339_ms(self.started_unix_ms),
                connect_ms: 0,
                first_direct_ms: None,
                path_kind: PathKind::Unknown,
                bytes: SessionBytes {
                    direct: 0,
                    relay: 0,
                    cdn: pending.cdn,
                    bridge: pending.bridge,
                    peer: pending.peer,
                },
                paths: Vec::new(),
                events: Vec::new(),
                verify_failures: self.source_verify_failures.load(Ordering::Relaxed),
                error: None,
            },
            samples: Vec::new(),
            swarm_files: Vec::new(),
            peer_bytes,
            is_final: true,
        };
        self.endpoint.observers().report_session(&report);
    }
}

/// 接收会话。可克隆，克隆体共享连接与缓存。
#[derive(Clone)]
pub struct ReceiveSession {
    inner: Arc<Inner>,
}

/// 取一个块可走的路线。
enum Route {
    Direct,
    Source(SourceSlot),
    Peer(Arc<PeerSlot>),
}

impl ReceiveSession {
    /// 创建会话。**不立即连接**：第一次取数（或 [`Self::probe`]）时才连。
    pub fn new(endpoint: ShareEndpoint, params: ReceiveParams) -> Self {
        Self {
            inner: Arc::new(Inner {
                swarm: Arc::new(SwarmState::new(endpoint.clone(), &params.manifest)),
                direct_inflight: AtomicUsize::new(0),
                origin_lacks: Mutex::new(HashSet::new()),
                endpoint,
                params,
                live: tokio::sync::Mutex::new(None),
                kind: Arc::new(watch::channel(PathKind::Unknown).0),
                cache: Mutex::new(Cache::default()),
                closed: AtomicBool::new(false),
                counter: AtomicU64::new(0),
                last_summary: Mutex::new(None),
                sources: Mutex::new(SourceState::default()),
                source_bytes: Arc::new(SourceCounters::default()),
                reported: Mutex::new(SourceBytes::default()),
                reported_peers: Mutex::new(HashMap::new()),
                source_verify_failures: AtomicU64::new(0),
                active: watch::channel(None).0,
                started_unix_ms: now_unix_ms(),
            }),
        }
    }

    /// 清单。
    pub fn manifest(&self) -> &ShareManifest {
        &self.inner.params.manifest
    }

    /// 接收票据原文（宿主向云端换取 CDN / 桥接地址时一并出示）。
    pub fn ticket(&self) -> &str {
        &self.inner.params.ticket
    }

    /// 当前来源路径类别（任务行徽标：直连·局域网 / 直连·公网 / 中继）。
    pub fn path_kind(&self) -> PathKind {
        *self.inner.kind.borrow()
    }

    // ───────────── 来源 ─────────────

    /// 挂上一个 HTTP 区间来源；同种类的旧来源被替换（并清除其拉黑状态）。
    /// 路线顺序固定：云端副本在桥接之前。
    pub fn set_source(&self, source: Arc<dyn ChunkSource>) {
        let mut state = self.lock_sources();
        let kind = source.kind();
        state.slots.retain(|slot| slot.source.kind() != kind);
        state.slots.push(SourceSlot {
            source,
            banned: Arc::new(AtomicBool::new(false)),
        });
        state.slots.sort_by_key(|slot| slot.source.kind());
    }

    /// 当前挂着且未被拉黑的来源种类（按路线顺序）。
    pub fn source_kinds(&self) -> Vec<SourceKind> {
        self.lock_sources()
            .slots
            .iter()
            .filter(|slot| !slot.banned.load(Ordering::Relaxed))
            .map(|slot| slot.source.kind())
            .collect()
    }

    /// 设置路线偏好（「切换桥接」= [`SourcePreference::Sources`]）。
    pub fn set_preference(&self, preference: SourcePreference) {
        self.lock_sources().preference = preference;
    }

    /// 当前路线偏好。
    pub fn preference(&self) -> SourcePreference {
        self.lock_sources().preference
    }

    /// 最近一个交付块的来源（`None` = iroh 直连 / 中继，或还没有块交付）。
    pub fn active_source(&self) -> Option<SourceKind> {
        *self.inner.active.borrow()
    }

    /// 订阅「最近交付块的来源」的变化。
    pub fn watch_source(&self) -> watch::Receiver<Option<SourceKind>> {
        self.inner.active.subscribe()
    }

    /// 各来源已取回并通过校验的字节数（会话累计）。
    pub fn source_bytes(&self) -> SourceBytes {
        self.inner.source_bytes.snapshot()
    }

    // ───────────── 共享 ─────────────

    /// 设置 / 更新共享对端与票据（FluxCloud `swarm/peers` 的结果），并确保后台维护任务在运行。
    /// 重复调用会合并：已知对端更新状态，新对端追加，云端列表里已不在线且未连接的对端被忘掉。
    pub fn set_swarm(&self, files: Vec<SwarmFile>) {
        self.inner.swarm.update(files);
        self.inner.swarm.start();
    }

    /// 当前就绪的共享对端数（已连接且至少一个文件已授权）。
    pub fn swarm_peers(&self) -> u32 {
        self.inner.swarm.connected_count()
    }

    /// 订阅就绪共享对端数的变化。
    pub fn watch_swarm_peers(&self) -> watch::Receiver<u32> {
        self.inner.swarm.subscribe_connected()
    }

    /// 第 `file_index` 个文件本会话已取回并校验的块位图（宿主据此向别人提供下载中的内容）；下标越界为 `None`。
    pub fn have(&self, file_index: u32) -> Option<Arc<ChunkSet>> {
        self.inner.swarm.local_have(file_index)
    }

    fn origin_lacks(&self, file_index: u32) -> bool {
        self.inner
            .origin_lacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&file_index)
    }

    fn mark_origin_lacks(&self, file_index: u32) {
        self.inner
            .origin_lacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(file_index);
    }

    /// 有回落路线可用：HTTP 来源，或共享对端。
    fn has_fallback(&self) -> bool {
        self.has_usable_source() || self.inner.swarm.has_peers()
    }

    fn lock_sources(&self) -> std::sync::MutexGuard<'_, SourceState> {
        self.inner
            .sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// 本块的路线顺序。
    ///
    /// 来源优先（且有可用 HTTP 来源）：来源、对端、直连。否则直连与对端按负载混排
    /// （`(是否饱和, 负载, 序号)`，直连序号 0），再接 HTTP 来源；直连被排除（冷却 / 原分享方已缺失）时，
    /// 冷却中的直连仍作为最后兜底。
    fn routes(&self, file_index: u32, chunk_index: u64) -> Vec<Route> {
        let (usable, cooling, preference) = {
            let state = self.lock_sources();
            let usable: Vec<SourceSlot> = state
                .slots
                .iter()
                .filter(|slot| !slot.banned.load(Ordering::Relaxed))
                .cloned()
                .collect();
            let cooling = state
                .direct_down_until
                .is_some_and(|until| Instant::now() < until);
            (usable, cooling, state.preference)
        };
        let candidates = self.inner.swarm.candidates(file_index, chunk_index);
        let origin_lacks = self.origin_lacks(file_index);
        if preference == SourcePreference::Sources && !usable.is_empty() {
            let mut routes: Vec<Route> = usable.into_iter().map(Route::Source).collect();
            routes.extend(candidates.into_iter().map(Route::Peer));
            if !origin_lacks {
                routes.push(Route::Direct);
            }
            return routes;
        }
        let has_fallback = !usable.is_empty() || !candidates.is_empty();
        let direct_ok = !origin_lacks && !(cooling && has_fallback);
        let mut p2p: Vec<(usize, usize, Route)> = Vec::with_capacity(candidates.len() + 1);
        if direct_ok {
            p2p.push((
                self.inner.direct_inflight.load(Ordering::Relaxed),
                0,
                Route::Direct,
            ));
        }
        for (i, peer) in candidates.into_iter().enumerate() {
            p2p.push((peer.load(), i + 1, Route::Peer(peer)));
        }
        p2p.sort_by_key(|(load, ordinal, _)| (*load >= ROUTE_SATURATION, *load, *ordinal));
        let mut routes: Vec<Route> = p2p.into_iter().map(|(_, _, route)| route).collect();
        routes.extend(usable.into_iter().map(Route::Source));
        if !direct_ok && !origin_lacks {
            routes.push(Route::Direct);
        }
        routes
    }

    fn has_usable_source(&self) -> bool {
        self.lock_sources()
            .slots
            .iter()
            .any(|slot| !slot.banned.load(Ordering::Relaxed))
    }

    fn set_direct_down(&self, down: bool) {
        self.lock_sources().direct_down_until = down.then(|| Instant::now() + DIRECT_COOLDOWN);
    }

    /// 直连刚判定离线、仍在冷却，且有回落路线（来源 / 对端）：此时不值得再等一次连接。
    fn direct_cooling_with_fallback(&self) -> bool {
        let cooling = self
            .lock_sources()
            .direct_down_until
            .is_some_and(|until| Instant::now() < until);
        cooling && self.has_fallback()
    }

    fn set_active(&self, source: Option<SourceKind>) {
        self.inner.active.send_if_modified(|current| {
            if *current == source {
                false
            } else {
                *current = source;
                true
            }
        });
    }

    // ───────────── 摘要 / 关闭 ─────────────

    /// 会话摘要：连接存活时是实时快照，否则是最近一次连接的终态；来源字节是整个会话的累计。
    pub async fn summary(&self) -> Option<SessionSummary> {
        let live = self
            .inner
            .live
            .lock()
            .await
            .as_ref()
            .map(|live| live.monitor.summary(None));
        let base = live.or_else(|| {
            self.inner
                .last_summary
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        });
        let total = self.inner.source_bytes.snapshot();
        let verify_failures = self.inner.source_verify_failures.load(Ordering::Relaxed);
        match base {
            Some(mut summary) => {
                summary.bytes.cdn = total.cdn;
                summary.bytes.bridge = total.bridge;
                summary.bytes.peer = total.peer;
                summary.verify_failures += verify_failures;
                Some(summary)
            }
            None if total != SourceBytes::default() => Some(SessionSummary {
                started_at: rfc3339_ms(self.inner.started_unix_ms),
                connect_ms: 0,
                first_direct_ms: None,
                path_kind: PathKind::Unknown,
                bytes: SessionBytes {
                    direct: 0,
                    relay: 0,
                    cdn: total.cdn,
                    bridge: total.bridge,
                    peer: total.peer,
                },
                paths: Vec::new(),
                events: Vec::new(),
                verify_failures,
                error: None,
            }),
            None => None,
        }
    }

    /// 关闭会话：结束监测、关闭连接。之后的取数返回 [`FetchError::Closed`]。
    pub async fn close(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.swarm.shutdown();
        let live = self.inner.live.lock().await.take();
        if let Some(live) = live {
            self.retire(live, None);
        }
        self.inner.flush_source_report();
    }

    fn retire(&self, live: Live, error: Option<String>) {
        let summary = live.monitor.finish(error);
        self.inner.mark_reported(&summary);
        *self
            .inner
            .last_summary
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(summary);
        live.conn.close(0u32.into(), b"done");
    }

    // ───────────── 探测 ─────────────

    /// 探测分享方状态（零字节请求）：用于 HEAD 与启动前检查。成功说明分享方在线且文件未变。
    ///
    /// 挂着可用来源时，分享方离线不算失败（块可以从来源取）；来源优先时直接放行。
    /// 探测到分享已被改动 / 停止 / 过期时顺便丢弃块缓存，避免之后还从缓存吐出旧数据。
    pub async fn probe(&self, file_index: u32) -> Result<(), FetchError> {
        self.file(file_index)?;
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(FetchError::Closed);
        }
        if self.origin_lacks(file_index) && self.inner.swarm.has_peers_for(file_index) {
            return Ok(());
        }
        let sources_first = {
            let state = self.lock_sources();
            state.preference == SourcePreference::Sources
        } && self.has_usable_source();
        if sources_first {
            return Ok(());
        }
        let result = self.probe_inner(file_index).await;
        match &result {
            Err(FetchError::Changed | FetchError::NotFound)
                if self.inner.swarm.has_peers_for(file_index) =>
            {
                self.mark_origin_lacks(file_index);
                return Ok(());
            }
            Err(e) if e.terminal() => self.forget_on_terminal(e),
            Err(FetchError::Offline(_)) if self.has_fallback() => {
                self.set_direct_down(true);
                return Ok(());
            }
            Err(_) => {}
            Ok(()) => self.set_direct_down(false),
        }
        result
    }

    async fn probe_inner(&self, file_index: u32) -> Result<(), FetchError> {
        let file = self.file(file_index)?;
        let mut last = FetchError::Closed;
        for _ in 0..MAX_ATTEMPTS {
            let live = self.live().await?;
            match self.request(&live, file, file_index, 0, 0).await {
                Ok(_) => return Ok(()),
                Err(e) if e.retryable() => {
                    self.drop_if_dead(&live.conn).await;
                    last = e;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// 与 [`Self::fetch_chunk`] 相同，但命中缓存时先向分享方确认状态（一次零字节请求）。
    ///
    /// 一个新的下载请求的第一块用它：保证“分享方离线 / 文件被改 / 分享已停止”立即反映出来，
    /// 不会被缓存掩盖；同一请求里后续的块沿用缓存即可。
    pub async fn fetch_chunk_revalidated(
        &self,
        file_index: u32,
        chunk_index: u64,
    ) -> Result<Bytes, FetchError> {
        if self.cached(file_index, chunk_index).is_some() {
            self.probe(file_index).await?;
        }
        self.fetch_chunk(file_index, chunk_index).await
    }

    fn forget_on_terminal(&self, error: &FetchError) {
        if matches!(
            error,
            FetchError::Changed
                | FetchError::Stopped
                | FetchError::Expired
                | FetchError::NotFound
                | FetchError::Rejected(_)
        ) {
            self.inner
                .cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .chunks
                .clear();
        }
    }

    // ───────────── 取块 ─────────────

    /// 取并校验第 `chunk_index` 块：直连、共享对端、HTTP 来源互为回落，按负载与偏好排序依次尝试。
    pub async fn fetch_chunk(
        &self,
        file_index: u32,
        chunk_index: u64,
    ) -> Result<Bytes, FetchError> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(FetchError::Closed);
        }
        let file = self.file(file_index)?;
        let (start, end) = file
            .chunk_range(chunk_index)
            .ok_or(FetchError::OutOfRange)?;
        if let Some(hit) = self.cached(file_index, chunk_index) {
            return Ok(hit);
        }
        let mut worst: Option<FetchError> = None;
        let mut waited = false;
        loop {
            for route in self.routes(file_index, chunk_index) {
                let result = match &route {
                    Route::Direct => {
                        let _flight = InFlight::new(&self.inner.direct_inflight);
                        self.fetch_direct(file, file_index, chunk_index, start, end)
                            .await
                    }
                    Route::Source(slot) => {
                        self.fetch_from_source(slot, file, file_index, chunk_index, start, end)
                            .await
                    }
                    Route::Peer(peer) => {
                        self.fetch_from_peer(peer, file, file_index, chunk_index, start, end)
                            .await
                    }
                };
                match result {
                    Ok((data, source)) => {
                        self.remember(file_index, chunk_index, &data);
                        // 对端交付不改变「最近交付块的来源」，避免任务行徽标抖动。
                        if !matches!(route, Route::Peer(_)) {
                            self.set_active(source);
                        }
                        if let Some(have) = self.inner.swarm.local_have(file_index) {
                            have.insert(chunk_index);
                        }
                        return Ok(data);
                    }
                    Err(e)
                        if matches!(route, Route::Direct)
                            && matches!(e, FetchError::Changed | FetchError::NotFound)
                            && self.inner.swarm.has_peers_for(file_index) =>
                    {
                        tracing::info!(file_index, error = %e, "origin no longer serves this file; using swarm peers");
                        self.mark_origin_lacks(file_index);
                        if worst.is_none() {
                            worst = Some(e);
                        }
                    }
                    Err(e) if e.terminal() => {
                        self.forget_on_terminal(&e);
                        return Err(e);
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "chunk route failed; trying the next one");
                        if worst.as_ref().is_none_or(|w| e.rank() >= w.rank()) {
                            worst = Some(e);
                        }
                    }
                }
            }
            if !waited && self.inner.swarm.pending_for(file_index) {
                waited = true;
                self.inner.swarm.wait_ready(READY_WAIT).await;
                if self.inner.closed.load(Ordering::SeqCst) {
                    return Err(FetchError::Closed);
                }
                continue;
            }
            break;
        }
        Err(worst
            .unwrap_or_else(|| FetchError::Offline("no route can serve this chunk yet".to_owned())))
    }

    /// 共享对端取块：成功后按对端字节计量；坏块计入校验失败。
    async fn fetch_from_peer(
        &self,
        peer: &Arc<PeerSlot>,
        file: &ShareFile,
        file_index: u32,
        chunk_index: u64,
        start: u64,
        end: u64,
    ) -> Result<(Bytes, Option<SourceKind>), FetchError> {
        match self
            .inner
            .swarm
            .fetch(peer, file, file_index, chunk_index, start, end)
            .await
        {
            Ok(data) => {
                self.inner
                    .source_bytes
                    .add_peer(&peer.id().to_z32(), data.len());
                Ok((data, None))
            }
            Err(e) => {
                if matches!(e, FetchError::Corrupt(_)) {
                    self.inner
                        .source_verify_failures
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(e)
            }
        }
    }

    /// 直连取块：失败时换一条流重试，最多 3 次。
    async fn fetch_direct(
        &self,
        file: &ShareFile,
        file_index: u32,
        chunk_index: u64,
        start: u64,
        end: u64,
    ) -> Result<(Bytes, Option<SourceKind>), FetchError> {
        let mut last = FetchError::Closed;
        for attempt in 0..MAX_ATTEMPTS {
            let live = match self.live().await {
                Ok(live) => live,
                Err(e) => {
                    if matches!(e, FetchError::Offline(_)) {
                        self.set_direct_down(true);
                    }
                    return Err(e);
                }
            };
            let result = self
                .request(&live, file, file_index, start, end - start)
                .await;
            let result = result.and_then(|data| {
                verify_chunk(file, chunk_index, &data).map_err(|e| {
                    live.monitor.add_verify_failure();
                    FetchError::Corrupt(e)
                })?;
                Ok(data)
            });
            match result {
                Ok(data) => {
                    live.monitor.add_bytes(data.len() as u64);
                    self.set_direct_down(false);
                    return Ok((data, None));
                }
                Err(e) if e.retryable() => {
                    tracing::debug!(attempt, error = %e, "chunk fetch failed, retrying on a new stream");
                    self.drop_if_dead(&live.conn).await;
                    last = e;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// 来源取块：一次快速重试；校验失败两次或来源明确拒绝即拉黑该来源。
    async fn fetch_from_source(
        &self,
        slot: &SourceSlot,
        file: &ShareFile,
        file_index: u32,
        chunk_index: u64,
        start: u64,
        end: u64,
    ) -> Result<(Bytes, Option<SourceKind>), FetchError> {
        let kind = slot.source.kind();
        let mut last = FetchError::Transfer(format!("{} source gave no data", kind.as_str()));
        for attempt in 0..SOURCE_ATTEMPTS {
            if slot.banned.load(Ordering::Relaxed) {
                break;
            }
            let fetched = slot
                .source
                .fetch(SourceRequest {
                    file_index,
                    offset: start,
                    len: end - start,
                })
                .await;
            match fetched {
                Ok(data) => match verify_chunk(file, chunk_index, &data) {
                    Ok(()) => {
                        self.inner.source_bytes.add(kind, data.len() as u64);
                        return Ok((data, Some(kind)));
                    }
                    Err(e) => {
                        self.inner
                            .source_verify_failures
                            .fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(source = kind.as_str(), attempt, error = %e, "source chunk failed verification");
                        last = FetchError::Corrupt(e);
                    }
                },
                Err(SourceError::Unavailable(message)) => {
                    tracing::debug!(source = kind.as_str(), attempt, %message, "source unavailable");
                    last = FetchError::Transfer(message);
                }
                Err(SourceError::Refused(message)) => {
                    tracing::info!(source = kind.as_str(), %message, "source refused; not using it again");
                    slot.banned.store(true, Ordering::Relaxed);
                    return Err(FetchError::Transfer(message));
                }
                Err(SourceError::TrafficExhausted) => return Err(FetchError::TrafficExhausted),
                Err(SourceError::Changed) => return Err(FetchError::Changed),
                Err(SourceError::Ended) => return Err(FetchError::Expired),
            }
        }
        if matches!(last, FetchError::Corrupt(_)) {
            tracing::warn!(
                source = kind.as_str(),
                "source keeps returning corrupt data; not using it again"
            );
            slot.banned.store(true, Ordering::Relaxed);
        }
        Err(last)
    }

    fn file(&self, file_index: u32) -> Result<&ShareFile, FetchError> {
        usize::try_from(file_index)
            .ok()
            .and_then(|i| self.inner.params.manifest.files.get(i))
            .ok_or(FetchError::OutOfRange)
    }

    fn cached(&self, file_index: u32, chunk: u64) -> Option<Bytes> {
        let cache = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache
            .chunks
            .iter()
            .find(|(k, _)| *k == (file_index, chunk))
            .map(|(_, b)| b.clone())
    }

    fn remember(&self, file_index: u32, chunk: u64, data: &Bytes) {
        let mut cache = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.chunks.retain(|(k, _)| *k != (file_index, chunk));
        if cache.chunks.len() >= CACHE_CHUNKS {
            cache.chunks.pop_front();
        }
        cache.chunks.push_back(((file_index, chunk), data.clone()));
    }

    /// 连接已死则丢弃（下次取数会重连）。
    async fn drop_if_dead(&self, conn: &Connection) {
        let Some(reason) = conn.close_reason() else {
            return;
        };
        let mut guard = self.inner.live.lock().await;
        if guard
            .as_ref()
            .is_some_and(|l| l.conn.stable_id() == conn.stable_id())
            && let Some(live) = guard.take()
        {
            drop(guard);
            self.retire(live, close_error(&reason));
        }
    }

    /// 取得可用连接：存活则复用，否则（重新）连接并完成 Hello。
    async fn live(&self) -> Result<LiveHandle, FetchError> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(FetchError::Closed);
        }
        let mut guard = self.inner.live.lock().await;
        if let Some(live) = guard.as_ref()
            && live.conn.close_reason().is_none()
        {
            return Ok(LiveHandle {
                conn: live.conn.clone(),
                monitor: live.monitor.clone(),
            });
        }
        if let Some(dead) = guard.take() {
            let error = dead.conn.close_reason().and_then(|r| close_error(&r));
            self.retire(dead, error);
        }
        if self.direct_cooling_with_fallback() {
            return Err(FetchError::Offline(
                "direct path is cooling down after a failed connect".to_owned(),
            ));
        }
        let fresh = self.connect().await?;
        let handle = LiveHandle {
            conn: fresh.conn.clone(),
            monitor: fresh.monitor.clone(),
        };
        *guard = Some(fresh);
        Ok(handle)
    }

    async fn connect(&self) -> Result<Live, FetchError> {
        let params = &self.inner.params;
        let started = Instant::now();
        let attempt = async {
            let endpoint = self
                .inner
                .endpoint
                .ensure_bound()
                .await
                .map_err(|e| FetchError::Offline(format!("{e:#}")))?;
            let target = params
                .addr_hint
                .clone()
                .unwrap_or_else(|| EndpointAddr::new(params.sender));
            let conn = endpoint
                .connect(target, ALPN)
                .await
                .map_err(|e| FetchError::Offline(format!("{e:#}")))?;
            let connect_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            Ok::<_, FetchError>((conn, connect_ms))
        };
        let budget = if self.has_fallback() {
            params.connect_timeout.min(SOURCE_CONNECT_BUDGET)
        } else {
            params.connect_timeout
        };
        let (conn, connect_ms) = tokio::time::timeout(budget, attempt)
            .await
            .map_err(|_| FetchError::Offline("connect timed out".to_owned()))??;

        if let Err(e) = self.hello(&conn).await {
            conn.close(0u32.into(), b"hello failed");
            return Err(e);
        }

        // 此前只靠来源交付的字节不属于这条新连接：先单独报告，连接的摘要从当前累计起算。
        self.inner.flush_source_report();
        let session_id = format!(
            "{}-{:x}-{:x}",
            self.inner.endpoint.id().fmt_short(),
            now_unix_ms(),
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        );
        let peer = params.sender.to_z32();
        let monitor = SessionMonitor::spawn(
            conn.clone(),
            MonitorConfig {
                session_id: session_id.clone(),
                share_id: params.manifest.share_id.clone(),
                role: SessionRole::Receiver,
                peer_endpoint_id: peer.clone(),
                connect_ms,
                report_interval: self.inner.endpoint.report_interval(),
                observers: self.inner.endpoint.observers().clone(),
                kind_tx: self.inner.kind.clone(),
                sources: Some(SourceAccount {
                    counters: self.inner.source_bytes.clone(),
                    base: *self
                        .inner
                        .reported
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner),
                    base_peers: self
                        .inner
                        .reported_peers
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone(),
                }),
            },
        );
        self.inner.endpoint.observers().emit(SessionEvent::Opened {
            session_id,
            share_id: params.manifest.share_id.clone(),
            role: SessionRole::Receiver,
            peer_endpoint_id: peer,
        });
        Ok(Live { conn, monitor })
    }

    async fn hello(&self, conn: &Connection) -> Result<(), FetchError> {
        let transfer = |e: anyhow::Error| FetchError::Offline(format!("{e:#}"));
        let exchange = async {
            let (mut send, mut recv) =
                conn.open_bi().await.map_err(|e| anyhow::anyhow!("{e:#}"))?;
            write_frame(
                &mut send,
                &Hello {
                    v: PROTOCOL_VERSION,
                    ticket: self.inner.params.ticket.clone(),
                },
            )
            .await?;
            send.finish().map_err(|e| anyhow::anyhow!("{e:#}"))?;
            read_frame::<HelloReply>(&mut recv).await
        };
        let reply = tokio::time::timeout(HELLO_TIMEOUT, exchange)
            .await
            .map_err(|_| FetchError::Offline("hello timed out".to_owned()))?
            .map_err(transfer)?;
        match reply.status {
            Status::Ok => Ok(()),
            other => Err(map_status(other, &reply.message)),
        }
    }

    /// 在一条新的双向流上发一个请求，返回原始（未校验）字节。
    async fn request(
        &self,
        live: &LiveHandle,
        file: &ShareFile,
        file_index: u32,
        offset: u64,
        len: u64,
    ) -> Result<Bytes, FetchError> {
        let _active = live.monitor.activity();
        let io = |e: anyhow::Error| FetchError::Transfer(format!("{e:#}"));
        let (mut send, mut recv) = live
            .conn
            .open_bi()
            .await
            .map_err(|e| FetchError::Transfer(format!("{e:#}")))?;
        write_frame(
            &mut send,
            &Request {
                file_index,
                offset,
                len,
            },
        )
        .await
        .map_err(io)?;
        send.finish()
            .map_err(|e| FetchError::Transfer(format!("{e:#}")))?;
        let header: Header = read_frame(&mut recv).await.map_err(io)?;
        if header.status != Status::Ok {
            return Err(map_status(header.status, ""));
        }
        if header.file_size != file.size {
            return Err(FetchError::Changed);
        }
        if header.len != len {
            return Err(FetchError::Transfer(format!(
                "sender announced {} bytes for a {len}-byte request",
                header.len
            )));
        }
        let mut data = vec![0u8; usize::try_from(len).map_err(|_| FetchError::OutOfRange)?];
        recv.read_exact(&mut data)
            .await
            .map_err(|e| FetchError::Transfer(format!("{e:#}")))?;
        Ok(Bytes::from(data))
    }
}

struct LiveHandle {
    conn: Connection,
    monitor: SessionMonitor,
}

fn map_status(status: Status, message: &str) -> FetchError {
    match status {
        Status::Ok => FetchError::Transfer("unexpected ok status".to_owned()),
        Status::NotFound => FetchError::NotFound,
        Status::Changed => FetchError::Changed,
        Status::Stopped => FetchError::Stopped,
        Status::Expired => FetchError::Expired,
        Status::BadTicket | Status::PeerMismatch => {
            FetchError::Rejected(format!("{status:?}: {message}"))
        }
        Status::BadRequest | Status::Internal => {
            FetchError::Transfer(format!("{status:?}: {message}"))
        }
    }
}
