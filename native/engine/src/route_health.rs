//! `ProxyMode::Auto` 多路径调度的跨重启先验——host × 路径的时间折扣速率估计。
//!
//! Auto 模式把直连、手动代理、系统代理视为同一 NodePool 内的并列路径，
//! 由真实分段流量实测各路径速率并据此分配连接。本模块只负责**跨任务 /
//! 跨重启的记忆**：每个 host 的每条路径维护一个时间折扣的单连接稳态速率
//! 估计，任务开始时用来为池内各路径**播种初始估计**；一旦本任务产生实测
//! 流量，实测值永远覆盖先验——先验只决定「从哪里开始试」，不决定结论。
//!
//! # 估计器：几何均值 + 指数时间折扣
//!
//! 每条路径存 `(log_mean, weight, ts)`：`log_mean` 为 ln(B/s) 的加权均值
//! （吞吐量近似对数正态、跨数量级，几何均值对单次异常快/慢样本更稳健），
//! `weight` 为有效样本数。读取/记录时先按经过时间衰减：
//! `w_now = weight × 0.5^(Δt / 12h)`；记录新观察
//! `log_mean ← (w_now·log_mean + ln x) / (w_now + 1)`，`weight ← min(w_now + 1, 8)`。
//!
//! 为什么是折扣而不是 TTL 阶梯：网络路由环境是非平稳的（线路拥塞、代理
//! 节点轮换、跨境链路时好时坏），非平稳多臂老虎机文献的标准做法是对历史
//! 观察做指数折扣，让旧证据平滑失去话语权（如 Discounted Thompson
//! Sampling，arXiv:2305.10718）。一个半衰期参数取代了原先冷却指数退避、
//! 代理胜绩确认天数、AdoptProxy、72h 重验等手调阶梯：近期被反复证实的
//! 路径权重高、先验可信；长期未观察的路径权重自然衰减到不足
//! [`MIN_PRIOR_WEIGHT`] 即不再提供先验，调度器回到无先验的均匀探索。
//! 权重封顶 [`MAX_WEIGHT`] 保证再「老牌」的结论也能被数次新观察扭转。
//!
//! # 网络指纹 epoch
//!
//! 路由观察只在同一网络环境下可信，加载/记录时指纹不符即整表丢弃重学
//! （RFC 8305 §4「历史数据 MUST NOT 跨接口使用、换网 SHOULD flush」；
//! Chromium 以 `last_local_address_when_quic_worked` 做同构判定）。网络指纹
//! 只由出口 LAN IP 决定；系统代理端点单独成 epoch——开关 / 更换系统代理
//! 只作废 system 路径的观察，直连与手动代理的结论原样保留（否则每次在代理
//! 客户端里切一下「系统代理」开关就会丢光全部先验，Auto 永远在冷启动）。
//! 两者都只存哈希，不落原始 IP/代理地址。
//!
//! # 路径失败（bad path）
//!
//! 传输层失败不是速率样本（0 B/s 无法进几何均值），单独记一个时间折扣的
//! 失败权重（半衰 [`FAIL_HALF_LIFE_SECS`]，封顶 [`FAIL_MAX_WEIGHT`]）：权重
//! ≥ [`BLOCKED_WEIGHT`] 即视为「近期走不通」，起飞不再选它（同 Chromium 把
//! 失败代理标记为 bad 一段时间再重试的语义）。该路径任何一次成功实测立即
//! 清零失败权重。
//!
//! # NoSwitch（完整性防线）
//!
//! validator 不一致（代理命中不同 CDN edge）是正确性问题而非性能问题，
//! 不进估计器：单独记录 24h，期间 forward failover 禁止把该 host 推上代理。
//!
//! 学习数据是可再生的性能缓存——版本不匹配 / 指纹不符 / 过期 / 解析失败
//! 一律丢弃重学，绝不影响下载正确性。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auto_proxy::{CandidateSource, RoutePath};
use crate::db::Db;
use crate::logger::log_info;

/// config 表 key。
const ROUTE_CONFIG_KEY: &str = "auto_route_health";

/// 持久化格式版本。语义规则变化时递增——旧版本数据加载时整体丢弃重学。
/// v2：TTL 阶梯（冷却/胜绩/AdoptProxy）→ 每路径时间折扣速率估计。
/// v3：网络指纹不再含系统代理端点（另存 `sys`）；新增路径失败权重。
const ROUTE_FORMAT_VERSION: u32 = 3;

/// NoSwitch 记录的有效期：与 `cdn_node_health`/`domain_conn_caps` 一致的
/// 24h（aria2 `--server-stat-timeout` 默认同为 86400s）。
const NO_SWITCH_TTL: Duration = Duration::from_secs(24 * 3600);

/// 容量上限（prune-on-save）：超限按最新观察时间淘汰最旧 host。
const MAX_HOSTS: usize = 512;

/// 网络指纹缓存时长——指纹计算含注册表/路由表查询，不必每次记录都做。
const FINGERPRINT_CACHE: Duration = Duration::from_secs(60);

/// 折扣半衰期：12h。网络路由环境天级漂移——隔夜的观察保留约一半话语权，
/// 三天前的观察基本只剩噪声级权重。
const HALF_LIFE_SECS: f64 = 12.0 * 3600.0;

/// 有效样本权重封顶：任何结论最多相当于 8 次观察，保证数次新观察即可扭转。
const MAX_WEIGHT: f64 = 8.0;

/// 提供先验所需的最小衰减后权重：不足半个样本的证据不播种调度器。
const MIN_PRIOR_WEIGHT: f64 = 0.5;

/// prune 阈值：衰减后权重低于此值的路径统计直接丢弃（已无任何话语权）。
const PRUNE_WEIGHT: f64 = 0.05;

/// 路径统计的绝对保留上限：7 天未更新即丢弃（与权重衰减双保险）。
const MAX_STAT_AGE_SECS: u64 = 7 * 24 * 3600;

/// 路径失败权重的半衰期：1h。一次失败让该路径在约 1h 内不被选作起飞路径，
/// 连续失败（封顶 4 次）最长约 3h；之后回到正常排序，由真实流量重新验证。
const FAIL_HALF_LIFE_SECS: f64 = 3600.0;

/// 失败权重封顶。
const FAIL_MAX_WEIGHT: f64 = 4.0;

/// 衰减后失败权重不低于此值即视为「近期走不通」。
const BLOCKED_WEIGHT: f64 = 0.5;

/// 单条路径的时间折扣速率统计。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct PathStat {
    /// ln(单连接稳态 B/s) 的加权均值。
    log_mean: f64,
    /// 上次更新时刻的有效样本权重（读取时按经过时间衰减）。
    weight: f64,
    /// 上次更新的 Unix 秒。
    ts: u64,
}

/// 单条路径的时间折扣失败权重（传输层失败次数的折扣和）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct FailStat {
    /// 上次更新时刻的失败权重（读取时按 [`FAIL_HALF_LIFE_SECS`] 衰减）。
    weight: f64,
    /// 上次失败的 Unix 秒。
    ts: u64,
}

/// 单 host 的路由观察。字段全部 `#[serde(default)]`：局部缺失按「无观察」
/// 处理，绝不因格式演进丢整表。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct HostRoute {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    direct: Option<PathStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manual: Option<PathStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system: Option<PathStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    direct_fail: Option<FailStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manual_fail: Option<FailStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_fail: Option<FailStat>,
    /// 上次 validator 不一致（代理命中不同 CDN edge）的 Unix 秒，0 = 无。
    #[serde(default)]
    nosw_ts: u64,
}

impl HostRoute {
    /// 网卡链路不持久化先验（拓扑易变、索引跨重启不稳定）。
    fn slot(&self, route: RoutePath) -> Option<PathStat> {
        match route {
            RoutePath::Direct => self.direct,
            RoutePath::Proxy(CandidateSource::ManualFields) => self.manual,
            RoutePath::Proxy(CandidateSource::System) => self.system,
            RoutePath::Link(_) => None,
        }
    }

    fn slot_mut(&mut self, route: RoutePath) -> Option<&mut Option<PathStat>> {
        match route {
            RoutePath::Direct => Some(&mut self.direct),
            RoutePath::Proxy(CandidateSource::ManualFields) => Some(&mut self.manual),
            RoutePath::Proxy(CandidateSource::System) => Some(&mut self.system),
            RoutePath::Link(_) => None,
        }
    }

    fn fail(&self, route: RoutePath) -> Option<FailStat> {
        match route {
            RoutePath::Direct => self.direct_fail,
            RoutePath::Proxy(CandidateSource::ManualFields) => self.manual_fail,
            RoutePath::Proxy(CandidateSource::System) => self.system_fail,
            RoutePath::Link(_) => None,
        }
    }

    fn fail_mut(&mut self, route: RoutePath) -> Option<&mut Option<FailStat>> {
        match route {
            RoutePath::Direct => Some(&mut self.direct_fail),
            RoutePath::Proxy(CandidateSource::ManualFields) => Some(&mut self.manual_fail),
            RoutePath::Proxy(CandidateSource::System) => Some(&mut self.system_fail),
            RoutePath::Link(_) => None,
        }
    }

    /// 反向 failover 语义：丢弃两条代理路径的速率估计，直连估计、失败
    /// 记录与 NoSwitch 保留。
    fn clear_proxy_stats(&mut self) {
        self.manual = None;
        self.system = None;
    }

    /// 系统代理端点变化：system 路径的全部观察都属于旧端点。
    fn clear_system(&mut self) {
        self.system = None;
        self.system_fail = None;
    }

    fn is_empty(&self) -> bool {
        self.direct.is_none()
            && self.manual.is_none()
            && self.system.is_none()
            && self.direct_fail.is_none()
            && self.manual_fail.is_none()
            && self.system_fail.is_none()
            && self.nosw_ts == 0
    }

    /// 最新观察时间（容量裁剪排序用）。
    fn newest_ts(&self) -> u64 {
        let rates = [self.direct, self.manual, self.system]
            .into_iter()
            .flatten()
            .map(|s| s.ts);
        let fails = [self.direct_fail, self.manual_fail, self.system_fail]
            .into_iter()
            .flatten()
            .map(|s| s.ts);
        rates.chain(fails).fold(self.nosw_ts, u64::max)
    }
}

/// 落盘格式：`{"v":3,"net":"<hash16>","sys":"<hash16>","hosts":{...}}`。
#[derive(Serialize, Deserialize)]
struct RouteFile {
    v: u32,
    net: String,
    /// 系统代理端点指纹；空 = 记录时未知 / 未开启系统代理。
    #[serde(default)]
    sys: String,
    hosts: HashMap<String, HostRoute>,
}

/// 某 host 某路径的先验：折扣后的单连接稳态速率（几何均值，B/s）与
/// 衰减后的有效样本权重。调度器据此播种池内路径的初始估计，权重决定
/// 先验相对实测的话语权。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PathPrior {
    pub bps: f64,
    pub weight: f64,
}

static ROUTES: OnceLock<StdMutex<HashMap<String, HostRoute>>> = OnceLock::new();

/// 当前生效的网络指纹（表 epoch）。空 = 尚未初始化 / 网络未就绪。
static NET_EPOCH: OnceLock<StdMutex<String>> = OnceLock::new();

/// 当前生效的系统代理端点指纹（system 路径的 epoch）。空 = 未知 / 未开启。
static SYS_EPOCH: OnceLock<StdMutex<String>> = OnceLock::new();

/// 网络快照缓存（60s）。
static FINGERPRINT: OnceLock<StdMutex<Option<(NetSnapshot, Instant)>>> = OnceLock::new();

/// 离线启动时暂存的磁盘先验：load 拿不到在线指纹无从校验，先存着，
/// 待 [`ensure_net_epoch`] 首次拿到在线指纹时比对采纳（不符即弃）。
static PENDING: OnceLock<StdMutex<Option<RouteFile>>> = OnceLock::new();

/// 写盘序列号：晚创建的快照作废早创建的（防「作废写」被乱序盖回）。
static PERSIST_SEQ: AtomicU64 = AtomicU64::new(0);

fn routes() -> &'static StdMutex<HashMap<String, HostRoute>> {
    ROUTES.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn net_epoch() -> &'static StdMutex<String> {
    NET_EPOCH.get_or_init(|| StdMutex::new(String::new()))
}

fn sys_epoch() -> &'static StdMutex<String> {
    SYS_EPOCH.get_or_init(|| StdMutex::new(String::new()))
}

fn pending() -> &'static StdMutex<Option<RouteFile>> {
    PENDING.get_or_init(|| StdMutex::new(None))
}

/// 当前 Unix 秒（病态时钟回退为 0，仅影响 TTL 判定的保守性）。
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// NoSwitch 时间戳是否仍在 TTL 内。未来时间戳（时钟回拨产物）按无效
/// 观察处理——否则恒新鲜永不过期。
fn fresh(recorded_secs: u64, now_secs: u64) -> bool {
    recorded_secs > 0
        && recorded_secs <= now_secs
        && now_secs - recorded_secs < NO_SWITCH_TTL.as_secs()
}

/// 衰减到 `now` 的有效权重。数据非法（非有限值）或时间戳在未来（时钟
/// 回拨）→ None，按不存在处理——否则未来时间戳永不衰减，会把旧结论钉死。
fn decayed_weight(stat: &PathStat, now: u64) -> Option<f64> {
    if stat.ts == 0 || stat.ts > now || !stat.log_mean.is_finite() || !stat.weight.is_finite() {
        return None;
    }
    let elapsed = (now - stat.ts) as f64;
    let w = stat.weight.max(0.0) * 0.5f64.powf(elapsed / HALF_LIFE_SECS);
    Some(w)
}

/// 纯函数：路径统计 → 先验（权重不足 [`MIN_PRIOR_WEIGHT`] → None）。
fn prior_of(stat: &PathStat, now: u64) -> Option<PathPrior> {
    let weight = decayed_weight(stat, now)?;
    if weight < MIN_PRIOR_WEIGHT {
        return None;
    }
    let bps = stat.log_mean.exp();
    bps.is_finite().then_some(PathPrior { bps, weight })
}

/// 纯函数：把一次观察 `bps` 折入路径统计（先衰减旧权重，再在对数空间
/// 加权平均）。调用方保证 `bps` 有限且为正。
fn blend(slot: &mut Option<PathStat>, bps: f64, now: u64) {
    let (log_mean, w) = slot
        .as_ref()
        .and_then(|s| decayed_weight(s, now).map(|w| (s.log_mean, w)))
        .unwrap_or((0.0, 0.0));
    *slot = Some(PathStat {
        log_mean: (w * log_mean + bps.ln()) / (w + 1.0),
        weight: (w + 1.0).min(MAX_WEIGHT),
        ts: now,
    });
}

/// 路径统计是否仍值得保留（权重未衰减殆尽且未超绝对保留期）。
fn stat_alive(stat: &PathStat, now: u64) -> bool {
    decayed_weight(stat, now).is_some_and(|w| w >= PRUNE_WEIGHT)
        && now - stat.ts <= MAX_STAT_AGE_SECS
}

/// 衰减到 `now` 的失败权重；非法数据 / 未来时间戳 → None。
fn fail_weight(stat: &FailStat, now: u64) -> Option<f64> {
    if stat.ts == 0 || stat.ts > now || !stat.weight.is_finite() {
        return None;
    }
    let elapsed = (now - stat.ts) as f64;
    Some(stat.weight.max(0.0) * 0.5f64.powf(elapsed / FAIL_HALF_LIFE_SECS))
}

/// 纯函数：把一次传输失败折入失败权重。
fn blend_fail(slot: &mut Option<FailStat>, now: u64) {
    let w = slot
        .as_ref()
        .and_then(|s| fail_weight(s, now))
        .unwrap_or(0.0);
    *slot = Some(FailStat {
        weight: (w + 1.0).min(FAIL_MAX_WEIGHT),
        ts: now,
    });
}

/// 纯函数：失败记录是否仍让该路径处于「近期走不通」。
fn is_blocked(stat: &FailStat, now: u64) -> bool {
    fail_weight(stat, now).is_some_and(|w| w >= BLOCKED_WEIGHT)
}

/// 网络环境快照：`net` 为网络指纹（只含出口 LAN IP），`sys` 为系统代理
/// 端点指纹。空串 = 未知。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NetSnapshot {
    net: String,
    sys: String,
}

/// 纯函数：带域分隔的 sha256 前 16 hex。
fn hash16(domain: &str, input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    hasher.update(input.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// 纯函数：网络指纹 = sha256("net" + '\0' + 本机出口 LAN IP) 前 16 hex。
fn fingerprint_of(lan_ip: &str) -> String {
    hash16("net", lan_ip)
}

/// 纯函数：系统代理端点指纹（`host:port`）。
fn system_endpoint_of(endpoint: &str) -> String {
    hash16("sys", endpoint)
}

/// 本机默认路由的出口 LAN IP。`UdpSocket::connect` **不发任何数据包**，
/// 纯路由表查询；无默认路由（离线/睡眠唤醒窗口）→ None。
fn lan_ip() -> Option<String> {
    std::net::UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|s| {
            s.connect(("8.8.8.8", 53))?;
            s.local_addr()
        })
        .ok()
        .map(|a| a.ip().to_string())
}

/// 本机是否有默认路由——区分「代理死了」与「整机断网」：反向 failover
/// 只应在前者作废先验（断网时所有代理路由任务都会报连接类错误，
/// 与代理无关）。
pub(crate) fn network_reachable() -> bool {
    lan_ip().is_some()
}

/// 计算当前网络快照（无缓存）。**拿不到出口 IP = 网络未就绪，`net` 为空
/// 表示 unknown**——不是「另一张网」：调用方见空指纹一律维持现状，
/// 避免睡眠唤醒/Wi-Fi 漫游/VPN 重拨的无路由窗口误清全表。系统代理端点
/// 取 Auto 实际使用的系统候选（含 PAC 解析出的端点）。
fn compute_snapshot() -> NetSnapshot {
    let Some(lan) = lan_ip() else {
        return NetSnapshot::default();
    };
    let sys = crate::proxy_config::detect_auto_system_proxy()
        .map(|p| system_endpoint_of(&format!("{}:{}", p.host, p.port)))
        .unwrap_or_default();
    NetSnapshot {
        net: fingerprint_of(&lan),
        sys,
    }
}

/// 带 60s 缓存的当前网络快照。
fn net_snapshot() -> NetSnapshot {
    let cache = FINGERPRINT.get_or_init(|| StdMutex::new(None));
    if let Ok(mut slot) = cache.lock() {
        if let Some((snap, at)) = slot.as_ref()
            && at.elapsed() < FINGERPRINT_CACHE
        {
            return snap.clone();
        }
        let snap = compute_snapshot();
        *slot = Some((snap.clone(), Instant::now()));
        return snap;
    }
    compute_snapshot()
}

/// 校准表 epoch：网络指纹变化（换 Wi-Fi / 开关 VPN）时清空内存表——旧
/// 网络学到的路由结论对新网络是噪声（RFC 8305 flush 语义）；系统代理端点
/// 换成另一个已知端点时只作废 system 路径。指纹为空（网络未就绪）时维持
/// 现状。每次 lookup/record 前调用。
fn ensure_net_epoch() {
    apply_snapshot(&net_snapshot());
}

fn apply_snapshot(snap: &NetSnapshot) {
    if snap.net.is_empty() {
        return;
    }
    let first_online = {
        let Ok(mut cur) = net_epoch().lock() else {
            return;
        };
        if *cur == snap.net {
            None
        } else {
            let had = !cur.is_empty();
            cur.clone_from(&snap.net);
            Some(!had)
        }
    };
    match first_online {
        Some(false) => {
            if let Ok(mut map) = routes().lock() {
                if !map.is_empty() {
                    log_info!(
                        "[route-health] 网络指纹变化，丢弃 {} 个 host 的路由先验重学",
                        map.len()
                    );
                }
                map.clear();
            }
            if let Ok(mut p) = pending().lock() {
                *p = None;
            }
        }
        // 离线启动后首次上线：这是磁盘先验唯一的运行时回读路径。
        Some(true) => adopt_pending(snap),
        None => {}
    }
    sync_sys_epoch(&snap.sys);
}

/// 系统代理端点 epoch：只在「已知端点 → 另一个已知端点」时作废 system
/// 路径；关闭系统代理（端点未知）不清任何东西——重新打开同一端点时先验
/// 原样可用。
fn sync_sys_epoch(sys: &str) {
    if sys.is_empty() {
        return;
    }
    let changed = {
        let Ok(mut cur) = sys_epoch().lock() else {
            return;
        };
        if *cur == sys {
            return;
        }
        let had = !cur.is_empty();
        *cur = sys.to_string();
        had
    };
    if changed && let Ok(mut map) = routes().lock() {
        let mut cleared = 0usize;
        for entry in map.values_mut() {
            if entry.system.is_some() || entry.system_fail.is_some() {
                entry.clear_system();
                cleared += 1;
            }
        }
        map.retain(|_, e| !e.is_empty());
        if cleared > 0 {
            log_info!(
                "[route-health] 系统代理端点变化，作废 {} 个 host 的系统代理路径观察",
                cleared
            );
        }
    }
}

/// 纯函数：把磁盘表并入时按系统代理端点 epoch 过滤——记录时与当前都是
/// 已知端点且不同 → 丢弃 system 路径观察。
fn strip_foreign_system(hosts: &mut HashMap<String, HostRoute>, recorded: &str, current: &str) {
    if recorded.is_empty() || current.is_empty() || recorded == current {
        return;
    }
    for entry in hosts.values_mut() {
        entry.clear_system();
    }
    hosts.retain(|_, e| !e.is_empty());
}

/// 采纳离线启动时暂存的磁盘先验（指纹相符才装表，否则丢弃）。
fn adopt_pending(snap: &NetSnapshot) {
    let stashed = pending().lock().ok().and_then(|mut p| p.take());
    let Some(file) = stashed else { return };
    if file.net != snap.net {
        log_info!("[route-health] 网络就绪但指纹与上次运行不符，暂存先验丢弃重学");
        return;
    }
    let now = now_unix_secs();
    let mut incoming = file.hosts;
    strip_foreign_system(&mut incoming, &file.sys, &snap.sys);
    if snap.sys.is_empty()
        && let Ok(mut cur) = sys_epoch().lock()
        && cur.is_empty()
    {
        cur.clone_from(&file.sys);
    }
    prune(&mut incoming, now);
    let loaded = incoming.len();
    if let Ok(mut map) = routes().lock() {
        for (host, entry) in incoming {
            map.entry(host).or_insert(entry);
        }
    }
    log_info!(
        "[route-health] 网络就绪，采纳暂存的 {} 个 host 路由先验",
        loaded
    );
}

/// 就地清除失效观察 + 容量裁剪（淘汰最旧）。
fn prune(map: &mut HashMap<String, HostRoute>, now: u64) {
    map.retain(|_, e| {
        for slot in [&mut e.direct, &mut e.manual, &mut e.system] {
            if slot.as_ref().is_some_and(|s| !stat_alive(s, now)) {
                *slot = None;
            }
        }
        for slot in [&mut e.direct_fail, &mut e.manual_fail, &mut e.system_fail] {
            if slot
                .as_ref()
                .is_some_and(|s| fail_weight(s, now).is_none_or(|w| w < PRUNE_WEIGHT))
            {
                *slot = None;
            }
        }
        if !fresh(e.nosw_ts, now) {
            e.nosw_ts = 0;
        }
        !e.is_empty()
    });
    if map.len() > MAX_HOSTS {
        let mut ts_sorted: Vec<u64> = map.values().map(HostRoute::newest_ts).collect();
        ts_sorted.sort_unstable();
        let cutoff = ts_sorted[ts_sorted.len() - MAX_HOSTS];
        let mut kept = 0usize;
        map.retain(|_, e| {
            if e.newest_ts() >= cutoff && kept < MAX_HOSTS {
                kept += 1;
                true
            } else {
                false
            }
        });
    }
}

/// Engine 启动时从 config 表读回持久化先验（与 `load_cdn_health` 同一
/// 生命周期点调用）。版本/指纹不匹配、解析失败 → 空表重学。
pub(crate) async fn load(db: &Db) {
    load_with_snapshot(db, net_snapshot()).await;
}

/// [`load`] 的快照注入变体（测试确定性；生产路径恒经 [`load`]）。
async fn load_with_snapshot(db: &Db, snap: NetSnapshot) {
    if let Ok(mut cur) = net_epoch().lock() {
        cur.clone_from(&snap.net);
    }
    if !snap.sys.is_empty()
        && let Ok(mut cur) = sys_epoch().lock()
    {
        cur.clone_from(&snap.sys);
    }
    let raw = match db.get_config(ROUTE_CONFIG_KEY).await {
        Ok(Some(v)) => v,
        Ok(None) => return,
        Err(e) => {
            log_info!(
                "[route-health] 读取持久化路由先验失败（忽略，重新学习）: {}",
                e
            );
            return;
        }
    };
    let parsed: RouteFile = match serde_json::from_str(&raw) {
        Ok(f) => f,
        Err(e) => {
            log_info!("[route-health] 路由先验解析失败（丢弃重学）: {}", e);
            return;
        }
    };
    if parsed.v != ROUTE_FORMAT_VERSION {
        log_info!(
            "[route-health] 路由先验格式版本不匹配（{} != {}），整体丢弃重学",
            parsed.v,
            ROUTE_FORMAT_VERSION
        );
        return;
    }
    if snap.net.is_empty() {
        // 网络未就绪：无从校验指纹。暂存待 ensure_net_epoch 首次上线时
        // 比对采纳（开机自启早于网络就绪的常态路径）。
        if let Ok(mut p) = pending().lock() {
            *p = Some(parsed);
        }
        log_info!("[route-health] 网络未就绪，路由先验暂存待采纳");
        return;
    }
    if parsed.net != snap.net {
        log_info!("[route-health] 网络指纹与上次运行不符，路由先验整体丢弃重学");
        return;
    }
    let now = now_unix_secs();
    let mut incoming = parsed.hosts;
    strip_foreign_system(&mut incoming, &parsed.sys, &snap.sys);
    if snap.sys.is_empty()
        && let Ok(mut cur) = sys_epoch().lock()
    {
        // 当前未开系统代理：沿用记录时的端点 epoch，重新打开同一端点时先验有效。
        cur.clone_from(&parsed.sys);
    }
    prune(&mut incoming, now);
    let loaded = incoming.len();
    if let Ok(mut map) = routes().lock() {
        for (host, entry) in incoming {
            map.entry(host).or_insert(entry);
        }
    }
    log_info!("[route-health] 已加载 {} 个 host 的路由先验", loaded);
}

/// 查询某 host 某路径的先验（无观察 / 权重衰减不足 / 换网 → None）。
pub(crate) fn path_prior(host: &str, route: RoutePath) -> Option<PathPrior> {
    ensure_net_epoch();
    let now = now_unix_secs();
    let map = routes().lock().ok()?;
    map.get(host)
        .and_then(|e| e.slot(route))
        .and_then(|s| prior_of(&s, now))
}

/// 记录一次路径实测：`per_conn_bps` 为该路径单连接稳态速率（B/s）。
/// 非有限值或 ≤0 直接忽略。成功实测同时清零该路径的失败权重。立即落盘。
pub(crate) fn record_path_rate(host: &str, route: RoutePath, per_conn_bps: f64, db: &Db) {
    if !per_conn_bps.is_finite() || per_conn_bps <= 0.0 || route.is_link() {
        return;
    }
    ensure_net_epoch();
    let now = now_unix_secs();
    if let Ok(mut map) = routes().lock() {
        let e = map.entry(host.to_string()).or_default();
        if let Some(slot) = e.slot_mut(route) {
            blend(slot, per_conn_bps, now);
        }
        if let Some(fail) = e.fail_mut(route) {
            *fail = None;
        }
    }
    persist(db);
}

/// 记录一次路径传输层失败（建连被拒 / 超时 / TLS 被劫持 / 中途断流等，
/// 见 [`crate::net_failure::NetFailure`]）。速率估计不动——失败与速率是
/// 两个维度：被墙的直连不是「慢」，而是「不通」。立即落盘。
pub(crate) fn record_path_failure(host: &str, route: RoutePath, db: &Db) {
    if route.is_link() {
        return;
    }
    ensure_net_epoch();
    let now = now_unix_secs();
    if let Ok(mut map) = routes().lock() {
        let e = map.entry(host.to_string()).or_default();
        if let Some(fail) = e.fail_mut(route) {
            blend_fail(fail, now);
        }
    }
    persist(db);
}

/// 该路径近期是否走不通（失败权重未衰减到 [`BLOCKED_WEIGHT`] 以下，且此后
/// 无成功实测）。起飞选路据此避开。
pub(crate) fn path_blocked(host: &str, route: RoutePath) -> bool {
    ensure_net_epoch();
    let now = now_unix_secs();
    routes()
        .lock()
        .ok()
        .and_then(|map| map.get(host).and_then(|e| e.fail(route)))
        .is_some_and(|stat| is_blocked(&stat, now))
}

/// 该 host 是否有未过期的 validator 不一致记录。forward failover 门禁
/// 消费：已知代理会命中不同 CDN edge 的 host，绝不许被 failover 推上
/// 代理续传（跨重启延续内存态 NoSwitch 的完整性防线）。
pub(crate) fn no_switch_active(host: &str) -> bool {
    ensure_net_epoch();
    let now = now_unix_secs();
    routes()
        .lock()
        .ok()
        .and_then(|map| map.get(host).map(|e| fresh(e.nosw_ts, now)))
        .unwrap_or(false)
}

/// 记录 validator 不一致（完整性防线）。立即落盘。
pub(crate) fn record_no_switch(host: &str, db: &Db) {
    ensure_net_epoch();
    let now = now_unix_secs();
    if let Ok(mut map) = routes().lock() {
        map.entry(host.to_string()).or_default().nosw_ts = now;
    }
    persist(db);
}

/// 反向 failover：经代理的任务连接类失败 → 作废该 host 两条代理路径的
/// 估计（直连估计 / NoSwitch 保留），重试回直连——杜绝「持久化代理先验 +
/// 代理失效 → 无法自愈」的锁死。立即落盘。
pub(crate) fn clear_proxy_prior(host: &str, db: &Db) {
    ensure_net_epoch();
    if let Ok(mut map) = routes().lock()
        && let Some(e) = map.get_mut(host)
    {
        e.clear_proxy_stats();
    }
    persist(db);
}

/// 代理设置变更时全表作废（内存 + 持久化 + 离线暂存）：所有先验都是对
/// 旧候选代理/旧出口的观察（指纹只覆盖系统代理，手动字段变更不换
/// epoch），与 `clear_domain_conn_caps` 同点位
/// 调用。
pub(crate) fn clear_all(db: &Db) {
    if let Ok(mut p) = pending().lock() {
        *p = None;
    }
    if let Ok(mut map) = routes().lock() {
        if !map.is_empty() {
            log_info!(
                "[route-health] 代理设置变更，作废 {} 个 host 的路由先验",
                map.len()
            );
        }
        map.clear();
    }
    // 绕过 persist 的空 epoch 守卫直接写空表：无论网络是否就绪，磁盘上
    // 针对旧候选代理的先验都必须作废（net 为空的空表下次 load 同样会被
    // 丢弃，语义一致）。
    let net = net_epoch().lock().map(|c| c.clone()).unwrap_or_default();
    let sys = sys_epoch().lock().map(|c| c.clone()).unwrap_or_default();
    spawn_write(
        db,
        &RouteFile {
            v: ROUTE_FORMAT_VERSION,
            net,
            sys,
            hosts: HashMap::new(),
        },
    );
}

/// 序列化并异步写盘。晚创建的快照作废早创建的：写任务落盘前校验自己
/// 仍是最新序列，过期即弃——防「作废写」（clear_*）被更早排队的常规
/// 快照乱序盖回。
fn spawn_write(db: &Db, file: &RouteFile) {
    let Ok(json) = serde_json::to_string(file) else {
        return;
    };
    let seq = PERSIST_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let db = db.clone();
    tokio::spawn(async move {
        if PERSIST_SEQ.load(Ordering::Relaxed) != seq {
            return; // 已有更新的快照排队，本次写作废。
        }
        if let Err(e) = db.set_config(ROUTE_CONFIG_KEY, &json).await {
            log_info!("[route-health] 路由先验持久化失败（忽略）: {}", e);
        }
    });
}

/// 把当前缓存快照异步写回 config 表（fire-and-forget；顺带 prune）。
/// 网络未就绪（epoch 为空）时不写——不许用离线会话的空表覆盖磁盘上
/// 仍然有效的先验。
fn persist(db: &Db) {
    let net = {
        let Ok(cur) = net_epoch().lock() else { return };
        cur.clone()
    };
    if net.is_empty() {
        return;
    }
    let snapshot = {
        let Ok(mut map) = routes().lock() else { return };
        prune(&mut map, now_unix_secs());
        map.clone()
    };
    let sys = sys_epoch().lock().map(|c| c.clone()).unwrap_or_default();
    let file = RouteFile {
        v: ROUTE_FORMAT_VERSION,
        net,
        sys,
        hosts: snapshot,
    };
    spawn_write(db, &file);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        FAIL_HALF_LIFE_SECS, FAIL_MAX_WEIGHT, FailStat, HALF_LIFE_SECS, HostRoute, MAX_HOSTS,
        MAX_STAT_AGE_SECS, MAX_WEIGHT, NetSnapshot, PathStat, ROUTE_CONFIG_KEY,
        ROUTE_FORMAT_VERSION, RouteFile, blend, blend_fail, decayed_weight, fingerprint_of,
        is_blocked, load_with_snapshot, now_unix_secs, prior_of, prune, routes,
        strip_foreign_system, system_endpoint_of,
    };
    use std::collections::HashMap;

    const NOW: u64 = 2_000_000_000;
    const HL: u64 = HALF_LIFE_SECS as u64;

    fn stat(bps: f64, weight: f64, ts: u64) -> PathStat {
        PathStat {
            log_mean: bps.ln(),
            weight,
            ts,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn decay_halves_weight_per_half_life() {
        let s = stat(1e6, 4.0, NOW - HL);
        assert!(
            close(decayed_weight(&s, NOW).unwrap(), 2.0),
            "一个半衰期权重减半"
        );
        let s2 = stat(1e6, 4.0, NOW - 2 * HL);
        assert!(close(decayed_weight(&s2, NOW).unwrap(), 1.0));
        let p = prior_of(&s, NOW).unwrap();
        assert!(close(p.bps, 1e6), "衰减只影响权重，不改变速率估计");
        assert!(close(p.weight, 2.0));
    }

    #[test]
    fn record_blends_geometrically_and_caps_weight() {
        let mut slot = None;
        blend(&mut slot, 1e6, NOW);
        let p = prior_of(&slot.unwrap(), NOW).unwrap();
        assert!(close(p.bps, 1e6) && close(p.weight, 1.0), "首次观察即估计");
        blend(&mut slot, 4e6, NOW);
        let p = prior_of(&slot.unwrap(), NOW).unwrap();
        assert!(close(p.bps, 2e6), "等权两样本取几何均值 sqrt(1e6·4e6)");
        assert!(close(p.weight, 2.0));
        for _ in 0..50 {
            blend(&mut slot, 4e6, NOW);
        }
        assert!(slot.unwrap().weight <= MAX_WEIGHT, "权重封顶");
        // 满权重的旧结论也能被少数新观察明显拉动。
        let mut old = Some(stat(1e6, MAX_WEIGHT, NOW));
        blend(&mut old, 1e8, NOW);
        let p = prior_of(&old.unwrap(), NOW).unwrap();
        assert!(p.bps > 1e6 * 1.5, "封顶权重下单次观察仍有 1/9 话语权");
    }

    #[test]
    fn stale_evidence_yields_to_new_observation() {
        // 陈旧的高权重结论衰减后，新观察占主导。
        let mut slot = Some(stat(1e6, MAX_WEIGHT, NOW - 6 * HL));
        blend(&mut slot, 1e8, NOW);
        let p = prior_of(&slot.unwrap(), NOW).unwrap();
        assert!(p.bps > 5e7, "8×2^-6=0.125 权重的旧证据只能轻微拉低新观察");
    }

    #[test]
    fn prior_below_min_weight_is_none() {
        let weak = stat(1e6, 1.0, NOW - 2 * HL); // 衰减到 0.25
        assert_eq!(prior_of(&weak, NOW), None);
        let ok = stat(1e6, 1.0, NOW - HL / 2); // ≈0.71
        assert!(prior_of(&ok, NOW).is_some());
    }

    #[test]
    fn future_timestamps_are_ignored() {
        // 时钟回拨后 ts > now：按不存在处理，绝不恒不衰减。
        let future = stat(1e6, MAX_WEIGHT, NOW + 3600);
        assert_eq!(prior_of(&future, NOW), None);
        let mut slot = Some(future);
        blend(&mut slot, 3e6, NOW);
        let s = slot.unwrap();
        assert!(close(s.weight, 1.0), "未来条目不参与混合，从新观察重新开始");
        assert!(close(s.log_mean.exp(), 3e6));
        assert_eq!(s.ts, NOW);
    }

    #[test]
    fn clear_proxy_keeps_direct_and_nosw() {
        let mut e = HostRoute {
            direct: Some(stat(1e6, 2.0, NOW)),
            manual: Some(stat(5e6, 2.0, NOW)),
            system: Some(stat(4e6, 2.0, NOW)),
            nosw_ts: NOW,
            ..HostRoute::default()
        };
        e.clear_proxy_stats();
        assert_eq!(e.manual, None);
        assert_eq!(e.system, None);
        assert_eq!(e.direct, Some(stat(1e6, 2.0, NOW)));
        assert_eq!(e.nosw_ts, NOW);
    }

    #[test]
    fn prune_drops_dead_stats_and_caps_hosts() {
        let mut map: HashMap<String, HostRoute> = HashMap::new();
        map.insert(
            "decayed.example".into(),
            HostRoute {
                direct: Some(stat(1e6, MAX_WEIGHT, NOW - 10 * HL)), // 8/1024 < 0.05
                ..HostRoute::default()
            },
        );
        map.insert(
            "ancient.example".into(),
            HostRoute {
                direct: Some(stat(1e6, 1e9, NOW - MAX_STAT_AGE_SECS - 1)),
                ..HostRoute::default()
            },
        );
        map.insert(
            "mixed.example".into(),
            HostRoute {
                direct: Some(stat(1e6, 1.0, NOW)),
                manual: Some(stat(1e6, 1.0, NOW - 20 * HL)),
                ..HostRoute::default()
            },
        );
        for n in 0..(MAX_HOSTS + 8) {
            map.insert(
                format!("h{n}.example"),
                HostRoute {
                    direct: Some(stat(1e6, 1.0, NOW - n as u64)),
                    ..HostRoute::default()
                },
            );
        }
        prune(&mut map, NOW);
        assert!(map.len() <= MAX_HOSTS);
        assert!(!map.contains_key("decayed.example"), "权重衰减殆尽必须清除");
        assert!(!map.contains_key("ancient.example"), "超绝对保留期必须清除");
        let mixed = &map["mixed.example"];
        assert!(
            mixed.direct.is_some() && mixed.manual.is_none(),
            "逐路径清除"
        );
        assert!(map.contains_key("h0.example"), "最新条目必须保留");
    }

    #[test]
    fn fingerprint_is_deterministic_and_input_sensitive() {
        let a = fingerprint_of("192.168.1.5");
        assert_eq!(a, fingerprint_of("192.168.1.5"));
        assert_ne!(a, fingerprint_of("10.0.0.3"), "LAN IP 变化必须换 epoch");
        assert_eq!(a.len(), 16, "16 hex = 8 字节截断");
        // 网络指纹与系统代理端点指纹分域：同一输入不得同构。
        assert_ne!(
            fingerprint_of("127.0.0.1:7890"),
            system_endpoint_of("127.0.0.1:7890")
        );
    }

    #[test]
    fn one_failure_blocks_for_about_one_half_life_and_repeats_extend_it() {
        let hl = FAIL_HALF_LIFE_SECS as u64;
        let mut slot = None;
        blend_fail(&mut slot, NOW);
        let once = slot.unwrap();
        assert!(is_blocked(&once, NOW));
        assert!(is_blocked(&once, NOW + hl), "一个半衰期内仍视为走不通");
        assert!(!is_blocked(&once, NOW + hl + 60), "过期后回到正常排序");

        for _ in 0..10 {
            blend_fail(&mut slot, NOW);
        }
        let repeated = slot.unwrap();
        assert!(repeated.weight <= FAIL_MAX_WEIGHT, "失败权重封顶");
        assert!(
            is_blocked(&repeated, NOW + 3 * hl),
            "连续失败延长到 3 个半衰期"
        );
        assert!(!is_blocked(&repeated, NOW + 3 * hl + 60));
        // 时钟回拨：未来时间戳不判阻断。
        let future = FailStat {
            weight: FAIL_MAX_WEIGHT,
            ts: NOW + 10,
        };
        assert!(!is_blocked(&future, NOW));
    }

    #[test]
    fn system_endpoint_change_strips_only_system_path() {
        let entry = HostRoute {
            direct: Some(stat(1e6, 2.0, NOW)),
            system: Some(stat(4e6, 2.0, NOW)),
            system_fail: Some(FailStat {
                weight: 1.0,
                ts: NOW,
            }),
            ..HostRoute::default()
        };
        let only_system = HostRoute {
            system: Some(stat(4e6, 2.0, NOW)),
            ..HostRoute::default()
        };
        let fresh_map = || {
            HashMap::from([
                ("mixed.example".to_string(), entry),
                ("sys-only.example".to_string(), only_system),
            ])
        };

        let mut changed = fresh_map();
        strip_foreign_system(&mut changed, "sys-a", "sys-b");
        let mixed = changed["mixed.example"];
        assert_eq!(mixed.direct, entry.direct, "直连结论不受系统代理更换影响");
        assert_eq!(mixed.system, None);
        assert_eq!(mixed.system_fail, None);
        assert!(
            !changed.contains_key("sys-only.example"),
            "清空后的条目移除"
        );

        // 系统代理关闭（端点未知）或端点未变：原样保留。
        for (recorded, current) in [("sys-a", ""), ("", "sys-b"), ("sys-a", "sys-a")] {
            let mut kept = fresh_map();
            strip_foreign_system(&mut kept, recorded, current);
            assert_eq!(kept["mixed.example"].system, entry.system);
            assert!(kept.contains_key("sys-only.example"));
        }
    }

    async fn mem_db() -> crate::db::Db {
        crate::db::Db::connect("sqlite::memory:")
            .await
            .expect("mem db")
    }

    /// 组合契约：经真实 config 表往返（in-memory sqlite）后估计值不变；
    /// net 指纹不符 → 整表丢弃。
    #[tokio::test]
    async fn load_roundtrips_estimates_and_respects_net_epoch() {
        let db = mem_db().await;
        let now = now_unix_secs();
        let seeded = HostRoute {
            direct: Some(stat(1.5e6, 3.0, now)),
            system: Some(stat(6e6, 2.0, now)),
            ..HostRoute::default()
        };
        let mut hosts = HashMap::new();
        hosts.insert("load-epoch.example".to_string(), seeded);
        let file = RouteFile {
            v: ROUTE_FORMAT_VERSION,
            net: "net-a".to_string(),
            sys: String::new(),
            hosts,
        };
        db.set_config(ROUTE_CONFIG_KEY, &serde_json::to_string(&file).unwrap())
            .await
            .expect("seed config");

        let snap = |net: &str| NetSnapshot {
            net: net.to_string(),
            sys: String::new(),
        };
        load_with_snapshot(&db, snap("net-b")).await;
        assert!(
            !routes().lock().unwrap().contains_key("load-epoch.example"),
            "指纹不符必须整表丢弃"
        );

        load_with_snapshot(&db, snap("net-a")).await;
        let loaded = routes().lock().unwrap().get("load-epoch.example").copied();
        let loaded = loaded.expect("指纹相符必须加载条目");
        let same = |a: Option<PathStat>, b: Option<PathStat>| match (a, b) {
            (Some(a), Some(b)) => {
                close(a.log_mean, b.log_mean) && close(a.weight, b.weight) && a.ts == b.ts
            }
            _ => false,
        };
        assert!(same(loaded.direct, seeded.direct), "直连估计往返不变");
        assert!(same(loaded.system, seeded.system), "代理估计往返不变");
        assert_eq!(loaded.manual, None);
    }

    #[tokio::test]
    async fn version_1_data_is_discarded() {
        let db = mem_db().await;
        let now = now_unix_secs();
        // v1 条目里 nosw_ts 字段与 v2 同名，若不校验版本会被误装表。
        let raw = format!(
            r#"{{"v":1,"net":"net-v1","hosts":{{"v1.example":{{"proxy_ts":{now},"proxy_n":3,"nosw_ts":{now}}}}}}}"#
        );
        db.set_config(ROUTE_CONFIG_KEY, &raw)
            .await
            .expect("seed config");
        load_with_snapshot(
            &db,
            NetSnapshot {
                net: "net-v1".to_string(),
                sys: String::new(),
            },
        )
        .await;
        assert!(
            !routes().lock().unwrap().contains_key("v1.example"),
            "旧格式版本必须整体丢弃"
        );
    }
}
