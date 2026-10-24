// 云端推送通知 DTO（native/protocol/src/cloud_notify.rs）。

import type { ErrorReason } from './error';

/** 一条任务生命周期语义事件（`DaemonEvent::TaskNotice`，agent 消费、不转发给 UI）。 */
export interface TaskNoticeDto {
  /** 事件唯一 ID（UUID v4），云端按 `(user, deliveryId)` 幂等去重。 */
  deliveryId: string;
  /** `task.created` / `task.started` / `task.completed` / `task.failed` / `task.paused` / `queue.started` / `queue.drained`。 */
  event: string;
  timestampMs: number;
  queueId: string;
  queueName: string;
  /** `queue.started` / `queue.drained` 为 null/缺省，其余事件必有。 */
  task?: TaskNoticeTaskDto | null;
}

export interface TaskNoticeTaskDto {
  id: string;
  fileName: string;
  url: string;
  saveDir: string;
  totalBytes: number;
  status: number;
  errorMessage: string;
}

/** `CloudNotifyStateDto.recentDeliveries` 的条数上限（= 云端第一页大小）。 */
export const CLOUD_NOTIFY_RECENT_DELIVERIES = 50;

/** 云端推送在本机的完整状态投影（`AgentSnapshot.cloudNotify`）。 */
export interface CloudNotifyStateDto {
  /** 本设备上报开关（设备本地，默认关）。 */
  reporting: boolean;
  includeUrl: boolean;
  includeSaveDir: boolean;
  /**
   * 服务端开放的渠道种类目录（公开接口，无需登录）。缺省 / null = 尚未拉取（未知）；
   * 空数组 = 管理员关闭了全部云端渠道。登出不清空。
   */
  catalog?: CloudNotifyKindDto[] | null;
  /** 最近一次成功拉取的云端概览；未登录或从未成功为 null。 */
  overview?: CloudNotifyOverviewDto | null;
  loading?: boolean;
  lastErrorReason?: ErrorReason | null;
  updatedAtUnixMs?: number | null;
  /** 最近的云端投递记录（新的在前，至多 `CLOUD_NOTIFY_RECENT_DELIVERIES` 条）；agent 按 SSE 增量合并。 */
  recentDeliveries?: CloudNotifyDeliveryDto[];
  /** 第一页之后的翻页游标；缺省 / null = 没有更早的记录。 */
  recentNextCursor?: string | null;
}

export interface CloudNotifyOverviewDto {
  /** 当前套餐（含用户级覆盖）是否允许云端推送。 */
  enabled: boolean;
  usage: CloudNotifyUsageDto;
  /** 渠道数上限；0 = 不限。 */
  maxChannels: number;
  channels: CloudNotifyChannelDto[];
  accountEmail: string;
}

/** `*Limit === 0` 表示不限。 */
export interface CloudNotifyUsageDto {
  dailyUsed: number;
  dailyLimit: number;
  monthlyUsed: number;
  monthlyLimit: number;
  /** RFC 3339 UTC。 */
  dailyResetAt: string;
  monthlyResetAt: string;
}

export interface CloudNotifyKindDto {
  kind: string;
  available: boolean;
}

export interface CloudNotifyChannelDto {
  id: string;
  /** `email` / `telegram`。 */
  kind: string;
  name: string;
  enabled: boolean;
  events: string[];
  /** 展示用投递目标：邮件 = 第一个收件地址；Telegram = `@用户名` / 会话名。 */
  target: string;
  /** 邮件渠道的全部收件地址（账号邮箱在前）；其他种类为空。 */
  addresses?: string[];
  /** `ok` / `failing`。 */
  status: string;
  lastError?: string | null;
  lastDeliveredAt?: string | null;
  createdAt: string;
}

export interface CloudNotifyDeliveryDto {
  id: string;
  event: string;
  /** 任务文件名；`queue.drained` 为队列名。 */
  title: string;
  channelId: string;
  channelKind: string;
  channelName: string;
  deviceName: string;
  /** `pending` / `sent` / `failed` / `dropped`（超出额度）。 */
  status: string;
  attempts: number;
  maxAttempts: number;
  error?: string | null;
  createdAt: string;
  sentAt?: string | null;
}

export interface CloudNotifyReportingParams {
  enabled: boolean;
}

export interface CloudNotifyPrivacyParams {
  includeUrl: boolean;
  includeSaveDir: boolean;
}

/** 单个邮件渠道的收件地址上限。 */
export const CLOUD_NOTIFY_EMAIL_MAX_ADDRESSES = 5;

/**
 * 云端只支持 email；Telegram 只能经绑定流程创建。`addresses` 缺省 / 空 = 只发账号邮箱；
 * 其余地址必须先经 `sendEmailCode` + `verifyEmail` 验证。
 */
export interface CloudNotifyChannelCreateParams {
  kind: string;
  name: string;
  events: string[];
  addresses?: string[];
}

/** 缺省字段保持不变；`addresses` 整体替换（规则同创建）。 */
export interface CloudNotifyChannelUpdateParams {
  id: string;
  name?: string;
  enabled?: boolean;
  events?: string[];
  addresses?: string[];
}

/** `agent.cloudNotify.sendEmailCode` 参数：向待添加的通知邮箱发验证码。 */
export interface CloudNotifyEmailCodeParams {
  address: string;
}

export interface CloudNotifyEmailCodeResult {
  /** 再次发送前需等待的秒数。 */
  resendAfterSecs: number;
  /** 验证码有效期（秒）。 */
  expiresInSecs: number;
}

/** `agent.cloudNotify.verifyEmail` 参数：验证通过后该地址记为本账号已验证，可加入任意邮件渠道。 */
export interface CloudNotifyEmailVerifyParams {
  address: string;
  code: string;
}

export interface CloudNotifyChannelIdParams {
  id: string;
}

export interface CloudNotifyTestResult {
  success: boolean;
  error?: string | null;
  latencyMs: number;
}

export interface CloudNotifyTelegramBindDto {
  code: string;
  /** `https://t.me/<bot>?start=<code>`。 */
  deepLink: string;
  expiresAt: string;
}

export interface CloudNotifyTelegramBindStatusParams {
  code: string;
}

export interface CloudNotifyTelegramBindStatusDto {
  /** `pending` / `bound` / `expired`。 */
  status: string;
  channel?: CloudNotifyChannelDto | null;
}

export interface CloudNotifyDeliveriesParams {
  /** 缺省 50，上限 100。 */
  limit?: number;
  before?: string;
}

export interface CloudNotifyDeliveriesPage {
  items: CloudNotifyDeliveryDto[];
  nextCursor?: string | null;
}
