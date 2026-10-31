//! FluxMesh（临时文件分享）的 agent RPC / 事件 wire 契约（`agent.share.*`、
//! `AgentEvent::ShareChanged` / `ShareProgress`、`AgentSnapshot.share`）。
//!
//! 本模块只定义 UI 与 agent 之间的纯 serde DTO：不依赖 `fluxdown_share`（agent 侧负责转换），
//! 与 FluxCloud `/share/*` 控制面 JSON 不是同一份契约。约定：
//!
//! - 时间一律 RFC 3339 字符串（与 `NetSnapshot.capturedAt`、云端 `dailyResetAt` 一致）；
//! - 字节数 `u64`，速率单位字节 / 秒；`maxClaims == 0` 表示不限（与云端 `share*` 权益「0 = 不限」一致）；
//! - 进度密集的数据（每个接收会话的速率 / 进度 / 路径）不进 [`ShareStateDto`] 的整段替换事件，
//!   走可合并的 [`ShareProgressDto`]（同一 `sessionId` 的后帧完整取代前帧）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// [`ShareStateDto::receive_records`] 在快照里最多保留的最近记录数；更早的记录经
/// `agent.share.receiveRecords` 翻页读取。
pub const SHARE_STATE_RECEIVE_RECORDS: usize = 20;
/// 面向用户的字节字段展示封顶（1 PiB）。FluxCloud 把额度类字节字段封顶到此值，客户端
/// 遇到 `>=` 此值显示「≥ 1 PB」。
pub const DISPLAY_BYTES_CAP: u64 = 1 << 50;

// ───────────────────────── 枚举 ─────────────────────────

/// 当前承载数据的路径类别（镜像 `fluxdown_share::PathKind`）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum SharePathKind {
    /// 直连，对端是内网 / 链路本地地址。
    DirectLan,
    /// 直连，对端是公网地址（含打洞成功）。
    DirectWan,
    /// 经中继（免费限速）。
    Relay,
    /// 经 CDN / 对象存储的云端暂存副本（按块 HTTP 区间取，块级 BLAKE3 不变）。
    Cdn,
    /// 经 share-gateway 实时桥接（分享方在线、无副本）。
    Bridge,
    /// 分享方未连接，块全部来自持有同一文件的其他设备（共享）。
    Swarm,
    /// 尚未选定路径。
    #[default]
    #[serde(other)]
    Unknown,
}

/// NAT 类型推断（镜像 `fluxdown_share::NatType`）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareNatType {
    /// 映射与目的无关（锥形）。
    Cone,
    /// 映射随目的变化（对称）。
    Symmetric,
    /// 无法判断。
    #[default]
    #[serde(other)]
    Unknown,
}

/// 连接模式（诊断用；默认自动）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareConnectionMode {
    /// 直连 + 中继，由 iroh 自动选择。
    #[default]
    Auto,
    /// 只走中继。
    RelayOnly,
    /// 只走直连（不使用中继）。
    DirectOnly,
}

/// 退出应用时仍有未收完的接收方时的处理。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareExitBehavior {
    /// 弹出确认。
    #[default]
    Prompt,
    /// 自动转为云端暂存（退出前把仍有未收完接收方的实时分享转成暂存并等待上传完成）。
    ConvertToCloud,
    /// 直接退出。
    Quit,
}

/// iroh tracing 日志级别（同时用于 `agent.share.logs` 的过滤下限）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareLogLevel {
    Error,
    #[default]
    Warn,
    Info,
    Debug,
    Trace,
}

/// 分享的传输方式。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareMode {
    /// 实时分享：只从分享方设备取。
    #[default]
    Live,
    /// 云端暂存：分享除分享方设备外还有一份云端副本（`cloudState = ready` 之后）。
    Cloud,
}

/// 分享状态。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareStatus {
    /// 有效。
    #[default]
    Active,
    /// 已过期。
    Expired,
    /// 已被停止（分享方或后台）。
    Stopped,
    /// 已被后台下架（`ShareDto.takedownReason` 为原因；可申诉一次）。
    Takedown,
    /// 对端发送了本端不认识的状态。
    #[serde(other)]
    Unknown,
}

/// FluxMesh流量种类（额度按种类独立记账）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareTrafficKind {
    /// 共享加速额度：从其他用户的共享收到的字节。
    Swarm,
    /// 官方加速流量（CDN / 桥接）。
    Download,
    /// 云端上传流量。
    Upload,
    /// 对端发送了本端不认识的种类。
    #[default]
    #[serde(other)]
    Unknown,
}

/// 额度明细（grant）的来源。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareGrantSource {
    /// 流量包。
    Pack,
    /// 上月未用完的赠送额度结转。
    Rollover,
    /// 共享贡献赚取。
    Contribution,
    /// 后台赠送。
    Admin,
    /// 后台手工免费来源。
    Free,
    /// 对端发送了本端不认识的来源。
    #[default]
    #[serde(other)]
    Unknown,
}

/// 共享加速被云端拒绝的原因（`ShareTaskFactsDto.swarmBlock`）。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareSwarmBlock {
    /// 本月共享加速额度已用完。
    QuotaExhausted,
    /// 需要登录。
    LoginRequired,
    /// 对端发送了本端不认识的原因。
    #[serde(other)]
    Unknown,
}

/// 云端暂存副本的状态（`ShareDto.cloudState`）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareCloudState {
    /// 没有云端副本。
    #[default]
    #[serde(rename = "none")]
    Idle,
    /// 正在上传（进度见 [`ShareDto::upload`]）。
    Uploading,
    /// 副本已就绪，接收方可经 CDN / 浏览器直链取件。
    Ready,
    /// 副本已被删除（分享方删除、过期清理或后台下架）。
    Deleted,
    /// 对端发送了本端不认识的状态。
    #[serde(other)]
    Unknown,
}

/// 云端暂存上传的进度状态。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareUploadStatus {
    /// 正在计算秒传证明 / 上传分片。
    #[default]
    Uploading,
    /// 暂时无法继续（网络中断 / 云端不可达），agent 会自动重试；断点已落盘。
    Paused,
    /// 已失败，需要用户重试（`reason` 说明原因）。
    Failed,
}

/// 云端暂存上传进度。只在上传期间出现，经 `ShareChanged` 下发（节流到约 2 次 / 秒）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareUploadDto {
    pub status: ShareUploadStatus,
    /// 已上传（含秒传与断点续传前已完成的分片）的字节数。
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// 当前上传速率（字节 / 秒），暂停 / 失败为 `0`。
    #[serde(default)]
    pub rate_bps: u64,
    /// `Failed` / `Paused` 时的原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<crate::ErrorReason>,
}

/// 会话角色。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareSessionRole {
    /// 分享方（发送字节）。
    Sender,
    /// 接收方（接收字节）。
    Receiver,
    /// 共享提供方（共享连接上向对端发送内容块）。
    Swarm,
}

/// 接收记录的状态。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareReceiveStatus {
    /// 仍有未完成的任务。
    #[default]
    Active,
    /// 全部任务已完成。
    Completed,
    /// 至少一个任务失败且没有仍在进行的任务。
    Failed,
}

/// 一次接收的共享（共享提供）状态。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareProvideState {
    /// 正在共享。
    Providing,
    /// 原分享已结束或文件已变动，共享自然结束。
    Ended,
    /// 用户停止了共享。
    Stopped,
    /// 未共享。
    #[default]
    #[serde(other)]
    Off,
}

// ───────────────────────── 分享 ─────────────────────────

/// 分享里的单个文件。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareFileDto {
    pub name: String,
    pub size: u64,
}

/// 分享方视角下一个正在取件的接收方会话。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareReceiverDto {
    pub session_id: String,
    /// 接收方设备名；游客或未知为空串。
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub path_kind: SharePathKind,
    /// 当前速率（字节 / 秒）。
    #[serde(default)]
    pub rate_bps: u64,
    /// 已发送的应用层字节数。
    #[serde(default)]
    pub bytes: u64,
    /// 已发送占分享总字节的比例（`0.0..=1.0`）。
    #[serde(default)]
    pub progress: f64,
    /// 会话开始时间（RFC 3339）。
    #[serde(default)]
    pub started_at: String,
}

/// 我的一个分享。同一组任务只有一条规范链接；改设置（`agent.share.update`）不新建。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareDto {
    pub id: String,
    pub slug: String,
    /// 规范分享链接（UI 据此自行渲染二维码）。
    pub url: String,
    /// 6 位取件码。
    pub pickup_code: String,
    pub title: String,
    pub files: Vec<ShareFileDto>,
    pub total_bytes: u64,
    #[serde(default)]
    pub mode: ShareMode,
    /// 有效期档位 ID（取自 [`SharePolicySummaryDto::live_tiers`]）。
    pub tier_id: String,
    /// 过期时间（RFC 3339）。
    pub expires_at: String,
    /// 领取次数上限；`0` = 不限。
    #[serde(default)]
    pub max_claims: u32,
    #[serde(default)]
    pub claim_count: u32,
    #[serde(default)]
    pub notify_on_claim: bool,
    #[serde(default)]
    pub status: ShareStatus,
    /// 最近一次被访问（resolve / 取件）的时间（RFC 3339）；从未访问为 `None`。
    #[serde(default)]
    pub last_accessed_at: Option<String>,
    /// 来源的本机已完成任务 ID（按分享内文件顺序；重启恢复后可能为空）。
    #[serde(default)]
    pub task_ids: Vec<String>,
    /// 正在取件的接收方会话（实时数据经 [`ShareProgressDto`] 刷新）。
    #[serde(default)]
    pub active_receivers: Vec<ShareReceiverDto>,
    /// 云端副本状态（阶段 E）。
    #[serde(default)]
    pub cloud_state: ShareCloudState,
    /// 云端已存储 / 计费的字节数。
    #[serde(default)]
    pub cloud_bytes: u64,
    /// 云端暂存上传进度；没有进行中的上传时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<ShareUploadDto>,
    /// 本机源文件已缺失或已被修改，实时分享无法提供，也无法重新开始；此时只能删除该分享。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub source_missing: bool,
    /// 该分享是否加入共享（其他持有同一文件的设备可互相取块）。
    #[serde(default)]
    pub swarm: bool,
    /// 管理员已关闭并锁定该分享的共享：此时 `swarm` 恒为 `false`，且不能再打开。
    #[serde(default)]
    pub swarm_locked: bool,
    /// 下架原因（仅 `status = takedown` 时存在）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takedown_reason: Option<String>,
    /// 下架申诉（提交过才有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appeal: Option<ShareAppealDto>,
}

/// 申诉处理状态。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareAppealStatus {
    /// 待处理。
    #[default]
    Pending,
    /// 已通过（分享已恢复）。
    Approved,
    /// 已驳回。
    Rejected,
    /// 对端发送了本端不认识的状态。
    #[serde(other)]
    Unknown,
}

/// 一份分享的下架申诉。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareAppealDto {
    #[serde(default)]
    pub status: ShareAppealStatus,
    /// 提交时间（RFC 3339）。
    #[serde(default)]
    pub created_at: String,
    /// 处理时间（RFC 3339）。
    #[serde(default)]
    pub handled_at: Option<String>,
    /// 后台处理备注。
    #[serde(default)]
    pub admin_note: Option<String>,
}

/// 一个有效期档位。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareTierDto {
    pub id: String,
    /// 有效小时数。
    pub hours: u32,
}

/// FluxCloud 下发的客户端策略摘要（UI 只需要展示与约束，不含中继令牌等敏感字段）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SharePolicySummaryDto {
    /// 云端总开关。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub guest_receive: bool,
    #[serde(default)]
    pub guest_bridge: bool,
    #[serde(default)]
    pub guest_share: bool,
    #[serde(default)]
    pub pickup_code_length: u8,
    #[serde(default)]
    pub live_tiers: Vec<ShareTierDto>,
    /// 云端暂存的保留期档位（`cloud-…`，≤ 336 小时）。
    #[serde(default)]
    pub retention_tiers: Vec<ShareTierDto>,
    /// 云端存储已配置：`false` 时 UI 隐藏「转为云端暂存」。
    #[serde(default)]
    pub cloud_enabled: bool,
    /// 可用中继 URL 列表（「手动选择中继」下拉的候选）。
    #[serde(default)]
    pub relays: Vec<ShareRelayDto>,
    /// 打洞超过该秒数仍走中继时提示切换桥接。
    #[serde(default)]
    pub hole_punch_hint_secs: u64,
    /// 提供方为「优先」请求保留的连接数（云端下发，0 = 不保留）。
    #[serde(default)]
    pub swarm_priority_reserved_slots: u32,
    /// 「每日共享上传上限」的云端缺省值（字节，0 = 不限）。
    #[serde(default)]
    pub provide_daily_cap_default_bytes: u64,
}

/// 策略中的一个中继节点（仅展示）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareRelayDto {
    pub url: String,
    #[serde(default)]
    pub region: String,
}

/// 本机FluxMesh设置（客户端「FluxMesh」设置页；与 FluxCloud 策略无关）。`agent.share.setLocalConfig`
/// 整体替换它，UI 以快照里的当前值为底稿修改后整体提交。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareLocalConfigDto {
    /// 本机启用FluxMesh（实验性功能，默认关、须用户手动开启）。关闭时 agent 不访问 FluxCloud 的
    /// FluxMesh接口、不绑定端点、不登记 / 上报任何内容；`ShareStateDto` 只带本机设置（其余为空、
    /// `available = false`），`agent.share.*` 除 `config` / `setLocalConfig` 外一律 `ShareUnavailable`。
    #[serde(default)]
    pub enabled: bool,
    /// IPv4 UDP 绑定端口，`None` / `0` 为随机端口。
    #[serde(default)]
    pub udp_port_v4: Option<u16>,
    /// IPv6 UDP 绑定端口，`None` / `0` 为随机端口。
    #[serde(default)]
    pub udp_port_v6: Option<u16>,
    /// 是否启用 portmapper；`None` 沿用云端策略默认值。
    #[serde(default)]
    pub portmapper: Option<bool>,
    /// 手动公布的额外外部地址（`ip:port`）。
    #[serde(default)]
    pub extra_addrs: Vec<String>,
    #[serde(default)]
    pub mode: ShareConnectionMode,
    /// 固定使用的中继 URL；`None` = 自动（按延迟选择）。
    #[serde(default)]
    pub pinned_relay: Option<String>,
    /// 全局发送限速（字节 / 秒），`None` / `0` 不限。
    #[serde(default)]
    pub send_rate_limit_bytes_per_sec: Option<u64>,
    #[serde(default)]
    pub exit_behavior: ShareExitBehavior,
    #[serde(default)]
    pub iroh_log_level: ShareLogLevel,
    /// 新建分享的默认有效期档位；`None` = 使用云端策略第一个档位。
    #[serde(default)]
    pub default_tier_id: Option<String>,
    /// 新建分享的默认领取次数上限；`0` = 不限。
    #[serde(default)]
    pub default_max_claims: u32,
    /// 新建分享的默认「领取通知」。
    #[serde(default)]
    pub default_notify_on_claim: bool,
    /// 新建分享默认加入共享；`None` = 开。
    #[serde(default)]
    pub default_swarm: Option<bool>,
    /// 接收的文件参与共享（共享）；`None` = 开。
    #[serde(default)]
    pub auto_provide: Option<bool>,
    /// 每日共享上传上限（只计为他人提供）：`None` = 跟随云端缺省，`Some(0)` = 不限，`Some(n)` = 字节。
    #[serde(default)]
    pub provide_daily_cap_bytes: Option<u64>,
    /// 「自然日」划分用的本地时区偏移（秒，UTC+8 = 28800），由 UI 每次提交时写入；`None` 按 UTC。
    #[serde(default)]
    pub provide_day_offset_secs: Option<i32>,
}

/// 本机端点网络状态（展示与上报 FluxCloud 的快照的同源摘要；端点未绑定时多为空）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareNetStatusDto {
    /// iroh 端点是否已绑定（空闲静默：无分享 / 接收任务时关闭为 `false`）。
    #[serde(default)]
    pub bound: bool,
    /// 是否已连上 home relay 或取得公网映射，即别人此刻能找到本机。
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub home_relay: Option<ShareHomeRelayDto>,
    #[serde(default)]
    pub nat_type: ShareNatType,
    /// 云端按本机全局地址查得的运营商 / 省份（GeoIP）。
    #[serde(default)]
    pub isp: Option<String>,
    #[serde(default)]
    pub province: Option<String>,
    /// IPv6 UDP 可用。
    #[serde(default)]
    pub ipv6: bool,
    /// 各中继延迟（毫秒），键为中继 URL。
    #[serde(default)]
    pub relay_latency_ms: BTreeMap<String, u64>,
}

/// home relay 连接状态。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareHomeRelayDto {
    pub url: String,
    pub connected: bool,
}

/// 一个下载任务上的FluxMesh事实（任务行来源徽标 / 「切换桥接」 / 「停止共享」 / 传输诊断页的入口）。
/// 只有FluxMesh接收任务（`isShareReceive`）才有条目；键为 daemon 任务 ID。进行中的接收任务都有；
/// 已完成的只在其接收记录仍在共享（`provideState = providing`）时保留。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareTaskFactsDto {
    #[serde(default)]
    pub is_share_receive: bool,
    #[serde(default)]
    pub path_kind: SharePathKind,
    #[serde(default)]
    pub peer_nat_type: ShareNatType,
    #[serde(default)]
    pub peer_isp: Option<String>,
    #[serde(default)]
    pub share_id: Option<String>,
    /// 当前承载该任务的会话 ID（`agent.share.diagnostics` 的 `sessionId` 过滤键）。
    #[serde(default)]
    pub session_id: Option<String>,
    /// 打洞超过 `policy.holePunchHintSecs` 仍走中继，UI 据此显示「切换桥接」提示。
    #[serde(default)]
    pub relay_stuck: bool,
    /// 当前在线、可共享取块的其他设备数。
    #[serde(default)]
    pub swarm_peers: u32,
    /// 共享加速被云端拒绝的原因（查找成功后清空）；任务仍从分享方直连继续。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_block: Option<ShareSwarmBlock>,
    /// 所属接收记录 ID（`agent.share.stopProviding` / 举报的定位键）；旧载荷缺省为 `None`。
    #[serde(default)]
    pub record_id: Option<String>,
    /// 分享方设备名；未知为空串。
    #[serde(default)]
    pub sender_device_name: String,
    /// 所属接收记录的共享（共享提供）状态。
    #[serde(default)]
    pub provide_state: ShareProvideState,
}

/// 创建分享时正在做块哈希的任务，`agent.share.create` 返回前出现，完成后移除。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareHashingDto {
    pub id: String,
    pub title: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// 一条接收记录。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareReceiveRecordDto {
    pub id: String,
    #[serde(default)]
    pub share_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub sender_device_name: String,
    #[serde(default)]
    pub file_count: u32,
    #[serde(default)]
    pub total_bytes: u64,
    /// 由本次接收建出的 daemon 任务 ID。
    #[serde(default)]
    pub task_ids: Vec<String>,
    /// 开始接收时间（RFC 3339）。
    pub started_at: String,
    #[serde(default)]
    pub status: ShareReceiveStatus,
    /// 共享（共享提供）状态；由 agent 在下发时叠加，旧载荷缺省为 `off`。
    #[serde(default)]
    pub provide_state: ShareProvideState,
}

/// 「额度与贡献」：`GET /share/usage` 的投影（`ShareStateDto.usage`）。字节字段已由云端封顶到
/// [`DISPLAY_BYTES_CAP`]。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareUsageDto {
    /// 计费月份 `YYYY-MM`（北京时间自然月）。
    #[serde(default)]
    pub month: String,
    /// 本月额度重置时刻（RFC3339）。
    #[serde(default)]
    pub reset_at: String,
    #[serde(default)]
    pub enforced: ShareUsageEnforcedDto,
    /// 固定顺序：swarm、download、upload。
    #[serde(default)]
    pub kinds: Vec<ShareKindUsageDto>,
    /// 有效且余额 > 0 的额度明细，按消耗顺序。
    #[serde(default)]
    pub grants: Vec<ShareGrantDto>,
    #[serde(default)]
    pub contribution: ShareContributionDto,
}

/// 云端当前是否按额度拦截。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ShareUsageEnforcedDto {
    /// 官方加速 / 云端上传流量超额即拒绝。
    pub traffic: bool,
    /// 共享加速额度超额即拒绝。
    pub swarm: bool,
}

/// 单种流量的本月额度。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ShareKindUsageDto {
    pub kind: ShareTrafficKind,
    /// 月额度为 0（不限）。
    pub unlimited: bool,
    /// 本月赠送额度。
    pub monthly_allowance: u64,
    pub used_this_month: u64,
    /// 剩余（本月赠送剩余 + 有效 grant 余额 − 预占），下限 0。
    pub remaining: u64,
    /// 进行中预占。
    pub reserved: u64,
}

/// 一笔有效额度明细。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ShareGrantDto {
    pub source: ShareGrantSource,
    pub kind: ShareTrafficKind,
    pub bytes: u64,
    pub used_bytes: u64,
    /// 到期时刻（RFC3339），`None` 为长期有效。
    pub expires_at: Option<String>,
}

/// 共享贡献与等级。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ShareContributionDto {
    pub month_bytes: u64,
    pub last30d_bytes: u64,
    pub lifetime_bytes: u64,
    pub level: u32,
    pub level_max: u32,
    /// 下一级所需的终身贡献；已满级为 `None`。
    pub next_level_bytes: Option<u64>,
    /// 当前生效的贡献赚取加成（‰，1000 = ×1）。
    pub boost_permille: u32,
    pub earn: Vec<ShareEarnDto>,
    /// 提供方繁忙时本账号享有优先。
    pub priority: bool,
    /// 云端已启用优先保留槽位。
    pub priority_active: bool,
    /// 贡献带来优先的门槛（0 = 贡献不带来优先）。
    pub priority_threshold_bytes: u64,
    pub priority_window_days: u32,
}

/// 单种流量的贡献赚取规则与本月进度。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ShareEarnDto {
    pub kind: ShareTrafficKind,
    pub permille: u32,
    pub earned_this_month: u64,
    /// 每月赚取上限，0 = 不限。
    pub cap_bytes: u64,
    pub valid_months: u32,
}

/// FluxMesh分区的完整状态（`AgentSnapshot.share`，`AgentEvent::ShareChanged` 整体替换）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareStateDto {
    /// 云端策略当前允许FluxMesh（`/share/config.enabled`）。
    #[serde(default)]
    pub enabled: bool,
    /// 本构建已编入FluxMesh、agent 的FluxMesh服务在运行、本机已开启（`localConfig.enabled`），
    /// 且云端允许使用（可接收）。为 `false` 时 UI 隐藏全部FluxMesh入口（设置页的开启开关除外）。
    #[serde(default)]
    pub available: bool,
    /// 本机当前可发起分享（已登录，或云端允许游客发起）。
    #[serde(default)]
    pub can_send: bool,
    #[serde(default)]
    pub local_config: ShareLocalConfigDto,
    #[serde(default)]
    pub policy_summary: SharePolicySummaryDto,
    /// 我的分享（含已过期 / 已停止的最近条目）。
    #[serde(default)]
    pub shares: Vec<ShareDto>,
    #[serde(default)]
    pub net_status: ShareNetStatusDto,
    /// daemon 任务 ID → FluxMesh事实（只含FluxMesh接收任务）。
    #[serde(default)]
    pub task_facts: BTreeMap<String, ShareTaskFactsDto>,
    /// 最近的接收记录（新的在前，至多 [`SHARE_STATE_RECEIVE_RECORDS`] 条）。
    #[serde(default)]
    pub receive_records: Vec<ShareReceiveRecordDto>,
    /// 链接 / 深链打开后 agent 已解析、等待 UI 确认的接收预览；`agent.share.receive{resolveId}`
    /// 成功、`agent.share.dismissPending` 或被新链接取代后清空。
    #[serde(default)]
    pub pending_receive: Option<ShareResolveDto>,
    /// 创建分享时正在做块哈希的任务，`agent.share.create` 返回前出现，完成后移除。
    #[serde(default)]
    pub hashing: Vec<ShareHashingDto>,
    /// 最近一次成功拉取的FluxMesh额度与贡献；未登录为 `None`。装箱以免撑大 `AgentEvent::ShareChanged`。
    #[serde(default)]
    pub usage: Option<Box<ShareUsageDto>>,
    /// 最近一次额度拉取失败的原因（成功后清空；保留旧 `usage`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_error: Option<crate::ErrorReason>,
}

impl ShareStateDto {
    /// 应用一条会话进度：刷新匹配 `sessionId` 的接收方行与任务事实的路径。未知会话忽略。
    /// 后帧完整取代前帧，所以同一会话的相邻 [`ShareProgressDto`] 可在帧队列里合并。
    pub fn apply_progress(&mut self, progress: &ShareProgressDto) {
        for share in &mut self.shares {
            for receiver in &mut share.active_receivers {
                if receiver.session_id == progress.session_id {
                    receiver.path_kind = progress.path_kind;
                    receiver.rate_bps = progress.rate_bps;
                    receiver.bytes = progress.bytes;
                    receiver.progress = progress.progress;
                }
            }
        }
        for facts in self.task_facts.values_mut() {
            if facts.session_id.as_deref() == Some(progress.session_id.as_str()) {
                facts.path_kind = progress.path_kind;
            }
        }
    }
}

/// 单个会话的进度 / 路径刷新（`AgentEvent::ShareProgress`，可合并：同 `sessionId` 的后帧完整取代前帧）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareProgressDto {
    pub session_id: String,
    pub path_kind: SharePathKind,
    /// 当前速率（字节 / 秒）。
    pub rate_bps: u64,
    /// 会话累计应用层字节数。
    pub bytes: u64,
    /// 占分享总字节的比例（`0.0..=1.0`）。
    pub progress: f64,
}

// ───────────────────────── RPC 参数 / 结果 ─────────────────────────

/// `agent.share.config` 结果：刷新后的云端策略摘要 + 本机设置。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareConfigDto {
    pub policy: SharePolicySummaryDto,
    pub local_config: ShareLocalConfigDto,
}

/// `agent.share.create` 参数：从已完成的本机任务创建分享；多个任务合成一个分享。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareCreateParams {
    /// 至少一个；全部须是本机已完成且文件仍在的任务，否则 `ShareTaskNotShareable`。
    pub task_ids: Vec<String>,
    /// 缺省取 [`ShareLocalConfigDto::default_tier_id`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<String>,
    /// 缺省取 [`ShareLocalConfigDto::default_max_claims`]；`0` = 不限。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_claims: Option<u32>,
    /// 缺省取 [`ShareLocalConfigDto::default_notify_on_claim`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_on_claim: Option<bool>,
    /// 缺省取 [`ShareLocalConfigDto::default_swarm`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm: Option<bool>,
}

/// `agent.share.update` 参数：改设置不新建分享，链接与取件码不变；缺省字段不改。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareUpdateParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<String>,
    /// `Some(0)` = 不限。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_claims: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_on_claim: Option<bool>,
    /// 是否加入共享。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm: Option<bool>,
}

/// `agent.share.stop` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareIdParams {
    pub id: String,
}

/// `agent.share.stopProviding` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareStopProvidingParams {
    pub record_id: String,
}

/// 举报原因（固定枚举；agent 映射为云端规范中文串）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum ShareReportReason {
    /// 侵权。
    Copyright,
    /// 色情低俗。
    Porn,
    /// 违法违规。
    Illegal,
    /// 恶意软件。
    Malware,
    /// 其他。
    #[default]
    Other,
}

impl ShareReportReason {
    /// 全部原因（UI 单选顺序）。
    pub const ALL: [Self; 5] = [
        Self::Copyright,
        Self::Porn,
        Self::Illegal,
        Self::Malware,
        Self::Other,
    ];

    /// 云端（官网落地页同口径）规范提交值。
    #[must_use]
    pub const fn canonical(self) -> &'static str {
        match self {
            Self::Copyright => "侵权",
            Self::Porn => "色情低俗",
            Self::Illegal => "违法违规",
            Self::Malware => "恶意软件",
            Self::Other => "其他",
        }
    }
}

/// `agent.share.report` 参数：举报一份已领取的分享（以本机 endpoint 签名提交）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareReportParams {
    pub share_id: String,
    pub reason: ShareReportReason,
    /// 补充说明（≤ 2000 字）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 联系方式（≤ 200 字）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
}

/// `agent.share.appeal` 参数：对已下架的分享申诉（每个分享一次）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareAppealParams {
    pub share_id: String,
    /// 申诉说明（非空，≤ 1000 字）。
    pub statement: String,
}

/// `agent.share.convertToCloud` 参数：把一个实时分享转为云端暂存并开始上传。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareConvertToCloudParams {
    pub id: String,
    /// 云端暂存保留期档位（取自 [`SharePolicySummaryDto::retention_tiers`]）；缺省取云端默认档。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<String>,
}

/// `agent.share.resolve` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareResolveParams {
    /// 6 位取件码、`https://…/s/<slug>` 或 `fluxdown://share/<slug>`。
    pub code_or_link: String,
    /// 云端要求人机验证（`ShareTurnstileRequired`）后，UI 取得的 Turnstile 令牌。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turnstile_token: Option<String>,
}

/// 接收前可用来源。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSourcesDto {
    /// 分享方设备是否在线（直连 / 中继可取）；未知为 `None`。
    #[serde(default)]
    pub sender_online: Option<bool>,
    /// 有云端暂存副本（可经 CDN 取）。
    #[serde(default)]
    pub cloud_copy: bool,
    /// 可经实时桥接取（云端允许该身份走桥接）。
    #[serde(default)]
    pub bridge: bool,
    /// 当前在线、持有同一文件的其他设备数（共享）。
    #[serde(default)]
    pub swarm_peers: u32,
}

/// `agent.share.resolve` 结果：接收确认框的预览。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareResolveDto {
    /// agent 内存中的解析句柄（含票据）；短时有效，`agent.share.receive` 凭它接收而不再向云端 resolve。
    pub resolve_id: String,
    pub share_id: String,
    pub title: String,
    pub sender_device_name: String,
    pub files: Vec<ShareFileDto>,
    pub total_bytes: u64,
    /// 分享过期时间（RFC 3339）。
    pub expires_at: String,
    #[serde(default)]
    pub sources: ShareSourcesDto,
}

/// `agent.share.receive` 参数：`resolveId` 与 `codeOrLink` 二选一（都缺或都给 → `InvalidArgument`）；
/// 只给 `codeOrLink` 时 agent 先隐式 resolve。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareReceiveParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_or_link: Option<String>,
    /// 缺省用 daemon 默认保存目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_dir: Option<String>,
    /// 只接收这些文件（`files` 下标）；缺省接收全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_indexes: Option<Vec<u32>>,
}

/// `agent.share.dismissPending` 参数：取消 `pendingReceive` 对应的待确认接收。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareDismissPendingParams {
    pub resolve_id: String,
}

/// `agent.share.receive` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareReceiveResultDto {
    pub record_id: String,
    /// 新建的 daemon 任务 ID（与 `fileIndexes` 或全部文件一一对应）。
    pub task_ids: Vec<String>,
}

/// `agent.share.receiveRecords` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareReceiveRecordsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// 上一页的 `nextCursor`；缺省从最新开始。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `agent.share.receiveRecords` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareReceiveRecordsPage {
    pub records: Vec<ShareReceiveRecordDto>,
    /// `None` = 没有更早的记录。
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// `agent.share.switchToBridge` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSwitchToBridgeParams {
    pub task_id: String,
}

/// `agent.share.diagnostics` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareDiagnosticsParams {
    /// 只返回该会话的详情（任务「传输诊断」页用）；缺省返回全部进行中与最近结束的会话。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// `agent.share.logs` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareLogsParams {
    /// 只返回不低于该级别的行；缺省 `debug`（缓冲里有什么就给什么）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<ShareLogLevel>,
    /// 对 `target + message` 的正则过滤；非法正则 → `InvalidArgument`（`field = "regex"`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// 最多返回的行数（取最新的，按时间升序）；缺省由 agent 决定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// 内存环形缓冲里的一行 iroh tracing 日志。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareLogLineDto {
    /// RFC 3339，毫秒精度。
    pub at: String,
    pub level: ShareLogLevel,
    pub target: String,
    pub message: String,
}

/// `agent.share.logs` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareLogsDto {
    pub lines: Vec<ShareLogLineDto>,
}

/// `agent.share.sessionKill` 参数：分享方强制中断一个接收方会话。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSessionKillParams {
    pub session_id: String,
}

// ───────────────────────── 诊断 ─────────────────────────

/// net report 摘要（镜像 `fluxdown_share::NetReportInfo`）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareNetReportDto {
    #[serde(default)]
    pub udp_v4: bool,
    #[serde(default)]
    pub udp_v6: bool,
    #[serde(default)]
    pub mapping_varies_by_dest: Option<bool>,
    #[serde(default)]
    pub nat_type: ShareNatType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_relay: Option<String>,
    #[serde(default)]
    pub relay_latency_ms: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_v4: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_v6: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captive_portal: Option<bool>,
}

/// 端点网络快照：展示与上报 FluxCloud 的同一结构（镜像 `fluxdown_share::NetSnapshot`）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareNetSnapshotDto {
    pub captured_at: String,
    pub endpoint_id: String,
    pub client_version: String,
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_report: Option<ShareNetReportDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_relay: Option<ShareHomeRelayDto>,
    #[serde(default)]
    pub addrs: Vec<String>,
    /// 全部 iroh 指标：键 `<group>.<name>`，直方图展开为 `.count` / `.sum`。
    #[serde(default)]
    pub metrics: BTreeMap<String, Value>,
}

/// 会话字节归属。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSessionBytesDto {
    pub direct: u64,
    pub relay: u64,
    /// 经云端暂存副本（CDN / 对象存储）取得并通过校验的字节。
    #[serde(default)]
    pub cdn: u64,
    /// 经 share-gateway 桥接取得并通过校验的字节。
    #[serde(default)]
    pub bridge: u64,
    /// 从共享对端取回并通过校验的字节。
    #[serde(default)]
    pub peer: u64,
}

/// 单条路径的统计（镜像 `fluxdown_share::PathSummary`）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SharePathSummaryDto {
    pub id: String,
    pub kind: SharePathKind,
    /// 对端传输地址（`IP:端口` 或中继 URL）。
    pub remote: String,
    pub rtt_ms: f64,
    pub cwnd: u64,
    pub mtu: u16,
    pub sent: u64,
    pub recv: u64,
    pub lost: u64,
    pub congestion_events: u64,
    /// 拥塞控制器的带宽估计（bit/s），无估计为 0。
    pub bw_estimate: u64,
}

/// 路径事件时间线条目。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SharePathEventDto {
    /// RFC 3339，毫秒精度。
    pub at: String,
    /// `opened` / `selected` / `closed` / `lagged`。
    pub kind: String,
    pub path: String,
}

/// 会话摘要（镜像 `fluxdown_share::SessionSummary`）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSessionSummaryDto {
    pub started_at: String,
    pub connect_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_direct_ms: Option<u64>,
    pub path_kind: SharePathKind,
    pub bytes: ShareSessionBytesDto,
    pub paths: Vec<SharePathSummaryDto>,
    pub events: Vec<SharePathEventDto>,
    pub verify_failures: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 传输中的时序采样（镜像 `fluxdown_share::SessionSample`）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSessionSampleDto {
    pub at: String,
    pub bytes: u64,
    pub rate_bps: f64,
    pub rtt_ms: f64,
    pub path_kind: SharePathKind,
}

/// 共享提供方会话里单个内容的累计服务量（镜像 `fluxdown_share::SwarmServedFile`）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSwarmServedFileDto {
    /// 文件根哈希（64 位小写 hex）。
    pub root_hash: String,
    /// 本机已知该内容的文件名（来自本机分享 / 接收记录）；未知为 `None`，UI 回退显示短哈希。
    #[serde(default)]
    pub file_name: Option<String>,
    pub bytes: u64,
    pub chunks: u64,
}

/// 一个会话的完整诊断详情。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareSessionDetailDto {
    pub session_id: String,
    pub share_id: String,
    pub role: ShareSessionRole,
    pub peer_endpoint_id: String,
    /// 对端设备名（已知时）。
    #[serde(default)]
    pub peer_device_name: Option<String>,
    #[serde(default)]
    pub peer_nat_type: ShareNatType,
    #[serde(default)]
    pub peer_isp: Option<String>,
    /// 会话已结束（保留在最近列表里供回看）。
    #[serde(default)]
    pub ended: bool,
    pub summary: ShareSessionSummaryDto,
    /// 最近的时序采样（内存环形缓冲，旧的在前）。
    #[serde(default)]
    pub samples: Vec<ShareSessionSampleDto>,
    /// 共享提供方（`role == swarm`）：本连接按内容累计已发送的字节与块数；其他角色为空。
    #[serde(default)]
    pub swarm_files: Vec<ShareSwarmServedFileDto>,
}

/// `agent.share.diagnostics` 结果。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShareDiagnosticsDto {
    /// 当前网络快照（含全量 iroh 指标 `metrics`）；端点未绑定为 `None`。
    #[serde(default)]
    pub snapshot: Option<ShareNetSnapshotDto>,
    /// 与上报 FluxCloud 完全相同的快照 JSON 文本（缩进格式，供「复制 / 查看原始快照」）；端点未绑定为空串。
    #[serde(default)]
    pub snapshot_json: String,
    #[serde(default)]
    pub sessions: Vec<ShareSessionDetailDto>,
}
