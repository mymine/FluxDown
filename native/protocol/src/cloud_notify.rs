//! 云端推送通知（FluxCloud 代发）的 wire 契约。
//!
//! 数据流：daemon 在 webhook 同一组语义触发点发出 [`TaskNoticeDto`]（`DaemonEvent::TaskNotice`），
//! 只由 agent 消费、不转发给 UI；agent 按本机上报开关与隐私字段裁剪后批量交给
//! FluxCloud `/api/v1/notifications/events`，渠道配置、额度与投递全部在云端。
//! UI 经 `agent.cloudNotify.*` 读写，状态投影为 `AgentSnapshot.cloud_notify`。

use serde::{Deserialize, Serialize};

/// 一条任务生命周期语义事件（与引擎 webhook 事件同源同语义）。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TaskNoticeDto {
    /// 事件唯一 ID（UUID v4），云端按 `(user, deliveryId)` 幂等去重。
    pub delivery_id: String,
    /// webhook wire 名：`task.created` / `task.started` / `task.completed` /
    /// `task.failed` / `task.paused` / `queue.started` / `queue.drained`。
    pub event: String,
    pub timestamp_ms: i64,
    pub queue_id: String,
    pub queue_name: String,
    /// `queue.started` / `queue.drained` 为 `None`，其余事件必有。
    #[serde(default)]
    pub task: Option<TaskNoticeTaskDto>,
}

/// [`TaskNoticeDto`] 里的任务快照（字段语义同 `engine::webhook::WebhookTask`）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TaskNoticeTaskDto {
    pub id: String,
    pub file_name: String,
    pub url: String,
    pub save_dir: String,
    pub total_bytes: i64,
    pub status: i32,
    pub error_message: String,
}

/// `CloudNotifyStateDto.recent_deliveries` 的条数上限（= 云端第一页大小）。
pub const CLOUD_NOTIFY_RECENT_DELIVERIES: usize = 50;

/// 云端推送在本机的完整状态投影（`AgentSnapshot.cloud_notify`）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyStateDto {
    /// 本设备上报开关（设备本地，默认关；开启需用户在 UI 显式同意）。
    pub reporting: bool,
    /// 上报时附带下载地址（默认关）。
    pub include_url: bool,
    /// 上报时附带保存目录（默认关）。
    pub include_save_dir: bool,
    /// 服务端开放的渠道种类目录（公开接口 `GET /api/v1/notifications/catalog`，不需要登录）。
    /// `None` = 尚未拉取成功（未知）；`Some(空)` = 管理员关闭了全部云端渠道，客户端只显示自托管 Webhook。
    /// 只含管理员启用的种类，顺序固定：email、telegram。
    #[serde(default)]
    pub catalog: Option<Vec<CloudNotifyKindDto>>,
    /// 最近一次成功拉取的云端概览；未登录或从未拉取成功为 `None`。
    #[serde(default)]
    pub overview: Option<CloudNotifyOverviewDto>,
    /// 正在拉取概览。
    #[serde(default)]
    pub loading: bool,
    /// 最近一次拉取失败的原因（成功后清空）。
    #[serde(default)]
    pub last_error_reason: Option<crate::ErrorReason>,
    /// 最近一次成功拉取概览的时间。
    #[serde(default)]
    pub updated_at_unix_ms: Option<i64>,
    /// 最近的云端投递记录（新的在前，至多 [`CLOUD_NOTIFY_RECENT_DELIVERIES`] 条）：agent 在登录 / 刷新 /
    /// SSE 重连时拉第一页，并按云端 SSE `notify.delivery` 增量按 `id` 合并，UI 直接读它即可实时更新；
    /// 「加载更多」再用 `agent.cloudNotify.deliveries` 从 [`Self::recent_next_cursor`] 往后翻。
    #[serde(default)]
    pub recent_deliveries: Vec<CloudNotifyDeliveryDto>,
    /// 第一页之后的翻页游标；`None` = 没有更早的记录。
    #[serde(default)]
    pub recent_next_cursor: Option<String>,
}

/// FluxCloud `GET /api/v1/notifications/overview` 的投影。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyOverviewDto {
    /// 当前套餐（含用户级覆盖）是否允许云端推送。
    pub enabled: bool,
    pub usage: CloudNotifyUsageDto,
    /// 渠道数上限；`0` = 不限。
    pub max_channels: u32,
    pub channels: Vec<CloudNotifyChannelDto>,
    /// 账号邮箱（邮件渠道的收件地址）。
    pub account_email: String,
}

/// 推送额度用量。`*_limit == 0` 表示不限。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyUsageDto {
    pub daily_used: u32,
    pub daily_limit: u32,
    pub monthly_used: u32,
    pub monthly_limit: u32,
    /// 下次日额度重置时间（RFC 3339 UTC）。
    pub daily_reset_at: String,
    /// 下次月额度重置时间（RFC 3339 UTC）。
    pub monthly_reset_at: String,
}

/// 一个渠道种类及其当前可用性（如服务端未配置 Telegram 机器人 / SMTP 则不可用）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyKindDto {
    pub kind: String,
    pub available: bool,
}

/// 用户的一个云端推送渠道。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyChannelDto {
    pub id: String,
    /// `email` / `telegram`（其余推送服务走本机自托管 Webhook，不经云端）。
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    /// 订阅的事件 wire 名。
    pub events: Vec<String>,
    /// 展示用投递目标：邮件 = 第一个收件地址；Telegram = `@用户名` / 会话名。
    pub target: String,
    /// 邮件渠道的全部收件地址（账号邮箱在前；至多 [`CLOUD_NOTIFY_EMAIL_MAX_ADDRESSES`] 个）；其他种类为空。
    #[serde(default)]
    pub addresses: Vec<String>,
    /// `ok` / `failing`（连续 3 次投递失败）。
    pub status: String,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_delivered_at: Option<String>,
    pub created_at: String,
}

/// 一条云端投递记录。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyDeliveryDto {
    pub id: String,
    /// 事件 wire 名。
    pub event: String,
    /// 任务文件名；`queue.drained` 为队列名。
    pub title: String,
    pub channel_id: String,
    pub channel_kind: String,
    pub channel_name: String,
    pub device_name: String,
    /// `pending` / `sent` / `failed` / `dropped`（超出额度）。
    pub status: String,
    pub attempts: u32,
    pub max_attempts: u32,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub sent_at: Option<String>,
}

/// `agent.cloudNotify.setReporting` 参数。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyReportingParams {
    pub enabled: bool,
}

/// `agent.cloudNotify.setPrivacy` 参数。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyPrivacyParams {
    pub include_url: bool,
    pub include_save_dir: bool,
}

/// 单个邮件渠道的收件地址上限。
pub const CLOUD_NOTIFY_EMAIL_MAX_ADDRESSES: usize = 5;

/// `agent.cloudNotify.createChannel` 参数：只用于 `email`；Telegram 只能经绑定流程创建。
///
/// `addresses` 为空 = 只发账号邮箱。列表里除账号邮箱外的地址必须先经
/// `agent.cloudNotify.sendEmailCode` + `agent.cloudNotify.verifyEmail` 验证过。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyChannelCreateParams {
    pub kind: String,
    pub name: String,
    pub events: Vec<String>,
    #[serde(default)]
    pub addresses: Vec<String>,
}

/// `agent.cloudNotify.updateChannel` 参数：缺省字段保持不变；`addresses` 整体替换（规则同创建）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyChannelUpdateParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<String>>,
}

/// `agent.cloudNotify.sendEmailCode` 参数：向待添加的通知邮箱发验证码。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyEmailCodeParams {
    pub address: String,
}

/// `agent.cloudNotify.sendEmailCode` 结果。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyEmailCodeResult {
    /// 再次发送前需等待的秒数。
    pub resend_after_secs: u32,
    /// 验证码有效期（秒）。
    pub expires_in_secs: u32,
}

/// `agent.cloudNotify.verifyEmail` 参数：提交发到该地址的验证码，验证通过后该地址记为本账号已验证，
/// 之后可加入任意邮件渠道（再次使用不需要验证码）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyEmailVerifyParams {
    pub address: String,
    pub code: String,
}

/// `agent.cloudNotify.deleteChannel` / `testChannel` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyChannelIdParams {
    pub id: String,
}

/// `agent.cloudNotify.testChannel` 结果（测试不计入额度）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyTestResult {
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
    pub latency_ms: u64,
}

/// `agent.cloudNotify.telegramBindStart` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyTelegramBindDto {
    pub code: String,
    /// `https://t.me/<bot>?start=<code>`，UI 同时渲染为二维码。
    pub deep_link: String,
    pub expires_at: String,
}

/// `agent.cloudNotify.telegramBindStatus` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyTelegramBindStatusParams {
    pub code: String,
}

/// `agent.cloudNotify.telegramBindStatus` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyTelegramBindStatusDto {
    /// `pending` / `bound` / `expired`。
    pub status: String,
    #[serde(default)]
    pub channel: Option<CloudNotifyChannelDto>,
}

/// `agent.cloudNotify.deliveries` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyDeliveriesParams {
    /// 缺省 50，上限 100。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// 上一页返回的 `nextCursor`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// `agent.cloudNotify.deliveries` 结果（新的在前）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudNotifyDeliveriesPage {
    pub items: Vec<CloudNotifyDeliveryDto>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}
