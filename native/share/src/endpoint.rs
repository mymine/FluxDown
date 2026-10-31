//! iroh 端点管理：懒绑定、策略装配、网络快照与状态监听。
//!
//! **零 n0 基础设施**：端点基于 `presets::Minimal`（只设置加密后端），中继、地址发布与地址查找
//! 全部来自 [`NetworkPolicy`]（FluxCloud 下发）。本模块不引用、也不会回落到 iroh 官方中继、
//! `iroh.link` 的 DNS / pkarr 服务，或 mainline DHT；DNS 解析器显式关闭了公共回落。
//!
//! **空闲静默**：[`ShareEndpoint::new`] 不绑定任何 socket；宿主在第一次需要分享 / 接收时调用
//! [`ShareEndpoint::ensure_bound`]，无活动时调用 [`ShareEndpoint::shutdown`]，之后没有任何任务与定时器。
//! 绑定期间网络状态靠 iroh 的 `Watcher` 订阅推送，不轮询。

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use iroh::address_lookup::{AddrFilter, DnsAddressLookup, PkarrPublisher};
use iroh::dns::{DnsResolver, NameserverConfig};
use iroh::endpoint::{PortmapperConfig, presets};
use iroh::protocol::Router;
use iroh::unstable_net_report::NetReport;
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey,
    TransportAddr, Watcher as _,
};
use iroh_metrics::{MetricValue, MetricsGroupSet as _};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::observe::Observers;
use crate::sender::{ShareRegistry, ShareSender};
use crate::swarm::{ContentRegistry, SWARM_ALPN};
use crate::ticket::KeyRing;
use crate::timefmt::now_rfc3339;
use crate::types::{
    ConnectionMode, HomeRelayInfo, LocalEndpointConfig, NameserverProtocol, NatType, NetReportInfo,
    NetSnapshot, NetworkPolicy, RelayNode,
};
use crate::wire::ALPN;

/// 网络状态变化的合并窗口：一次网络切换会连续触发多个 watcher，等它们落定后再出一份快照。
const SETTLE: Duration = Duration::from_millis(500);

/// 构造 [`ShareEndpoint`] 的参数。
pub struct ShareEndpointParams {
    /// 端点私钥，由宿主持久化（0600 权限的独立文件）；其公钥即 endpoint ID。
    pub secret_key: SecretKey,
    /// FluxCloud 下发的网络策略。
    pub policy: NetworkPolicy,
    /// 本机端点设置。
    pub local: LocalEndpointConfig,
    /// 客户端版本（写进快照）。
    pub client_version: String,
    /// 票据验签密钥环（宿主用 `GET /share/keys` 填充，可随时刷新）。
    pub keys: KeyRing,
    /// 分享登记表（宿主维护）。
    pub registry: ShareRegistry,
    /// 事件与上报出口。
    pub observers: Observers,
    /// 共享内容表（宿主维护）：本机能向别人提供的完整文件与下载中的部分块。
    pub content: ContentRegistry,
}

struct Bound {
    endpoint: Endpoint,
    router: Router,
    cancel: CancellationToken,
}

struct Shared {
    secret_key: SecretKey,
    id: EndpointId,
    policy: NetworkPolicy,
    local: LocalEndpointConfig,
    client_version: String,
    observers: Observers,
    sender: ShareSender,
    /// 串行化绑定 / 关闭。
    lifecycle: tokio::sync::Mutex<()>,
    /// 当前已绑定的端点；同步读取用。
    current: RwLock<Option<Arc<Bound>>>,
    /// 去重后的最新快照（UI 订阅）。
    snapshots: watch::Sender<Option<Arc<NetSnapshot>>>,
}

/// FluxMesh端点：同时承担分享方（接入请求）与接收方（出站连接）。可克隆，克隆体共享同一个端点。
#[derive(Clone)]
pub struct ShareEndpoint {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for ShareEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareEndpoint")
            .field("id", &self.shared.id.fmt_short().to_string())
            .field("bound", &self.is_bound())
            .finish()
    }
}

impl ShareEndpoint {
    /// 创建（**不绑定**）。会校验策略与本机设置，错误在此处暴露而不是推迟到首次使用。
    pub fn new(params: ShareEndpointParams) -> Result<Self> {
        // 尽早暴露策略错误（真正绑定时会再装配一次）。
        validate(&params.policy, &params.local)?;
        let id = params.secret_key.public();
        let report_interval = Duration::from_secs(params.policy.report_interval_secs.max(1));
        let sender = ShareSender::new(
            params.registry,
            params.keys,
            params.observers.clone(),
            report_interval,
            params.local.send_rate_limit_bytes_per_sec.unwrap_or(0),
            id,
            params.content,
        );
        Ok(Self {
            shared: Arc::new(Shared {
                secret_key: params.secret_key,
                id,
                policy: params.policy,
                local: params.local,
                client_version: params.client_version,
                observers: params.observers,
                sender,
                lifecycle: tokio::sync::Mutex::new(()),
                current: RwLock::new(None),
                snapshots: watch::channel(None).0,
            }),
        })
    }

    /// 本端 endpoint ID（由私钥决定，无需绑定）。
    pub fn id(&self) -> EndpointId {
        self.shared.id
    }

    /// 本端 endpoint ID 的 z32 文本（与 FluxCloud 契约一致）。
    pub fn id_z32(&self) -> String {
        self.shared.id.to_z32()
    }

    /// 生效的网络策略。
    pub fn policy(&self) -> &NetworkPolicy {
        &self.shared.policy
    }

    /// 发送方句柄（分享登记、限速、强制断开会话）。
    pub fn sender(&self) -> &ShareSender {
        &self.shared.sender
    }

    pub(crate) fn observers(&self) -> &Observers {
        &self.shared.observers
    }

    pub(crate) fn report_interval(&self) -> Duration {
        Duration::from_secs(self.shared.policy.report_interval_secs.max(1))
    }

    /// 已绑定时返回底层 iroh 端点，**不会触发绑定**；宿主可据此直接订阅它的 `Watcher`
    /// （`net_report()` / `home_relay_status()` / `watch_addr()`）或读取 `bound_sockets()`。
    pub fn bound_endpoint(&self) -> Option<Endpoint> {
        self.bound()
            .filter(|b| !b.endpoint.is_closed())
            .map(|b| b.endpoint.clone())
    }

    /// 端点当前是否已绑定。
    pub fn is_bound(&self) -> bool {
        self.bound().is_some_and(|b| !b.endpoint.is_closed())
    }

    fn bound(&self) -> Option<Arc<Bound>> {
        self.shared
            .current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 确保端点已绑定并返回它（首次调用时才真正绑定 socket、启动发布 / 监听任务）。
    pub async fn ensure_bound(&self) -> Result<Endpoint> {
        let _guard = self.shared.lifecycle.lock().await;
        if let Some(bound) = self.bound()
            && !bound.endpoint.is_closed()
        {
            return Ok(bound.endpoint.clone());
        }
        let endpoint = build_endpoint(&self.shared).await?;
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, self.shared.sender.handler())
            .accept(SWARM_ALPN, self.shared.sender.swarm_handler())
            .spawn();
        let cancel = CancellationToken::new();
        tokio::spawn(watch_net(
            self.shared.clone(),
            endpoint.clone(),
            cancel.clone(),
        ));
        tracing::info!(endpoint = %self.shared.id.fmt_short(), "share endpoint bound");
        *self
            .shared
            .current
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(Bound {
            endpoint: endpoint.clone(),
            router,
            cancel,
        }));
        Ok(endpoint)
    }

    /// 关闭端点：断开所有连接、停止发布与监听。之后可再次 [`Self::ensure_bound`]。
    pub async fn shutdown(&self) {
        let _guard = self.shared.lifecycle.lock().await;
        let bound = self
            .shared
            .current
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(bound) = bound else {
            return;
        };
        bound.cancel.cancel();
        if let Err(e) = bound.router.shutdown().await {
            tracing::warn!(error = %e, "share router did not shut down cleanly");
        }
        bound.endpoint.close().await;
        self.shared.snapshots.send_replace(None);
        tracing::info!(endpoint = %self.shared.id.fmt_short(), "share endpoint closed");
    }

    /// 订阅去重后的网络快照（UI 展示用；与上报给 FluxCloud 的是同一份）。端点关闭时为 `None`。
    pub fn subscribe_snapshots(&self) -> watch::Receiver<Option<Arc<NetSnapshot>>> {
        self.shared.snapshots.subscribe()
    }

    /// 立即采集一份完整快照（含最新指标）；端点未绑定时返回 `None`。
    pub fn snapshot(&self) -> Option<NetSnapshot> {
        let bound = self.bound()?;
        Some(collect_snapshot(&self.shared, &bound.endpoint))
    }

    /// 当前 home relay URL（未绑定或无中继时为 `None`）。
    pub fn home_relay(&self) -> Option<String> {
        let bound = self.bound()?;
        home_relay_of(&mut bound.endpoint.home_relay_status()).map(|h| h.url)
    }

    /// 强制重新探测网络（网络切换后宿主可调用）。
    pub async fn network_change(&self) {
        if let Some(bound) = self.bound() {
            bound.endpoint.network_change().await;
        }
    }

    /// 更新共享提供策略：为优先请求保留的连接槽位数、每日提供上限（0 = 不限）与自然日的 UTC 偏移（秒）。
    pub fn set_swarm_policy(&self, priority_reserved: usize, daily_cap: u64, utc_offset_secs: i64) {
        self.shared
            .sender
            .set_swarm_policy(priority_reserved, daily_cap, utc_offset_secs);
    }

    /// 当前共享提供的（日序号，已提供字节），供宿主落盘。
    pub fn swarm_daily_usage(&self) -> (i64, u64) {
        self.shared.sender.swarm_daily_usage()
    }

    /// 启动时恢复落盘的当日共享提供用量。
    pub fn restore_swarm_daily(&self, day: i64, used: u64) {
        self.shared.sender.restore_swarm_daily(day, used);
    }
}

// ───────────────────────── 策略装配 ─────────────────────────

fn validate(policy: &NetworkPolicy, local: &LocalEndpointConfig) -> Result<()> {
    relay_map(policy, local)?;
    build_lookup_resolver(policy)?;
    for extra in &local.extra_addrs {
        extra
            .parse::<SocketAddr>()
            .with_context(|| format!("invalid extra address {extra:?}"))?;
    }
    if !policy.pkarr_url.trim().is_empty() {
        policy
            .pkarr_url
            .parse::<url::Url>()
            .with_context(|| format!("invalid pkarrUrl {:?}", policy.pkarr_url))?;
    }
    Ok(())
}

/// 把策略里的中继列表转成 `RelayMap`；`pinned_relay` 非空时只保留该节点。
fn relay_map(policy: &NetworkPolicy, local: &LocalEndpointConfig) -> Result<Option<RelayMap>> {
    let nodes: Vec<&RelayNode> = match &local.pinned_relay {
        Some(pinned) => {
            let want = pinned.trim_end_matches('/');
            let found: Vec<_> = policy
                .relays
                .iter()
                .filter(|n| n.url.trim_end_matches('/') == want)
                .collect();
            ensure!(
                !found.is_empty(),
                "pinned relay {pinned} is not in the network policy"
            );
            found
        }
        None => policy.relays.iter().collect(),
    };
    if nodes.is_empty() {
        return Ok(None);
    }
    let mut configs = Vec::with_capacity(nodes.len());
    for node in nodes {
        let url: RelayUrl = node
            .url
            .parse()
            .with_context(|| format!("invalid relay url {:?}", node.url))?;
        // `RelayConfig` 是 non_exhaustive 且 QAD 端口类型不对外导出：经 serde 构造。
        let mut json = serde_json::json!({ "url": url });
        if let Some(port) = node.quic_port {
            json["quic"] = serde_json::json!({ "port": port });
        }
        if let Some(token) = &node.auth_token {
            json["auth_token"] = serde_json::Value::String(token.clone());
        }
        let config: RelayConfig = serde_json::from_value(json)
            .with_context(|| format!("cannot build relay config for {}", node.url))?;
        configs.push(config);
    }
    Ok(Some(configs.into_iter().collect()))
}

fn parse_nameserver_addr(text: &str, default_port: u16) -> Result<(IpAddr, u16)> {
    if let Ok(sa) = text.parse::<SocketAddr>() {
        return Ok((sa.ip(), sa.port()));
    }
    let ip: IpAddr = text
        .trim_matches(['[', ']'])
        .parse()
        .with_context(|| format!("invalid nameserver address {text:?}"))?;
    Ok((ip, default_port))
}

/// 地址查找专用解析器：只用策略里的 nameserver，关闭任何公共回落，不读系统配置。
fn build_lookup_resolver(policy: &NetworkPolicy) -> Result<Option<DnsResolver>> {
    let doh_host = policy
        .doh_url
        .as_deref()
        .filter(|u| !u.trim().is_empty())
        .map(|u| {
            let url: url::Url = u.parse().with_context(|| format!("invalid dohUrl {u:?}"))?;
            Ok::<_, anyhow::Error>((
                url.host_str().map(str::to_owned),
                url.port_or_known_default().unwrap_or(443),
            ))
        })
        .transpose()?;
    let mut configs: Vec<NameserverConfig> = Vec::new();
    for ns in &policy.nameservers {
        let cfg = match ns.protocol {
            NameserverProtocol::Udp => {
                let (ip, port) = parse_nameserver_addr(&ns.addr, 53)?;
                NameserverConfig::udp(ip).with_port(port)
            }
            NameserverProtocol::Tcp => {
                let (ip, port) = parse_nameserver_addr(&ns.addr, 53)?;
                NameserverConfig::tcp(ip).with_port(port)
            }
            NameserverProtocol::Https => {
                let (ip, port) = parse_nameserver_addr(&ns.addr, 443)?;
                let cfg = NameserverConfig::https(ip).with_port(port);
                match &doh_host {
                    Some((Some(host), _)) => cfg.with_tls_server_name(host.clone()),
                    _ => cfg,
                }
            }
        };
        configs.push(cfg);
    }
    if let Some((Some(host), port)) = &doh_host
        && let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
    {
        configs.push(NameserverConfig::https(ip).with_port(*port));
    }
    let lookup_needed = !policy.dns_origin.trim().is_empty();
    if configs.is_empty() {
        ensure!(
            !lookup_needed,
            "the network policy sets dnsOrigin but lists no nameserver"
        );
        return Ok(None);
    }
    Ok(Some(
        DnsResolver::builder()
            .add_nameserver_configs(configs)
            .disable_fallback()
            .build(),
    ))
}

async fn build_endpoint(shared: &Shared) -> Result<Endpoint> {
    let policy = &shared.policy;
    let local = &shared.local;
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(shared.secret_key.clone())
        .alpns(vec![ALPN.to_vec(), SWARM_ALPN.to_vec()]);

    // 端点级解析器（中继 / QAD 主机名、pkarr 服务器主机名）：只用系统配置，禁用公共回落。
    builder = builder.dns_resolver(
        DnsResolver::builder()
            .with_system_defaults()
            .disable_fallback()
            .build(),
    );

    let relays = relay_map(policy, local)?;
    builder = match (local.mode, relays) {
        (ConnectionMode::Auto, Some(map)) => builder.relay_mode(RelayMode::Custom(map)),
        (ConnectionMode::Auto, None) | (ConnectionMode::DirectOnly, _) => {
            builder.relay_mode(RelayMode::Disabled)
        }
        (ConnectionMode::RelayOnly, Some(map)) => builder
            .relay_mode(RelayMode::Custom(map))
            .clear_ip_transports(),
        (ConnectionMode::RelayOnly, None) => {
            bail!("relay-only mode needs at least one relay in the network policy")
        }
    };

    if let Some(port) = local.udp_port_v4.filter(|p| *p != 0) {
        builder = builder
            .bind_addr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
            .map_err(|e| anyhow::anyhow!("invalid IPv4 UDP bind port {port}: {e:#}"))?;
    }
    if let Some(port) = local.udp_port_v6.filter(|p| *p != 0) {
        builder = builder
            .bind_addr(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
            .map_err(|e| anyhow::anyhow!("invalid IPv6 UDP bind port {port}: {e:#}"))?;
    }
    for extra in &local.extra_addrs {
        let addr: SocketAddr = extra
            .parse()
            .with_context(|| format!("invalid extra address {extra:?}"))?;
        builder = builder.external_addr(addr);
    }
    if !local.portmapper.unwrap_or(policy.portmapper) {
        builder = builder.portmapper_config(PortmapperConfig::Disabled);
    }

    // 地址发布：只发到策略里的 pkarr 服务（仅直连模式下没有中继 URL，必须发布直连地址才有意义）。
    if !policy.pkarr_url.trim().is_empty() {
        let url: url::Url = policy
            .pkarr_url
            .parse()
            .with_context(|| format!("invalid pkarrUrl {:?}", policy.pkarr_url))?;
        let publish_direct =
            policy.publish_direct_addrs || local.mode == ConnectionMode::DirectOnly;
        let filter = if publish_direct {
            AddrFilter::unfiltered()
        } else {
            AddrFilter::relay_only()
        };
        builder = builder.address_lookup(
            PkarrPublisher::builder(url)
                .ttl(policy.pkarr_ttl_secs)
                .republish_interval(Duration::from_secs(policy.republish_secs.max(1)))
                .addr_filter(filter),
        );
    }
    // 地址查找：origin 域 + 只用策略 nameserver 的解析器。
    if !policy.dns_origin.trim().is_empty() {
        let resolver = build_lookup_resolver(policy)?
            .context("dnsOrigin is set but no nameserver is available")?;
        builder = builder.address_lookup(
            DnsAddressLookup::builder(policy.dns_origin.clone()).dns_resolver(resolver),
        );
    }

    builder
        .bind()
        .await
        .map_err(|e| anyhow::anyhow!("share endpoint bind failed: {e:#}"))
}

// ───────────────────────── 快照 ─────────────────────────

/// 由 `mapping_varies_by_dest` 推断 NAT 类型。
pub fn nat_type_of(mapping_varies_by_dest: Option<bool>) -> NatType {
    match mapping_varies_by_dest {
        Some(false) => NatType::Cone,
        Some(true) => NatType::Symmetric,
        None => NatType::Unknown,
    }
}

fn net_report_info(report: &NetReport) -> NetReportInfo {
    let mapping = report.mapping_varies_by_dest();
    let mut latency: BTreeMap<String, u64> = BTreeMap::new();
    for (_probe, url, d) in report.relay_latency.iter() {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        latency
            .entry(url.to_string())
            .and_modify(|cur| *cur = (*cur).min(ms))
            .or_insert(ms);
    }
    NetReportInfo {
        udp_v4: report.udp_v4,
        udp_v6: report.udp_v6,
        mapping_varies_by_dest: mapping,
        nat_type: nat_type_of(mapping),
        preferred_relay: report.preferred_relay.as_ref().map(ToString::to_string),
        relay_latency_ms: latency,
        global_v4: report.global_v4.map(|a| a.to_string()),
        global_v6: report.global_v6.map(|a| a.to_string()),
        captive_portal: report.captive_portal,
    }
}

fn home_relay_of(
    watcher: &mut impl iroh::Watcher<Value = Vec<iroh::endpoint::RelayStatus>>,
) -> Option<HomeRelayInfo> {
    let statuses = watcher.get();
    let pick = statuses
        .iter()
        .find(|s| s.is_connected())
        .or_else(|| statuses.first())?;
    Some(HomeRelayInfo {
        url: pick.url().to_string(),
        connected: pick.is_connected(),
    })
}

fn addrs_of(addr: &EndpointAddr) -> Vec<String> {
    let mut out: Vec<String> = addr
        .addrs
        .iter()
        .map(|a| match a {
            TransportAddr::Ip(sa) => sa.to_string(),
            TransportAddr::Relay(url) => url.to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    out.sort();
    out
}

/// 序列化全部 iroh 指标：遍历所有指标组与计数器，不做挑选。
fn metrics_of(endpoint: &Endpoint) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    for (group, item) in endpoint.metrics().iter() {
        let key = format!("{group}.{}", item.name());
        match item.value() {
            MetricValue::Counter(v) => {
                out.insert(key, serde_json::Value::from(v));
            }
            MetricValue::Gauge(v) => {
                out.insert(key, serde_json::Value::from(v));
            }
            MetricValue::Histogram { sum, count, .. } => {
                out.insert(format!("{key}.count"), serde_json::Value::from(count));
                if let Some(sum) = serde_json::Number::from_f64(sum) {
                    out.insert(format!("{key}.sum"), serde_json::Value::Number(sum));
                }
            }
            other => {
                out.insert(key, serde_json::Value::String(format!("{other:?}")));
            }
        }
    }
    out
}

fn platform_name() -> String {
    // `std::env::consts::OS` 的取值已是 windows / macos / linux / android / ios 等小写名。
    std::env::consts::OS.to_owned()
}

fn compose_snapshot(
    shared: &Shared,
    endpoint: &Endpoint,
    report: Option<&NetReport>,
    home_relay: Option<HomeRelayInfo>,
    addr: &EndpointAddr,
) -> NetSnapshot {
    NetSnapshot {
        captured_at: now_rfc3339(),
        endpoint_id: shared.id.to_z32(),
        client_version: shared.client_version.clone(),
        platform: platform_name(),
        net_report: report.map(net_report_info),
        home_relay,
        addrs: addrs_of(addr),
        metrics: metrics_of(endpoint),
    }
}

fn collect_snapshot(shared: &Shared, endpoint: &Endpoint) -> NetSnapshot {
    let report = endpoint.net_report().get();
    let home_relay = home_relay_of(&mut endpoint.home_relay_status());
    compose_snapshot(
        shared,
        endpoint,
        report.as_ref(),
        home_relay,
        &endpoint.addr(),
    )
}

/// 订阅 net report / home relay / 本地地址三个 watcher，状态落定后产出去重的快照。
async fn watch_net(shared: Arc<Shared>, endpoint: Endpoint, cancel: CancellationToken) {
    let mut net = endpoint.net_report();
    let mut relay = endpoint.home_relay_status();
    let mut addr = endpoint.watch_addr();
    let mut last: Option<Arc<NetSnapshot>> = None;
    loop {
        let report = net.get();
        let home_relay = home_relay_of(&mut relay);
        let current_addr = addr.get();
        // 没有中继可探测时 net report 永远不会来，不能无限等它。
        let ready = report.is_some() || shared.policy.relays.is_empty();
        if ready {
            let snapshot = compose_snapshot(
                &shared,
                &endpoint,
                report.as_ref(),
                home_relay,
                &current_addr,
            );
            if last.as_ref().is_none_or(|prev| !prev.same_state(&snapshot)) {
                let snapshot = Arc::new(snapshot);
                shared.observers.report_net(&snapshot);
                shared.snapshots.send_replace(Some(snapshot.clone()));
                last = Some(snapshot);
            }
        }
        tokio::select! {
            r = net.updated() => if r.is_err() { break },
            r = relay.updated() => if r.is_err() { break },
            r = addr.updated() => if r.is_err() { break },
            () = cancel.cancelled() => break,
        }
        // 合并连续变化。
        tokio::select! {
            () = tokio::time::sleep(SETTLE) => {}
            () = cancel.cancelled() => break,
        }
    }
    tracing::debug!("share net watcher stopped");
}
