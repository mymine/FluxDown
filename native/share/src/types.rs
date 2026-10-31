//! FluxMesh的数据类型（serde camelCase，对应设计文档 §9 契约）。
//!
//! 这些类型同时是 FluxCloud 控制面 JSON 的镜像、客户端 UI 展示结构和上报结构：
//! “展示即上报”，`NetSnapshot` / `SessionSummary` / `SessionSample` 在三处使用同一份。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hashing::CHUNK_SIZE;

// ───────────────────────── 网络策略（`GET /share/config` 的 `network`） ─────────────────────────

/// FluxCloud 下发的客户端网络策略（`/share/config` 的 `network` 段）。
///
/// 端点只会使用这里列出的中继、pkarr 与 DNS；不存在任何内置的 n0 / iroh.link 回退。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    /// 自建中继节点（至少一个才能做 NAT 探测与中继兜底）。
    #[serde(default)]
    pub relays: Vec<RelayNode>,
    /// `DnsAddressLookup` 的 origin 域名，例如 `dns.fluxdown.com`。
    pub dns_origin: String,
    /// `PkarrPublisher` 的发布地址（`PUT <pkarrUrl>/<z32>`）。
    pub pkarr_url: String,
    /// 解析 origin 与 pkarr 主机名时使用的 nameserver（唯一来源，不回落到系统 / 公共 DNS）。
    #[serde(default)]
    pub nameservers: Vec<Nameserver>,
    /// DoH 入口 URL；其主机名用作 `https` 类 nameserver 的 TLS 服务器名，
    /// 主机名是 IP 字面量时本身也作为一个 DoH nameserver。
    #[serde(default)]
    pub doh_url: Option<String>,
    /// pkarr 记录 TTL（秒）。
    #[serde(default = "default_pkarr_ttl")]
    pub pkarr_ttl_secs: u32,
    /// pkarr 即使无变化也重发布的间隔（秒）。
    #[serde(default = "default_republish")]
    pub republish_secs: u64,
    /// 是否把直连 IP 地址一并发布到 pkarr（否则只发布中继 URL）。
    #[serde(default)]
    pub publish_direct_addrs: bool,
    /// 默认是否启用 portmapper（可被本机设置覆盖）。
    #[serde(default)]
    pub portmapper: bool,
    /// 打洞超过该秒数仍走中继时，UI 提示切换桥接。
    #[serde(default = "default_hole_punch_hint")]
    pub hole_punch_hint_secs: u64,
    /// 传输进行中的会话摘要上报间隔（秒）。
    #[serde(default = "default_report_interval")]
    pub report_interval_secs: u64,
}

fn default_pkarr_ttl() -> u32 {
    30
}
fn default_republish() -> u64 {
    300
}
fn default_hole_punch_hint() -> u64 {
    8
}
fn default_report_interval() -> u64 {
    5
}

/// 中继节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayNode {
    /// 节点 ID（FluxCloud 内部标识）。
    #[serde(default)]
    pub node_id: String,
    /// 中继 HTTPS URL。
    pub url: String,
    /// 区域标签，仅用于展示。
    #[serde(default)]
    pub region: String,
    /// QUIC 地址发现（QAD）端口；缺省为 iroh 默认端口 7842。
    #[serde(default)]
    pub quic_port: Option<u16>,
    /// 中继令牌准入使用的 Bearer 令牌（由网关 / 控制面签发，缺省不带）。
    #[serde(default)]
    pub auth_token: Option<String>,
}

/// DNS 服务器。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Nameserver {
    /// `ip` 或 `ip:port`（IPv6 用 `[::1]:53`）；缺省端口按协议取 53 / 443。
    pub addr: String,
    /// 传输协议。
    pub protocol: NameserverProtocol,
}

/// nameserver 传输协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NameserverProtocol {
    /// DNS over UDP。
    Udp,
    /// DNS over TCP。
    Tcp,
    /// DNS over HTTPS。
    Https,
}

// ───────────────────────── 本机端点配置 ─────────────────────────

/// 连接模式（诊断用；默认自动）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionMode {
    /// 直连 + 中继，由 iroh 自动选择。
    #[default]
    Auto,
    /// 只走中继（不绑定任何 IP 传输）。
    RelayOnly,
    /// 只走直连（不使用中继）。
    DirectOnly,
}

/// 本机端点设置（客户端「FluxMesh」设置页，与 FluxCloud 策略无关）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEndpointConfig {
    /// IPv4 UDP 绑定端口，`None` / `0` 为随机端口。
    #[serde(default)]
    pub udp_port_v4: Option<u16>,
    /// IPv6 UDP 绑定端口，`None` / `0` 为随机端口。
    #[serde(default)]
    pub udp_port_v6: Option<u16>,
    /// 是否启用 portmapper（UPnP / NAT-PMP / PCP）；`None` 沿用 [`NetworkPolicy::portmapper`]。
    #[serde(default)]
    pub portmapper: Option<bool>,
    /// 手动公布的额外外部地址（`ip:port`，例如路由器手工做了端口映射）。
    #[serde(default)]
    pub extra_addrs: Vec<String>,
    /// 连接模式。
    #[serde(default)]
    pub mode: ConnectionMode,
    /// 固定使用的中继 URL；`None` 为自动（全部策略中继，按延迟选择）。
    #[serde(default)]
    pub pinned_relay: Option<String>,
    /// 全局发送限速（字节 / 秒），`None` / `0` 不限。
    #[serde(default)]
    pub send_rate_limit_bytes_per_sec: Option<u64>,
}

// ───────────────────────── 清单 ─────────────────────────

/// 一个分享里的单个文件。哈希一律小写 hex。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFile {
    /// 文件名（展示用；不含目录）。
    pub name: String,
    /// 字节数。
    pub size: u64,
    /// 修改时间（Unix 毫秒），发送方用它与大小一起检测“文件被改”。
    pub mtime_ms: i64,
    /// 每 4 MiB 块的 BLAKE3 哈希（hex）；最后一块可以更短。空文件没有块。
    pub chunk_hashes: Vec<String>,
    /// 根哈希：所有块哈希（32 字节原始值）按序拼接后的 BLAKE3（hex）。
    pub root_hash: String,
}

impl ShareFile {
    /// 块数 `ceil(size / 4 MiB)`。
    pub fn chunk_count(&self) -> u64 {
        self.size.div_ceil(CHUNK_SIZE)
    }

    /// 第 `index` 块在文件内的 `[start, end)` 字节范围；越界返回 `None`。
    pub fn chunk_range(&self, index: u64) -> Option<(u64, u64)> {
        if index >= self.chunk_count() {
            return None;
        }
        let start = index.checked_mul(CHUNK_SIZE)?;
        let end = start.saturating_add(CHUNK_SIZE).min(self.size);
        Some((start, end))
    }

    /// 第 `index` 块的期望哈希（32 字节）；缺失或 hex 非法返回 `None`。
    pub fn chunk_hash(&self, index: u64) -> Option<[u8; 32]> {
        let hex = self.chunk_hashes.get(usize::try_from(index).ok()?)?;
        crate::hashing::parse_hash(hex)
    }
}

/// 一个分享的完整清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareManifest {
    /// FluxCloud 分配的分享 ID（票据里的 `sid`）。
    pub share_id: String,
    /// 文件列表，`fileIndex` 即下标。
    pub files: Vec<ShareFile>,
}

// ───────────────────────── 票据 ─────────────────────────

/// 票据负载（`base64url(json)` 的内容）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketPayload {
    /// 版本，目前为 1。
    pub v: u32,
    /// 签发密钥 ID。
    pub kid: String,
    /// 分享 ID。
    pub sid: String,
    /// 接收方 endpoint ID（z32）。
    pub rep: String,
    /// 过期时间（Unix 秒）。
    pub exp: u64,
    /// 作用域：`recv`（接收方直连取数）、`bridge`（网关代接收方取数，`rep` 为网关 endpoint）
    /// 或 `swarm`（共享：向持有同一内容的对端取块，需带 `root`）。
    pub scope: String,
    /// 共享（`swarm`）票据绑定的内容：文件根哈希（64 位小写 hex）；其他作用域为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// 共享（`swarm`）票据的优先标记：1 = 提供方繁忙时可使用保留连接槽位；缺省 0 不序列化。
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub pri: u8,
}

/// 票据：`base64url(payloadJson) "." base64url(ed25519Sig)`，签名对象是 payload JSON 原始字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub(crate) raw: String,
    pub(crate) payload: TicketPayload,
    pub(crate) payload_bytes: Vec<u8>,
    pub(crate) signature: [u8; 64],
}

impl Ticket {
    /// 票据原文（原样放进 `Hello`）。
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// 解析出的负载（**未验签**，验签见 [`crate::verify_ticket`]）。
    pub fn payload(&self) -> &TicketPayload {
        &self.payload
    }
}

// ───────────────────────── 路径 / 会话 ─────────────────────────

/// 当前承载数据的路径类别。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathKind {
    /// 直连，对端地址是内网 / 链路本地地址。
    DirectLan,
    /// 直连，对端是公网地址（含打洞成功）。
    DirectWan,
    /// 经中继。
    Relay,
    /// 尚未选定路径（刚建连）。
    #[default]
    Unknown,
}

impl PathKind {
    /// 是否直连。
    pub fn is_direct(self) -> bool {
        matches!(self, Self::DirectLan | Self::DirectWan)
    }
}

/// 会话角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionRole {
    /// 分享方（发送字节）。
    Sender,
    /// 接收方（接收字节）。
    Receiver,
    /// 共享提供方（共享连接上向对端发送内容块）。
    Swarm,
}

/// 会话字节归属（按路径类别统计，数据方向上的 UDP 载荷字节）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBytes {
    /// 经直连路径的字节。
    pub direct: u64,
    /// 经中继路径的字节。
    pub relay: u64,
    /// 经云端暂存副本（CDN）取得并通过校验的字节（接收方）。
    #[serde(default)]
    pub cdn: u64,
    /// 经网关桥接取得并通过校验的字节（接收方）。
    #[serde(default)]
    pub bridge: u64,
    /// 从共享对端取回并通过校验的字节（接收方）。
    #[serde(default)]
    pub peer: u64,
}

/// 单条路径的统计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathSummary {
    /// 路径编号（连接内唯一）。
    pub id: String,
    /// 路径类别。
    pub kind: PathKind,
    /// 对端传输地址（IP:端口 或 中继 URL）。
    pub remote: String,
    /// 往返时延（毫秒）。
    pub rtt_ms: f64,
    /// 拥塞窗口（字节）。
    pub cwnd: u64,
    /// 当前 MTU。
    pub mtu: u16,
    /// 该路径发送的 UDP 字节。
    pub sent: u64,
    /// 该路径接收的 UDP 字节。
    pub recv: u64,
    /// 丢包数。
    pub lost: u64,
    /// 拥塞事件数。
    pub congestion_events: u64,
    /// 拥塞控制器的带宽估计（bit/s），无估计为 0。
    pub bw_estimate: u64,
}

/// 路径事件时间线条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathEventEntry {
    /// 发生时间（RFC 3339，毫秒精度）。
    pub at: String,
    /// `opened` / `selected` / `closed` / `lagged`。
    pub kind: String,
    /// 相关路径的对端地址（`lagged` 时为空串）。
    pub path: String,
}

/// 会话摘要（§9.4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// 会话开始时间（RFC 3339）。
    pub started_at: String,
    /// 建连耗时毫秒。接收方为 `connect()` 到握手完成；发送方（被动接入）恒为 0。
    pub connect_ms: u64,
    /// 会话开始到首次选中直连路径的毫秒数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_direct_ms: Option<u64>,
    /// 当前路径类别。
    pub path_kind: PathKind,
    /// 按路径类别的字节数。
    pub bytes: SessionBytes,
    /// 各路径统计（含已关闭路径的最终值）。
    pub paths: Vec<PathSummary>,
    /// 路径事件时间线。
    pub events: Vec<PathEventEntry>,
    /// BLAKE3 块校验失败次数（接收方）。
    pub verify_failures: u64,
    /// 终态错误文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 传输中的时序采样。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSample {
    /// 采样时间（RFC 3339）。
    pub at: String,
    /// 会话累计应用层字节数。
    pub bytes: u64,
    /// 自上次采样以来的速率（字节 / 秒）。
    pub rate_bps: f64,
    /// 当前路径 RTT（毫秒）。
    pub rtt_ms: f64,
    /// 当前路径类别。
    pub path_kind: PathKind,
}

/// `POST /share/telemetry/sessions` 的请求体（同时是会话进行中的 UI 快照）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReport {
    /// 会话 ID（本机生成，进行中与终态相同）。
    pub session_id: String,
    /// 分享 ID。
    pub share_id: String,
    /// 本机角色。
    pub role: SessionRole,
    /// 对端 endpoint ID（z32）。
    pub peer_endpoint_id: String,
    /// 摘要。
    pub summary: SessionSummary,
    /// 自上次上报以来新增的采样。
    pub samples: Vec<SessionSample>,
    /// 共享提供方：本连接按内容累计（单调不减）已发送的字节与块数，最多 [`MAX_SWARM_FILES`] 条；
    /// 其他角色为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swarm_files: Vec<SwarmServedFile>,
    /// 共享接收方：逐提供方对端累计字节（按 bytes 降序，最多 [`MAX_PEER_BYTES`] 条）；其他角色为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peer_bytes: Vec<PeerBytes>,
    /// 是否终态。
    #[serde(rename = "final")]
    pub is_final: bool,
}

/// 一条共享连接上单个内容的累计服务量（只计成功发出的 `Get` 数据）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmServedFile {
    /// 文件根哈希（64 位小写 hex）。
    pub root_hash: String,
    /// 已发送的数据字节。
    pub bytes: u64,
    /// 已发送的块数。
    pub chunks: u64,
}

/// 单份会话报告最多携带的 [`SwarmServedFile`] 条数（FluxCloud 的校验上限）。
pub const MAX_SWARM_FILES: usize = 64;

/// 接收方从某个提供方对端收到的累计字节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerBytes {
    /// 提供方 endpoint ID（z32）。
    pub endpoint_id: String,
    /// 累计字节。
    pub bytes: u64,
}

/// 单份会话报告最多携带的 [`PeerBytes`] 条数（FluxCloud 的校验上限）。
pub const MAX_PEER_BYTES: usize = 64;

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

// ───────────────────────── 网络快照（§9.4） ─────────────────────────

/// NAT 类型推断。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NatType {
    /// 映射与目的无关（锥形）。
    Cone,
    /// 映射随目的变化（对称）。
    Symmetric,
    /// 无法判断。
    #[default]
    Unknown,
}

/// net report 摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetReportInfo {
    /// IPv4 QAD 往返成功。
    pub udp_v4: bool,
    /// IPv6 QAD 往返成功。
    pub udp_v6: bool,
    /// 不同探测目标得到的公网映射是否不同（v4 / v6 合并后）。
    pub mapping_varies_by_dest: Option<bool>,
    /// 由 `mapping_varies_by_dest` 推断的 NAT 类型。
    pub nat_type: NatType,
    /// 首选中继 URL。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_relay: Option<String>,
    /// 各中继延迟（毫秒，取各探测方式的最小值）。
    pub relay_latency_ms: BTreeMap<String, u64>,
    /// 全局 IPv4 地址 `ip:port`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_v4: Option<String>,
    /// 全局 IPv6 地址 `[ip]:port`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_v6: Option<String>,
    /// 是否疑似强制门户。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captive_portal: Option<bool>,
}

/// home relay 连接状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeRelayInfo {
    /// 中继 URL。
    pub url: String,
    /// 是否已连接。
    pub connected: bool,
}

/// 端点网络快照：UI 展示与上报 FluxCloud 的同一结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetSnapshot {
    /// 采集时间（RFC 3339）。
    pub captured_at: String,
    /// 本端 endpoint ID（z32）。
    pub endpoint_id: String,
    /// 客户端版本。
    pub client_version: String,
    /// `windows|macos|linux|android|ios`。
    pub platform: String,
    /// 首次 net report 完成前为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_report: Option<NetReportInfo>,
    /// 当前 home relay。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_relay: Option<HomeRelayInfo>,
    /// 本端公布的传输地址（IP 与中继）。
    pub addrs: Vec<String>,
    /// 全部 iroh 指标：键 `<group>.<name>`，直方图展开为 `.count` / `.sum`。
    pub metrics: BTreeMap<String, serde_json::Value>,
}

impl NetSnapshot {
    /// 两份快照在“状态”上是否相同：忽略随时间自然变化的 `capturedAt` 与 `metrics`。
    ///
    /// 上报去重使用该比较：只有网络状态变化才产生新的上报，指标随该次上报附带。
    pub fn same_state(&self, other: &Self) -> bool {
        self.endpoint_id == other.endpoint_id
            && self.client_version == other.client_version
            && self.platform == other.platform
            && self.net_report == other.net_report
            && self.home_relay == other.home_relay
            && self.addrs == other.addrs
    }
}
