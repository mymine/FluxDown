//! 会话监测：路径事件时间线、逐路径统计、传输中定时采样与终态汇总。
//!
//! 发送方与接收方共用同一份实现。静默约束：
//! - 路径事件订阅是事件驱动的，没有定时器；
//! - 采样定时器只在“有传输流进行中”（活动计数 > 0）时存在，活动归零立即销毁；
//! - 会话结束（连接关闭）后任务退出。

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use iroh::TransportAddr;
use iroh::endpoint::{Connection, ConnectionError, PathEvent};
use tokio::sync::watch;
use tokio_stream::StreamExt as _;
use tokio_util::sync::CancellationToken;

use crate::observe::{Observers, SessionEvent};
use crate::source::{SourceBytes, SourceCounters};
use crate::timefmt::{now_unix_ms, rfc3339_ms};
use crate::types::{
    MAX_SWARM_FILES, PathEventEntry, PathKind, PathSummary, SessionBytes, SessionReport,
    SessionRole, SessionSample, SessionSummary, SwarmServedFile,
};

/// 时间线最多保留的事件数。
const MAX_EVENTS: usize = 300;
/// 两次上报之间最多缓存的采样数。
const MAX_PENDING_SAMPLES: usize = 600;

/// 把传输地址归类。内网（RFC1918 / 链路本地 / ULA / 回环）为局域网直连，其余 IP 为公网直连。
pub fn classify_addr(addr: &TransportAddr) -> PathKind {
    match addr {
        TransportAddr::Relay(_) => PathKind::Relay,
        TransportAddr::Ip(sa) => {
            if is_lan(sa.ip()) {
                PathKind::DirectLan
            } else {
                PathKind::DirectWan
            }
        }
        _ => PathKind::Unknown,
    }
}

fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let first = v6.segments().first().copied().unwrap_or(0);
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

fn remote_text(addr: &TransportAddr) -> String {
    match addr {
        TransportAddr::Relay(url) => url.to_string(),
        TransportAddr::Ip(sa) => sa.to_string(),
        other => format!("{other:?}"),
    }
}

/// 会话静态参数。
pub(crate) struct MonitorConfig {
    pub session_id: String,
    pub share_id: String,
    pub role: SessionRole,
    pub peer_endpoint_id: String,
    pub connect_ms: u64,
    pub report_interval: Duration,
    pub observers: Observers,
    /// 当前路径类别的广播（接收方用它做任务来源徽标）。
    pub kind_tx: Arc<watch::Sender<PathKind>>,
    /// 接收方：会话级来源计量与本连接的起算值（发送方为 `None`）。
    pub sources: Option<SourceAccount>,
}

/// 把会话级（跨重连）的来源字节折进本连接的摘要：只计起算值之后新增的部分。
pub(crate) struct SourceAccount {
    pub counters: Arc<SourceCounters>,
    pub base: SourceBytes,
    pub base_peers: std::collections::HashMap<String, u64>,
}

#[derive(Default)]
struct State {
    selected_id: Option<String>,
    selected_kind: PathKind,
    selected_remote: String,
    first_direct_ms: Option<u64>,
    paths: BTreeMap<String, PathSummary>,
    events: Vec<PathEventEntry>,
    pending: Vec<SessionSample>,
    last_bytes: u64,
    last_at: Option<Instant>,
}

struct Inner {
    cfg: MonitorConfig,
    conn: Connection,
    started: Instant,
    started_unix_ms: u64,
    bytes: AtomicU64,
    verify_failures: AtomicU64,
    active: watch::Sender<usize>,
    activations: AtomicU64,
    state: Mutex<State>,
    /// 共享提供方：按内容累计的服务量（键为根哈希 hex）。
    served: Mutex<BTreeMap<String, SwarmServedFile>>,
    finished: AtomicBool,
    cancel: CancellationToken,
}

/// 一条连接对应的会话监测器，可克隆。
#[derive(Clone)]
pub(crate) struct SessionMonitor {
    inner: Arc<Inner>,
}

/// 活动守卫：存在期间视为“传输进行中”，采样定时器运行。
pub(crate) struct ActivityGuard {
    inner: Arc<Inner>,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.inner.active.send_modify(|n| *n = n.saturating_sub(1));
    }
}

impl SessionMonitor {
    /// 创建并启动监测任务。
    pub(crate) fn spawn(conn: Connection, cfg: MonitorConfig) -> Self {
        let inner = Arc::new(Inner {
            cfg,
            conn,
            started: Instant::now(),
            started_unix_ms: now_unix_ms(),
            bytes: AtomicU64::new(0),
            verify_failures: AtomicU64::new(0),
            active: watch::channel(0).0,
            activations: AtomicU64::new(0),
            state: Mutex::new(State::default()),
            served: Mutex::new(BTreeMap::new()),
            finished: AtomicBool::new(false),
            cancel: CancellationToken::new(),
        });
        // 先订阅事件流再读取现有路径，避免两者之间漏事件。
        let events = inner.conn.path_events();
        inner.refresh_and_notify();
        tokio::spawn(run(inner.clone(), events));
        Self { inner }
    }

    /// 记录应用层字节（发送方：写入流的字节；接收方：通过校验的字节）。
    pub(crate) fn add_bytes(&self, n: u64) {
        self.inner.bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn add_verify_failure(&self) {
        self.inner.verify_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// 标记一段传输开始；返回的守卫析构时标记结束。
    pub(crate) fn activity(&self) -> ActivityGuard {
        self.inner.activations.fetch_add(1, Ordering::Relaxed);
        self.inner.active.send_modify(|n| *n += 1);
        ActivityGuard {
            inner: self.inner.clone(),
        }
    }

    /// 共享提供方：记入一块成功发出的数据（`bytes` 字节）。
    ///
    /// 单个会话最多逐条记录 [`MAX_SWARM_FILES`] 个内容，超出的新内容不再逐条记账
    /// （字节仍计入会话总量）；这是上报契约的条数上限，超限整份报告会被 FluxCloud 拒收。
    pub(crate) fn add_served(&self, root_hex: &str, bytes: u64) {
        let mut served = self
            .inner
            .served
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !served.contains_key(root_hex) && served.len() >= MAX_SWARM_FILES {
            return;
        }
        let entry = served
            .entry(root_hex.to_owned())
            .or_insert_with(|| SwarmServedFile {
                root_hash: root_hex.to_owned(),
                bytes: 0,
                chunks: 0,
            });
        entry.bytes = entry.bytes.saturating_add(bytes);
        entry.chunks = entry.chunks.saturating_add(1);
    }

    /// 当前摘要快照（刷新路径统计）。
    pub(crate) fn summary(&self, error: Option<String>) -> SessionSummary {
        let mut st = self.inner.lock();
        self.inner.refresh(&mut st);
        self.inner.summary_of(&st, error)
    }

    /// 结束会话：产出终态摘要，发送终态报告与 `Closed` 事件。只有第一次调用生效。
    pub(crate) fn finish(&self, error: Option<String>) -> SessionSummary {
        let inner = &self.inner;
        let (summary, report) = {
            let mut st = inner.lock();
            inner.refresh(&mut st);
            let summary = inner.summary_of(&st, error.clone());
            let samples = std::mem::take(&mut st.pending);
            (summary.clone(), inner.report_of(summary, samples, true))
        };
        if !inner.finished.swap(true, Ordering::SeqCst) {
            inner.cancel.cancel();
            inner.cfg.observers.report_session(&report);
            inner.cfg.observers.emit(SessionEvent::Closed {
                session_id: inner.cfg.session_id.clone(),
                error,
                summary: Box::new(summary.clone()),
            });
        }
        summary
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// 读取连接当前所有路径并更新统计与选中状态。
    fn refresh(&self, st: &mut State) -> Option<(PathKind, String)> {
        let mut selected: Option<(String, PathKind, String)> = None;
        {
            let list = self.conn.paths();
            for path in list.iter() {
                let id = path.id().to_string();
                let kind = classify_addr(path.remote_addr());
                let remote = remote_text(path.remote_addr());
                let stats = path.stats();
                if path.is_selected() {
                    selected = Some((id.clone(), kind, remote.clone()));
                }
                st.paths.insert(
                    id.clone(),
                    PathSummary {
                        id,
                        kind,
                        remote,
                        rtt_ms: stats.rtt.as_secs_f64() * 1000.0,
                        cwnd: stats.cwnd,
                        mtu: stats.current_mtu,
                        sent: stats.udp_tx.bytes,
                        recv: stats.udp_rx.bytes,
                        lost: stats.lost_packets,
                        congestion_events: stats.congestion_events,
                        bw_estimate: stats.bandwidth_estimate.unwrap_or(0),
                    },
                );
            }
        }
        selected.and_then(|(id, kind, remote)| self.apply_selection(st, id, kind, remote))
    }

    /// 应用一次路径选中；路径发生变化时返回新的 (类别, 对端)。
    fn apply_selection(
        &self,
        st: &mut State,
        id: String,
        kind: PathKind,
        remote: String,
    ) -> Option<(PathKind, String)> {
        if st.selected_id.as_deref() == Some(id.as_str()) && st.selected_remote == remote {
            return None;
        }
        st.selected_id = Some(id);
        st.selected_kind = kind;
        st.selected_remote.clone_from(&remote);
        if kind.is_direct() && st.first_direct_ms.is_none() {
            st.first_direct_ms = Some(self.elapsed_ms());
        }
        Some((kind, remote))
    }

    fn notify_path(&self, change: Option<(PathKind, String)>) {
        let Some((kind, remote)) = change else {
            return;
        };
        self.cfg.kind_tx.send_replace(kind);
        self.cfg.observers.emit(SessionEvent::PathChanged {
            session_id: self.cfg.session_id.clone(),
            kind,
            remote,
        });
    }

    fn refresh_and_notify(&self) {
        let change = {
            let mut st = self.lock();
            self.refresh(&mut st)
        };
        self.notify_path(change);
    }

    fn push_event(&self, st: &mut State, kind: &str, path: String) {
        if st.events.len() >= MAX_EVENTS {
            st.events.remove(0);
        }
        st.events.push(PathEventEntry {
            at: rfc3339_ms(now_unix_ms()),
            kind: kind.to_owned(),
            path,
        });
    }

    fn on_path_event(&self, event: PathEvent) {
        let change = {
            let mut st = self.lock();
            match event {
                PathEvent::Opened { remote_addr, .. } => {
                    self.push_event(&mut st, "opened", remote_text(&remote_addr));
                    self.refresh(&mut st)
                }
                PathEvent::Selected {
                    id, remote_addr, ..
                } => {
                    self.push_event(&mut st, "selected", remote_text(&remote_addr));
                    let change = self.apply_selection(
                        &mut st,
                        id.to_string(),
                        classify_addr(&remote_addr),
                        remote_text(&remote_addr),
                    );
                    self.refresh(&mut st).or(change)
                }
                PathEvent::Closed {
                    id,
                    remote_addr,
                    last_stats,
                    ..
                } => {
                    self.push_event(&mut st, "closed", remote_text(&remote_addr));
                    let id = id.to_string();
                    st.paths.insert(
                        id.clone(),
                        PathSummary {
                            id,
                            kind: classify_addr(&remote_addr),
                            remote: remote_text(&remote_addr),
                            rtt_ms: last_stats.rtt.as_secs_f64() * 1000.0,
                            cwnd: last_stats.cwnd,
                            mtu: last_stats.current_mtu,
                            sent: last_stats.udp_tx.bytes,
                            recv: last_stats.udp_rx.bytes,
                            lost: last_stats.lost_packets,
                            congestion_events: last_stats.congestion_events,
                            bw_estimate: last_stats.bandwidth_estimate.unwrap_or(0),
                        },
                    );
                    None
                }
                PathEvent::Lagged { .. } => {
                    self.push_event(&mut st, "lagged", String::new());
                    self.refresh(&mut st)
                }
                _ => None,
            }
        };
        self.notify_path(change);
    }

    fn summary_of(&self, st: &State, error: Option<String>) -> SessionSummary {
        let mut bytes = SessionBytes::default();
        if let Some(account) = &self.cfg.sources {
            let now = account.counters.snapshot();
            bytes.cdn = now.cdn.saturating_sub(account.base.cdn);
            bytes.bridge = now.bridge.saturating_sub(account.base.bridge);
            bytes.peer = now.peer.saturating_sub(account.base.peer);
        }
        for path in st.paths.values() {
            let carried = match self.cfg.role {
                SessionRole::Sender | SessionRole::Swarm => path.sent,
                SessionRole::Receiver => path.recv,
            };
            match path.kind {
                PathKind::DirectLan | PathKind::DirectWan => bytes.direct += carried,
                PathKind::Relay => bytes.relay += carried,
                PathKind::Unknown => {}
            }
        }
        SessionSummary {
            started_at: rfc3339_ms(self.started_unix_ms),
            connect_ms: self.cfg.connect_ms,
            first_direct_ms: st.first_direct_ms,
            path_kind: st.selected_kind,
            bytes,
            paths: st.paths.values().cloned().collect(),
            events: st.events.clone(),
            verify_failures: self.verify_failures.load(Ordering::Relaxed),
            error,
        }
    }

    fn report_of(
        &self,
        summary: SessionSummary,
        samples: Vec<SessionSample>,
        is_final: bool,
    ) -> SessionReport {
        SessionReport {
            session_id: self.cfg.session_id.clone(),
            share_id: self.cfg.share_id.clone(),
            role: self.cfg.role,
            peer_endpoint_id: self.cfg.peer_endpoint_id.clone(),
            summary,
            samples,
            swarm_files: self
                .served
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
                .cloned()
                .collect(),
            peer_bytes: self
                .cfg
                .sources
                .as_ref()
                .map(|a| a.counters.per_peer_delta(&a.base_peers))
                .unwrap_or_default(),
            is_final,
        }
    }

    /// 一次定时采样：追加时序点并上报进行中的摘要。
    fn sample(&self) {
        let (report, progress, change) = {
            let mut st = self.lock();
            let change = self.refresh(&mut st);
            let now = Instant::now();
            let total = self.bytes.load(Ordering::Relaxed);
            let dt = st
                .last_at
                .map_or(self.cfg.report_interval, |t| {
                    now.saturating_duration_since(t)
                })
                .as_secs_f64()
                .max(0.001);
            let rate = total.saturating_sub(st.last_bytes) as f64 / dt;
            st.last_bytes = total;
            st.last_at = Some(now);
            let rtt_ms = st
                .selected_id
                .as_ref()
                .and_then(|id| st.paths.get(id))
                .map_or(0.0, |p| p.rtt_ms);
            if st.pending.len() >= MAX_PENDING_SAMPLES {
                st.pending.remove(0);
            }
            let kind = st.selected_kind;
            st.pending.push(SessionSample {
                at: rfc3339_ms(now_unix_ms()),
                bytes: total,
                rate_bps: rate,
                rtt_ms,
                path_kind: kind,
            });
            let summary = self.summary_of(&st, None);
            let samples = std::mem::take(&mut st.pending);
            (
                self.report_of(summary, samples, false),
                SessionEvent::Progress {
                    session_id: self.cfg.session_id.clone(),
                    bytes: total,
                    rate_bps: rate,
                    path_kind: kind,
                },
                change,
            )
        };
        self.notify_path(change);
        self.cfg.observers.report_session(&report);
        self.cfg.observers.emit(progress);
    }

    /// 采样周期从“活动开始”起算：重置基线，避免第一个点的速率把空闲期算进去。
    fn reset_baseline(&self) {
        let mut st = self.lock();
        st.last_bytes = self.bytes.load(Ordering::Relaxed);
        st.last_at = Some(Instant::now());
    }
}

async fn run(inner: Arc<Inner>, mut events: iroh::endpoint::PathEventStream) {
    let mut active = inner.active.subscribe();
    let mut tick: Option<tokio::time::Interval> = None;
    // 上一个采样点（或定时器启动）时已累计的传输段数，用来判断“这个周期里有没有过传输”。
    let mut seen_activations = 0u64;
    loop {
        // 守卫计数在块与块之间（接收方逐块取数、发送方逐流应答）会反复归零，不能一归零就停表，
        // 否则 `report_interval` 永远凑不满，进行中的报告与采样点在快速传输里会丢光。
        // 改为：定时器常驻，每个 tick 只要计数仍 > 0 或本周期内开始过新传输就上报；
        // 整个周期都没有传输才算一段传输结束，停表。
        if tick.is_none() && *active.borrow_and_update() > 0 {
            inner.reset_baseline();
            seen_activations = inner.activations.load(Ordering::Relaxed);
            let period = inner.cfg.report_interval.max(Duration::from_millis(100));
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick = Some(interval);
        }
        tokio::select! {
            event = events.next() => match event {
                Some(event) => inner.on_path_event(event),
                None => break,
            },
            () = async {
                match tick.as_mut() {
                    Some(t) => { t.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                let activations = inner.activations.load(Ordering::Relaxed);
                if *active.borrow() > 0 || activations != seen_activations {
                    seen_activations = activations;
                    inner.sample();
                } else {
                    // 一段传输结束：补一次统计刷新（终态里要有最后的路径计数）并停表。
                    tick = None;
                    inner.refresh_and_notify();
                }
            }
            changed = active.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            () = inner.cancel.cancelled() => break,
        }
    }
}

/// 把连接关闭原因转成会话终态错误文本；正常的应用层关闭（对端主动结束）不算错误。
pub(crate) fn close_error(reason: &ConnectionError) -> Option<String> {
    match reason {
        ConnectionError::ApplicationClosed(_) | ConnectionError::LocallyClosed => None,
        other => Some(format!("{other:#}")),
    }
}
