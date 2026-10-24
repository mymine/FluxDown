//! coordinator 侧的多路径调度状态：连接窗口采样 → 路径估计、冷路径探索、
//! 完成时间抢占、`ProxyMode::Auto` 主导链路标签与跨任务先验回写、多网卡
//! 额外链路的后台规划与挂载。
//!
//! 判据全部来自 [`crate::path_scheduler`]；路径选择本身在
//! [`crate::cdn::NodePool::lease_for`]。本模块只在每个完整 ramp 窗口被
//! 驱动一次，不引入新的事件循环分支。

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use super::{LiveSegment, SegState};
use crate::auto_proxy::{AutoProxyCtx, RoutePath, route};
use crate::cdn::NodePool;
use crate::db::Db;
use crate::events::{EngineEvent, EventSink};
use crate::logger::{log_error, log_info};
use crate::model::CdnNodeInfo;
use crate::multi_nic::{MultiNicInput, PreparedLinks};
use crate::path_scheduler::{MIN_SAMPLE_WINDOWS, completion_secs, should_preempt, window_rate};

/// 剩余字节低于此值不再探索冷路径——探索连接来不及进入稳态。
const EXPLORE_MIN_REMAINING: i64 = 4 * 1024 * 1024;

/// 抢占后抑制 ramp 扩容评估/劣化收缩的窗口数：交接期的吞吐波动不是
/// 连接额度的责任，不得触发回滚或写域名连接上限。
const MIGRATION_QUIET_TICKS: u32 = 2;

/// 单个在途连接的窗口跟踪。
struct Track {
    seg_index: i32,
    node_id: usize,
    /// 上个窗口边界时该段的已下字节。
    last_bytes: i64,
    /// 上个完整窗口边界；初值为租约开始时刻。
    last_at: Instant,
    /// 已跨越的完整窗口数（1 = 首个完整窗口，慢启动预热）。
    windows: u32,
    /// 本次完整窗口的字节增量；初始窗口尚不足完整时长时为 `None`。
    last_delta: Option<i64>,
    /// 最近一个完整窗口的稳态速率（预热窗/限速窗为 `None`）。
    last_rate: Option<f64>,
    preempted: bool,
}

/// 本窗口的守卫。
pub(super) struct TickGuards {
    /// 可采样：本窗口未被限速器节流。限速器饱和的窗口速率反映限速器而非链路；
    /// 限速值高于链路速率时连接并非 app-limited，窗口照常采样。
    pub sampling: bool,
    /// 可改道（Range 已验证、非串行、非连接敏感、非重连敌对、本窗口未被
    /// 限速器节流）。
    pub may_reroute: bool,
    /// 任务剩余字节。
    pub remaining_total: i64,
    /// 不可抢占的段（开放式首段生命线）。
    pub protected_seg: Option<i32>,
}

/// 本窗口的调度产出。
pub(super) struct TickReport {
    /// 本窗发起的抢占数。
    pub preempted: usize,
    /// 需要为冷路径额外放出 1 个探索 worker。
    pub explore_worker_wanted: bool,
}

/// `ProxyMode::Auto` 的主导链路标签发布器。
struct RouteLabeler {
    ctx: Arc<AutoProxyCtx>,
    published: &'static str,
    pinned: bool,
}

impl RouteLabeler {
    /// 按主导路径（窗口累计字节最多）推导标签。主导路径仍是起飞路径时
    /// 保留 manager 写入的更具体标签（failover/cached），仅在备选路径被
    /// 实测过时把 `direct` 升级为 `direct:sampled`（或 validator 不一致时
    /// `direct:pinned`）。
    fn desired(&self, route_bytes: &[(RoutePath, u64)], explored: bool) -> &'static str {
        let start = self.ctx.start_route;
        let dominant = route_bytes
            .iter()
            .filter(|(_, bytes)| *bytes > 0)
            .max_by(|a, b| a.1.cmp(&b.1).then((a.0 == start).cmp(&(b.0 == start))))
            .map_or(start, |(route, _)| *route);
        if dominant != start {
            return dominant.sampled_label();
        }
        if start == RoutePath::Direct && self.ctx.start_label == route::DIRECT {
            if self.pinned {
                return route::DIRECT_PINNED;
            }
            if explored {
                return route::DIRECT_SAMPLED;
            }
        }
        self.ctx.start_label
    }
}

/// 多网卡额外链路的规划状态。
enum LinkState {
    /// 功能关闭或已处理完毕。
    Idle,
    /// 后台规划中（解析目标 + 枚举网卡 + 构建绑定 client）。
    Pending(tokio::task::JoinHandle<PreparedLinks>),
}

/// coordinator 侧多路径状态（每次 `run_coordinated_download` 一份）。
pub(super) struct Multipath {
    tracks: HashMap<u64, Track>,
    labeler: Option<RouteLabeler>,
    quiet_ticks: u32,
    links: LinkState,
    /// 目标 host（链路规划完成后填入；补齐无 sink 池暂存事件的归属）。
    link_host: String,
}

impl Drop for Multipath {
    fn drop(&mut self) {
        // 下载先于规划结束：链路已无用武之地，不让后台任务继续构建 client。
        if let LinkState::Pending(handle) = &self.links {
            handle.abort();
        }
    }
}

impl Multipath {
    /// 构造并（Auto 任务）为每条备选路径构建 client 挂入节点池。client
    /// 构建失败的路径跳过（与该路径不存在等价）。多网卡聚合开启时在后台
    /// 发起链路规划，结果由 [`Self::poll_links`] 在窗口边界挂入——首连接
    /// 不等待 DNS/网卡枚举。
    pub(super) fn new(
        ctx: Option<Arc<AutoProxyCtx>>,
        multi_nic: Option<Arc<MultiNicInput>>,
        url: &str,
        nodes: &NodePool,
        task_id: &str,
    ) -> Self {
        let labeler = ctx.map(|ctx| {
            let mut alternates = Vec::with_capacity(ctx.alternates.len());
            for alt in &ctx.alternates {
                match crate::downloader::build_client_with_tls_policy(
                    &alt.config,
                    &ctx.user_agent,
                    false,
                ) {
                    Ok(client) => alternates.push((alt.route, client, alt.prior_bps)),
                    Err(e) => log_error!(
                        "[multipath] task {} {:?} 路径 client 构建失败，跳过: {e}",
                        task_id,
                        alt.route
                    ),
                }
            }
            log_info!(
                "[multipath] task {} host {} 起飞 {:?}（先验 {:?}），备选路径 {:?}",
                task_id,
                ctx.host,
                ctx.start_route,
                ctx.start_prior_bps,
                alternates
                    .iter()
                    .map(|(route, _, prior)| (*route, *prior))
                    .collect::<Vec<_>>()
            );
            nodes.add_paths(ctx.start_route, ctx.start_prior_bps, alternates);
            RouteLabeler {
                published: ctx.start_label,
                ctx,
                pinned: false,
            }
        });
        let links = match multi_nic {
            Some(input) => {
                let url = url.to_string();
                LinkState::Pending(tokio::spawn(async move {
                    crate::multi_nic::prepare_links(&url, &input).await
                }))
            }
            None => LinkState::Idle,
        };
        Self {
            tracks: HashMap::new(),
            labeler,
            quiet_ticks: 0,
            links,
            link_host: String::new(),
        }
    }

    /// 窗口边界驱动：链路规划完成则挂入节点池并发 `links` / `links_off`
    /// 事件；随后转发池内暂存的节点事件（无 sink 的单节点池挂了链路时，
    /// 租约快照/踢除事件在此补齐任务归属后发出）。
    pub(super) async fn poll_links(
        &mut self,
        nodes: &NodePool,
        sink: &dyn EventSink,
        task_id: &str,
    ) {
        if matches!(&self.links, LinkState::Pending(handle) if handle.is_finished())
            && let LinkState::Pending(handle) = std::mem::replace(&mut self.links, LinkState::Idle)
        {
            match handle.await {
                Ok(prepared) => self.attach_links(prepared, nodes, sink, task_id),
                Err(e) => log_error!("[multi-nic] task {} 链路规划任务异常: {e}", task_id),
            }
        }
        for mut event in nodes.take_deferred_events() {
            if let EngineEvent::TaskCdnEvent {
                task_id: owner,
                host,
                ..
            } = &mut event
            {
                if owner.is_empty() {
                    *owner = task_id.to_string();
                }
                if host.is_empty() {
                    host.clone_from(&self.link_host);
                }
            }
            sink.emit(event);
        }
    }

    fn attach_links(
        &mut self,
        prepared: PreparedLinks,
        nodes: &NodePool,
        sink: &dyn EventSink,
        task_id: &str,
    ) {
        self.link_host = prepared.host;
        let (kind, reason, infos) = match prepared.outcome {
            Ok(links) => {
                let infos: Vec<CdnNodeInfo> = links
                    .iter()
                    .map(|(link, _)| CdnNodeInfo {
                        ip: link.label(),
                        origin: link.local_ip().map(|ip| ip.to_string()).unwrap_or_default(),
                        bytes: 0,
                        ewma_bps: 0,
                        active: 0,
                    })
                    .collect();
                log_info!(
                    "[multi-nic] task {} host {} 挂入额外链路: {:?}",
                    task_id,
                    self.link_host,
                    infos.iter().map(|n| n.ip.as_str()).collect::<Vec<_>>()
                );
                nodes.add_links(links);
                ("links", String::new(), infos)
            }
            Err(reason) => ("links_off", reason.to_string(), Vec::new()),
        };
        sink.emit(EngineEvent::TaskCdnEvent {
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            host: self.link_host.clone(),
            nodes: infos,
            ip: String::new(),
            reason,
            candidates: 0,
            alive: 0,
            cap: 0,
            auto_cap: false,
        });
    }

    /// 完整 ramp 窗口驱动：采样 → 路径估计 → 探索开关 → 完成时间抢占。
    ///
    /// `segments` 须已与共享进度同步。每个活动段的 `rate_bps` 被重写为
    /// 持有连接本窗稳态速率（未知为 `None`），供拆分挑选与均衡切分。
    pub(super) fn on_tick(
        &mut self,
        nodes: &NodePool,
        segments: &mut BTreeMap<i32, LiveSegment>,
        now: Instant,
        guards: &TickGuards,
        task_id: &str,
    ) -> TickReport {
        let live = nodes.live_conns();
        let mut samples: Vec<(usize, f64)> = Vec::new();
        for seg in segments.values_mut() {
            seg.rate_bps = None;
        }
        let mut next_tracks: HashMap<u64, Track> = HashMap::with_capacity(live.len());
        for conn in &live {
            let Some(seg) = segments.get_mut(&conn.seg_index) else {
                continue;
            };
            let current = seg.downloaded_bytes;
            let mut track = self.tracks.remove(&conn.lease_id).unwrap_or(Track {
                seg_index: conn.seg_index,
                node_id: conn.node_id,
                last_bytes: conn.start_downloaded,
                last_at: conn.started_at,
                last_delta: None,
                windows: 0,
                last_rate: None,
                preempted: false,
            });
            // 租约出生时已有准确的字节/时刻基线，不再白等一个 ramp 窗口。
            // 很晚才出生的连接仍须积满完整预热窗；不能把不足一窗的零增量
            // 当停滞，也不能把预热字节混入下一个稳态样本。
            let elapsed = now.saturating_duration_since(track.last_at);
            if elapsed.as_millis() >= super::RAMP_MIN_EVAL_WINDOW_MS {
                let delta = (current - track.last_bytes).max(0);
                track.last_bytes = current;
                track.last_at = now;
                track.last_delta = Some(delta);
                track.windows = track.windows.saturating_add(1);
                track.last_rate = (guards.sampling && track.windows >= MIN_SAMPLE_WINDOWS)
                    .then(|| window_rate(delta, elapsed));
            } else {
                track.last_delta = None;
                track.last_rate = None;
            }
            if let Some(rate) = track.last_rate {
                samples.push((track.node_id, rate));
                if seg.state == SegState::Active {
                    seg.rate_bps = Some(rate);
                }
            }
            track.seg_index = conn.seg_index;
            next_tracks.insert(conn.lease_id, track);
        }
        self.tracks = next_tracks;
        nodes.observe_window(&samples);

        let explore = guards.may_reroute && guards.remaining_total >= EXPLORE_MIN_REMAINING;
        nodes.set_explore(explore);

        let preempted = if guards.may_reroute && nodes.is_multipath() {
            self.preempt_stragglers(nodes, segments, guards, task_id, live.len())
        } else {
            0
        };
        if preempted > 0 {
            self.quiet_ticks = MIGRATION_QUIET_TICKS;
        }
        TickReport {
            preempted,
            explore_worker_wanted: explore && nodes.has_idle_cold_path(),
        }
    }

    /// 完成时间抢占：按预计完成时间从晚到早，至多 `max(1, 在途/4)` 条。
    fn preempt_stragglers(
        &mut self,
        nodes: &NodePool,
        segments: &BTreeMap<i32, LiveSegment>,
        guards: &TickGuards,
        task_id: &str,
        live_count: usize,
    ) -> usize {
        let Some(best) = nodes.best_measured_rate() else {
            return 0;
        };
        let mut candidates: Vec<(f64, u64, i32, i64, f64, usize)> = Vec::new();
        for (&lease_id, track) in &self.tracks {
            if track.preempted || guards.protected_seg == Some(track.seg_index) {
                continue;
            }
            let Some(seg) = segments.get(&track.seg_index) else {
                continue;
            };
            if seg.state != SegState::Active {
                continue;
            }
            // 稳态样本，或首个完整窗口即零字节（停滞无需等慢启动）。
            let rate = match track.last_rate {
                Some(rate) => rate,
                None if track.windows >= 1 && guards.sampling && track.last_delta == Some(0) => 0.0,
                None => continue,
            };
            let remaining = seg.remaining();
            if should_preempt(remaining, rate, best) {
                candidates.push((
                    completion_secs(remaining, rate),
                    lease_id,
                    track.seg_index,
                    remaining,
                    rate,
                    track.node_id,
                ));
            }
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        let budget = (live_count / 4).max(1);
        let mut attempts = 0;
        let mut done = 0;
        let mut stalled_nodes: Vec<(usize, f64)> = Vec::new();
        // 网卡链路本窗剩余的有效连接 = 在途租约 − 已被抢占但尚未归还的租约
        // （首次触及时计算）：链路承载独立容量，慢于最优单连接不是交出最后
        // 一条连接的理由——非停滞连接只在链路仍有其它连接时抢占；停滞连接
        // 照常抢占并计入扣减。
        let mut link_left: HashMap<usize, u32> = HashMap::new();
        for (_, lease_id, seg_index, remaining, rate, node_id) in candidates {
            if attempts == budget {
                break;
            }
            if let Some(outstanding) = nodes.link_outstanding(node_id) {
                let left = link_left.entry(node_id).or_insert_with(|| {
                    let draining = self
                        .tracks
                        .values()
                        .filter(|t| t.preempted && t.node_id == node_id)
                        .count();
                    outstanding.saturating_sub(u32::try_from(draining).unwrap_or(u32::MAX))
                });
                if rate > 0.0 && *left <= 1 {
                    continue;
                }
                *left = left.saturating_sub(1);
            }
            attempts += 1;
            if nodes.preempt(lease_id) {
                if let Some(track) = self.tracks.get_mut(&lease_id) {
                    track.preempted = true;
                    // 整窗零字节的停滞连接：以 0 速率计入该路径实证，冷路径不再
                    // 被反复探索，已知路径跌出竞争集。
                    if rate == 0.0 {
                        stalled_nodes.push((track.node_id, 0.0));
                    }
                }
                done += 1;
                log_info!(
                    "[multipath] task {} seg {} 抢占：剩余 {} B，本连接 {:.0} B/s，最优路径 {:.0} B/s",
                    task_id,
                    seg_index,
                    remaining,
                    rate,
                    best
                );
            }
        }
        if !stalled_nodes.is_empty() {
            nodes.observe_stalled(&stalled_nodes);
        }
        done
    }

    /// 本窗是否处于交接期（抑制 ramp 扩容评估与劣化收缩）。每窗调用一次。
    pub(super) fn take_quiet_tick(&mut self) -> bool {
        if self.quiet_ticks > 0 {
            self.quiet_ticks -= 1;
            true
        } else {
            false
        }
    }

    /// 备选路径（代理 / 网卡链路）是否承载过流量（为真时连接规模观察混有
    /// 其它出口的连接，不得学习为源站域名连接上限/起步提示）。
    pub(super) fn alternates_used(&self, nodes: &NodePool) -> bool {
        nodes.alternates_explored()
    }

    /// 处理代理路径 validator 踢除（记 NoSwitch）并按主导路径（已结束租约
    /// 回报 + 在途租约进度的累计传输字节）发布链路标签。
    pub(super) async fn publish_route(
        &mut self,
        nodes: &NodePool,
        segments: &BTreeMap<i32, LiveSegment>,
        db: &Db,
        sink: &dyn EventSink,
        task_id: &str,
    ) {
        let Some(labeler) = self.labeler.as_mut() else {
            return;
        };
        for kicked in nodes.take_validator_kicks() {
            log_info!(
                "[multipath] task {} host {} {:?} 路径 validator 不一致，已踢除并记录 NoSwitch",
                task_id,
                labeler.ctx.host,
                kicked
            );
            crate::route_health::record_no_switch(&labeler.ctx.host, db);
            labeler.pinned = true;
        }
        record_path_failures(&labeler.ctx.host, nodes, db, Some(task_id));
        let in_flight = in_flight(nodes, segments);
        let label = labeler.desired(&nodes.route_bytes(&in_flight), nodes.alternates_explored());
        if label == labeler.published {
            return;
        }
        labeler.published = label;
        log_info!("[multipath] task {} 主导链路: {}", task_id, label);
        if let Err(e) = db.set_task_auto_route(task_id, label).await {
            log_error!("[multipath] task {} 路由落库失败: {e:#}", task_id);
        }
        sink.emit(EngineEvent::TaskRouteChanged {
            task_id: task_id.to_string(),
            route: label.to_string(),
        });
    }

    /// 任务结束：把已实测的各路径单连接估计写入跨任务折扣先验，并抽干末窗
    /// 新增的路径失败（先失败后先验：先验不含本任务失败过的路径）。
    pub(super) fn record_priors(&self, nodes: &NodePool, db: &Db) {
        let Some(labeler) = self.labeler.as_ref() else {
            return;
        };
        record_path_failures(&labeler.ctx.host, nodes, db, None);
        for (route, bps) in nodes.path_estimates() {
            crate::route_health::record_path_rate(&labeler.ctx.host, route, bps, db);
        }
    }
}

/// 把节点池暂存的 Auto 路径传输失败写入跨任务路径健康度。池内已按路径去重
/// （同一任务同一路径只出现一次），此处不再重复判断。
fn record_path_failures(host: &str, nodes: &NodePool, db: &Db, task_id: Option<&str>) {
    for route in nodes.take_path_failures() {
        log_info!(
            "[multipath] task {} host {} {:?} 路径传输失败，记入路径健康度",
            task_id.unwrap_or("-"),
            host,
            route
        );
        crate::route_health::record_path_failure(host, route, db);
    }
}

/// 在途租约的「槽位, 本次已传字节」：当前段进度 − 租约起点。喂 `route_bytes` /
/// `source_bytes`；段已不在布局内的租约（拆分 / 完成瞬间）不计。
pub(super) fn in_flight(
    nodes: &NodePool,
    segments: &BTreeMap<i32, LiveSegment>,
) -> Vec<(usize, u64)> {
    nodes
        .live_conns()
        .iter()
        .filter_map(|conn| {
            let seg = segments.get(&conn.seg_index)?;
            Some((
                conn.node_id,
                (seg.downloaded_bytes - conn.start_downloaded).max(0) as u64,
            ))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::auto_proxy::CandidateSource;
    use std::time::Duration;

    const MANUAL: RoutePath = RoutePath::Proxy(CandidateSource::ManualFields);

    fn labeler(start: RoutePath, start_label: &'static str) -> RouteLabeler {
        RouteLabeler {
            ctx: Arc::new(AutoProxyCtx {
                host: "h".to_string(),
                user_agent: String::new(),
                start_route: start,
                start_prior_bps: None,
                start_label,
                alternates: Vec::new(),
            }),
            published: start_label,
            pinned: false,
        }
    }

    #[test]
    fn label_follows_dominant_path() {
        let l = labeler(RoutePath::Direct, route::DIRECT);
        assert_eq!(l.desired(&[], false), route::DIRECT);
        assert_eq!(
            l.desired(&[(RoutePath::Direct, 10), (MANUAL, 0)], true),
            route::DIRECT_SAMPLED
        );
        assert_eq!(
            l.desired(&[(RoutePath::Direct, 10), (MANUAL, 30)], true),
            route::PROXY_SAMPLED_MANUAL
        );
    }

    #[test]
    fn start_specific_labels_survive_while_start_path_dominates() {
        let failover = labeler(RoutePath::Direct, route::DIRECT_FAILOVER);
        assert_eq!(
            failover.desired(&[(RoutePath::Direct, 10), (MANUAL, 1)], true),
            route::DIRECT_FAILOVER
        );
        let cached = labeler(MANUAL, route::PROXY_CACHED_MANUAL);
        assert_eq!(
            cached.desired(&[(MANUAL, 10), (RoutePath::Direct, 1)], true),
            route::PROXY_CACHED_MANUAL
        );
        assert_eq!(
            cached.desired(&[(MANUAL, 1), (RoutePath::Direct, 10)], true),
            route::DIRECT_SAMPLED
        );
    }

    #[test]
    fn validator_kick_pins_direct_label() {
        let mut l = labeler(RoutePath::Direct, route::DIRECT);
        l.pinned = true;
        assert_eq!(
            l.desired(&[(RoutePath::Direct, 10), (MANUAL, 1)], true),
            route::DIRECT_PINNED
        );
    }

    type HeldLease = (
        crate::cdn::node_pool::NodeLease,
        tokio_util::sync::CancellationToken,
    );

    /// 主链路单连接 10MB/s（实证）；链路 30 连接 × 1MB/s 的容量让前两次租借
    /// 落到链路；另挂 6 条不在分段表里的主链路租约，只抬高在途数（抢占预算 = 2）。
    fn nic_pool_with_two_slow_link_leases() -> (Arc<NodePool>, Vec<HeldLease>) {
        use crate::cdn::node_pool::LeaseRequest;
        let pool = NodePool::single(reqwest::Client::new());
        pool.add_links(vec![(
            Arc::new(crate::multi_nic::LinkBinding {
                name: "en-guard".into(),
                index: 2,
                v4: Some("172.20.10.2".parse().unwrap()),
                v6: None,
            }),
            reqwest::Client::new(),
        )]);
        pool.observe_window(&[(0, 10e6)]);
        pool.observe_window(&vec![(1, 1e6); 30]);
        pool.set_explore(true);
        let mut held = Vec::new();
        for seg in 0..8 {
            let cancel = tokio_util::sync::CancellationToken::new();
            let lease = pool.lease_for(LeaseRequest {
                seg_index: if seg < 2 { seg } else { 100 + seg },
                start_downloaded: 0,
                bytes: i64::MAX,
                allow_alternates: seg < 2,
                cancel: cancel.clone(),
            });
            if seg < 2 {
                assert_eq!(lease.route(), RoutePath::Link(2));
            }
            held.push((lease, cancel));
        }
        (pool, held)
    }

    #[test]
    fn slow_link_keeps_its_last_connection_across_preemption_ticks() {
        let (pool, held) = nic_pool_with_two_slow_link_leases();
        let mut segments: BTreeMap<i32, LiveSegment> = (0..2)
            .map(|index| {
                let start = i64::from(index) * 1_000_000_000;
                (
                    index,
                    LiveSegment {
                        index,
                        start_byte: start,
                        end_byte: start + 999_999_999,
                        downloaded_bytes: 0,
                        state: SegState::Active,
                        rate_bps: None,
                    },
                )
            })
            .collect();
        let guards = TickGuards {
            sampling: true,
            may_reroute: true,
            remaining_total: i64::MAX,
            protected_seg: None,
        };
        let mut mp = Multipath::new(None, None, "https://example.com/f", &pool, "t");
        let started = pool
            .live_conns()
            .iter()
            .map(|conn| conn.started_at)
            .max()
            .expect("live leases");
        // 每窗 1s 下 1MB：稳态 1MB/s，远慢于最优 10MB/s 的一半 → 两条都满足抢占判据；
        // 被抢占的租约在测试里一直未归还，模拟 worker 尚未退出的交接窗口。
        for tick in 1..=6 {
            for seg in segments.values_mut() {
                seg.downloaded_bytes += 1_000_000;
            }
            mp.on_tick(
                &pool,
                &mut segments,
                started + Duration::from_secs(tick),
                &guards,
                "t",
            );
        }
        let cancelled = held[..2].iter().filter(|(_, c)| c.is_cancelled()).count();
        assert_eq!(cancelled, 1, "链路两条慢连接只能交出一条，最后一条保留");
    }

    struct ProxySamplingFixture {
        pool: Arc<NodePool>,
        held: Vec<HeldLease>,
        segments: BTreeMap<i32, LiveSegment>,
        started: Instant,
    }

    fn proxy_sampling_fixture(start_downloaded: i64) -> ProxySamplingFixture {
        use crate::cdn::node_pool::LeaseRequest;
        let pool = NodePool::single(reqwest::Client::new());
        pool.add_paths(
            RoutePath::Direct,
            None,
            vec![(MANUAL, reqwest::Client::new(), None)],
        );
        pool.set_explore(true);
        let mut held = Vec::new();
        for seg_index in 0..2 {
            let cancel = tokio_util::sync::CancellationToken::new();
            let lease = pool.lease_for(LeaseRequest {
                seg_index,
                start_downloaded: if seg_index == 1 { start_downloaded } else { 0 },
                bytes: 32 * 1024 * 1024,
                allow_alternates: seg_index == 1,
                cancel: cancel.clone(),
            });
            held.push((lease, cancel));
        }
        assert_eq!(held[1].0.route(), MANUAL);
        pool.observe_window(&[(0, 3.0 * 1024.0 * 1024.0)]);
        let started = pool
            .live_conns()
            .into_iter()
            .find(|conn| conn.seg_index == 1)
            .expect("proxy lease")
            .started_at;
        let segments = [(
            1,
            LiveSegment {
                index: 1,
                start_byte: 0,
                end_byte: 32 * 1024 * 1024 - 1,
                downloaded_bytes: start_downloaded,
                state: SegState::Active,
                rate_bps: None,
            },
        )]
        .into();
        ProxySamplingFixture {
            pool,
            held,
            segments,
            started,
        }
    }

    fn sampling_guards(sampling: bool) -> TickGuards {
        TickGuards {
            sampling,
            may_reroute: true,
            remaining_total: i64::MAX,
            protected_seg: None,
        }
    }

    fn minimum_sampling_window() -> Duration {
        Duration::from_millis(
            u64::try_from(super::super::RAMP_MIN_EVAL_WINDOW_MS).expect("sampling window fits u64"),
        )
    }

    #[test]
    fn lease_origin_preserves_first_warmup_and_actual_sample_duration() {
        let resumed = 16 * 1024;
        let mut f = proxy_sampling_fixture(resumed);
        let mut mp = Multipath::new(None, None, "https://example.com/f", &f.pool, "t");
        let guards = sampling_guards(true);
        f.segments.get_mut(&1).unwrap().downloaded_bytes += 2048;
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + Duration::from_secs(2),
            &guards,
            "t",
        );
        assert!(!f.held[1].1.is_cancelled(), "nonzero warmup is not a stall");
        assert_eq!(
            f.segments[&1].rate_bps, None,
            "warmup must not become a rate sample"
        );

        // A delayed tick must use its actual 3s interval, not the nominal 2s ramp.
        f.segments.get_mut(&1).unwrap().downloaded_bytes += 3 * 2048;
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + Duration::from_secs(5),
            &guards,
            "t",
        );
        assert_eq!(f.segments[&1].rate_bps, Some(2048.0));
        assert!(
            f.held[1].1.is_cancelled(),
            "first steady window must reclaim the slow lease"
        );
        assert!(!f.held[0].1.is_cancelled());
    }

    #[test]
    fn short_initial_ticks_accumulate_a_complete_warmup_before_sampling() {
        let mut f = proxy_sampling_fixture(0);
        let mut mp = Multipath::new(None, None, "https://example.com/f", &f.pool, "t");
        let guards = sampling_guards(true);
        let window = minimum_sampling_window();
        for elapsed in [window / 5, window * 4 / 5] {
            mp.on_tick(&f.pool, &mut f.segments, f.started + elapsed, &guards, "t");
            assert!(
                !f.held[1].1.is_cancelled(),
                "short zero-byte window is not a stall"
            );
            assert_eq!(f.segments[&1].rate_bps, None);
        }
        f.segments.get_mut(&1).unwrap().downloaded_bytes = 1024;
        mp.on_tick(&f.pool, &mut f.segments, f.started + window, &guards, "t");
        assert!(
            !f.held[1].1.is_cancelled(),
            "first complete window is warmup only"
        );
        assert_eq!(f.segments[&1].rate_bps, None);

        // A partial window following warmup cannot trigger early preemption.
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + window * 8 / 5,
            &guards,
            "t",
        );
        assert!(!f.held[1].1.is_cancelled());
        f.segments.get_mut(&1).unwrap().downloaded_bytes += 1024;
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + window * 2,
            &guards,
            "t",
        );
        assert_eq!(f.segments[&1].rate_bps, Some(1024.0 / window.as_secs_f64()));
        assert!(
            f.held[1].1.is_cancelled(),
            "second complete window must reclaim the slow lease"
        );
    }

    #[test]
    fn limited_windows_are_excluded_from_rates_and_stall_preemption() {
        let mut f = proxy_sampling_fixture(0);
        let mut mp = Multipath::new(None, None, "https://example.com/f", &f.pool, "t");
        let limited = sampling_guards(false);
        // Even a zero-byte initial window under an active limiter is not path evidence.
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + Duration::from_secs(2),
            &limited,
            "t",
        );
        assert!(!f.held[1].1.is_cancelled());
        f.segments.get_mut(&1).unwrap().downloaded_bytes += 64 * 1024;
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + Duration::from_secs(4),
            &limited,
            "t",
        );
        assert_eq!(f.segments[&1].rate_bps, None);
        assert!(!f.held[1].1.is_cancelled());

        f.segments.get_mut(&1).unwrap().downloaded_bytes += 2 * 2048;
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + Duration::from_secs(6),
            &sampling_guards(true),
            "t",
        );
        assert_eq!(
            f.segments[&1].rate_bps,
            Some(2048.0),
            "limited bytes must not contaminate the next sample"
        );
        assert!(f.held[1].1.is_cancelled());
    }

    #[test]
    fn truly_stalled_lease_is_cancelled_on_its_first_complete_window() {
        let mut f = proxy_sampling_fixture(0);
        let mut mp = Multipath::new(None, None, "https://example.com/f", &f.pool, "t");
        let guards = sampling_guards(true);
        let window = minimum_sampling_window();
        mp.on_tick(
            &f.pool,
            &mut f.segments,
            f.started + window * 2 / 5,
            &guards,
            "t",
        );
        assert!(!f.held[1].1.is_cancelled());
        mp.on_tick(&f.pool, &mut f.segments, f.started + window, &guards, "t");
        assert!(
            f.held[1].1.is_cancelled(),
            "a full zero-byte window is real stall evidence"
        );
        assert!(!f.held[0].1.is_cancelled());
    }

    /// CI 复现：极慢代理的探索段被帮手拆到拆分最小片以下（既分不走、
    /// 曾经也因剩余 < 64KiB 不可抢占），只能以 2KiB/s 拖尾 30s+。稳态
    /// 样本一到位就必须抢占交接，无论剩余多小。
    #[test]
    fn slow_path_holding_sub_split_tail_fragment_is_preempted() {
        use crate::cdn::node_pool::LeaseRequest;
        let pool = NodePool::single(reqwest::Client::new());
        pool.add_paths(
            RoutePath::Direct,
            None,
            vec![(MANUAL, reqwest::Client::new(), None)],
        );
        pool.set_explore(true);
        let lease = |seg_index: i32, allow_alternates: bool| {
            let cancel = tokio_util::sync::CancellationToken::new();
            let lease = pool.lease_for(LeaseRequest {
                seg_index,
                start_downloaded: 0,
                bytes: i64::MAX,
                allow_alternates,
                cancel: cancel.clone(),
            });
            (lease, cancel)
        };
        let direct = lease(0, false);
        let proxy = lease(1, true);
        assert_eq!(direct.0.route(), RoutePath::Direct);
        assert_eq!(proxy.0.route(), MANUAL);

        // 段 1 = CI 日志中的段 8 残片：67761 B，随后按 2KiB/s 推进。
        let seg = |index: i32, start: i64, len: i64| {
            (
                index,
                LiveSegment {
                    index,
                    start_byte: start,
                    end_byte: start + len - 1,
                    downloaded_bytes: 0,
                    state: SegState::Active,
                    rate_bps: None,
                },
            )
        };
        let mut segments: BTreeMap<i32, LiveSegment> =
            [seg(0, 0, 1 << 30), seg(1, 1 << 30, 67_761)].into();
        let guards = TickGuards {
            sampling: true,
            may_reroute: true,
            remaining_total: i64::MAX,
            protected_seg: None,
        };
        let window = Duration::from_secs(2);
        let mut mp = Multipath::new(None, None, "https://example.com/f", &pool, "t");
        let started = pool
            .live_conns()
            .iter()
            .map(|conn| conn.started_at)
            .max()
            .expect("live leases");
        let mut preempted_at = None;
        for tick in 0..4 {
            if let Some(s) = segments.get_mut(&0) {
                s.downloaded_bytes += 2 * 3 * 1024 * 1024;
            }
            if let Some(s) = segments.get_mut(&1) {
                s.downloaded_bytes += 2 * 2048;
            }
            mp.on_tick(
                &pool,
                &mut segments,
                started + window * (tick + 1),
                &guards,
                "t",
            );
            if preempted_at.is_none() && proxy.1.is_cancelled() {
                preempted_at = Some((tick, segments[&1].remaining()));
            }
        }
        let (tick, remaining) = preempted_at.expect("slow tail fragment must be preempted");
        // 租约首窗已完整跨过预热；第 2 窗的首个稳态样本即抢占。
        assert_eq!(tick, 1);
        assert!(remaining < 64 * 1024, "remaining = {remaining}");
        assert!(!direct.1.is_cancelled(), "最优路径连接不得被抢占");
    }
}
