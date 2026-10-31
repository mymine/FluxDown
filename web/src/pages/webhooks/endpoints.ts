// Webhook 端点模型与配置读写：daemon 配置键 `webhook.endpoints`（JSON 数组字符串），
// 与 `engine::webhook::EndpointSpec` 同 wire 形状（camelCase）。

import type { TFunction } from '../../i18n'
import { createConfigListPatcher } from '../../lib/configList'
import { errorMessage } from '../../lib/rpc'
import type { WebhookDeliveryDto, WebhookTestResponse } from '../../lib/rpc'

export const ENDPOINTS_KEY = 'webhook.endpoints'

export interface EndpointSpec {
  id: string
  name: string
  preset: string
  url: string
  enabled: boolean
  events: string[]
  queueId: string
  headers: Record<string, string>
  bodyTemplate: string
  signSecret: string
  allowHttp: boolean
  useProxy: boolean
}

/** 事件 wire 名（与引擎 `WebhookEventKind::wire()` 逐字一致）及其 i18n 键。 */
export const WEBHOOK_EVENTS = [
  { wire: 'task.created', labelKey: 'webhookEventCreated' },
  { wire: 'task.started', labelKey: 'webhookEventStarted' },
  { wire: 'task.completed', labelKey: 'webhookEventCompleted' },
  { wire: 'task.failed', labelKey: 'webhookEventFailed' },
  { wire: 'task.paused', labelKey: 'webhookEventPaused' },
  { wire: 'queue.started', labelKey: 'webhookEventQueueStarted' },
  { wire: 'queue.drained', labelKey: 'webhookEventQueueDrained' },
] as const

/** 仅本机 webhook 可订阅、FluxCloud 不认识的事件（agent 上报前也会丢弃）。 */
const LOCAL_ONLY_EVENTS: readonly string[] = ['queue.started']

/** 云端推送渠道可订阅的事件：`WEBHOOK_EVENTS` 去掉本机专属事件，保持原顺序。 */
export const CLOUD_EVENTS = WEBHOOK_EVENTS.filter((event) => !LOCAL_ONLY_EVENTS.includes(event.wire))

const str = (value: unknown): string => (typeof value === 'string' ? value : '')
const bool = (value: unknown, fallback: boolean): boolean => (typeof value === 'boolean' ? value : fallback)

function parseEndpoint(raw: unknown): EndpointSpec | null {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null
  const item = raw as Record<string, unknown>
  const headers: Record<string, string> = {}
  if (typeof item.headers === 'object' && item.headers !== null && !Array.isArray(item.headers)) {
    for (const [key, value] of Object.entries(item.headers)) {
      if (typeof value === 'string') headers[key] = value
    }
  }
  return {
    id: str(item.id),
    name: str(item.name),
    preset: str(item.preset),
    url: str(item.url),
    // serde `default_true`：缺省启用。
    enabled: bool(item.enabled, true),
    events: Array.isArray(item.events) ? item.events.filter((event): event is string => typeof event === 'string') : [],
    queueId: str(item.queueId),
    headers,
    bodyTemplate: str(item.bodyTemplate),
    signSecret: str(item.signSecret),
    allowHttp: bool(item.allowHttp, false),
    useProxy: bool(item.useProxy, false),
  }
}

/** 配置串 → 端点列表；空串 / 非 JSON 数组视为空列表，非对象元素跳过，字段类型不符回退默认值（与 GPUI `parse_endpoints`、引擎 `reload_endpoints` 一致）。 */
export function parseEndpoints(raw: string | undefined): EndpointSpec[] {
  if (!raw || raw.trim() === '') return []
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed.map(parseEndpoint).filter((entry): entry is EndpointSpec => entry !== null)
  } catch {
    return []
  }
}

/** 读-改-写端点列表（串行化 + 冲突重试见 `createConfigListPatcher`）。 */
export const patchEndpoints = createConfigListPatcher(ENDPOINTS_KEY, parseEndpoints, (list) => JSON.stringify(list))

/** 新增或按 id 覆盖。 */
export const upsertEndpoint = (draft: EndpointSpec) =>
  patchEndpoints((list) => {
    const index = list.findIndex((entry) => entry.id === draft.id)
    if (index < 0) return [...list, draft]
    return list.map((entry, i) => (i === index ? draft : entry))
  })

export const setEndpointEnabled = (id: string, enabled: boolean) =>
  patchEndpoints((list) => {
    if (!list.some((entry) => entry.id === id)) return null
    return list.map((entry) => (entry.id === id ? { ...entry, enabled } : entry))
  })

export const removeEndpoint = (id: string) => patchEndpoints((list) => list.filter((entry) => entry.id !== id))

/** 端点的最近一次投递（按时间戳最大）。 */
export function latestDelivery(deliveries: readonly WebhookDeliveryDto[], endpointId: string): WebhookDeliveryDto | undefined {
  let latest: WebhookDeliveryDto | undefined
  for (const delivery of deliveries) {
    if (delivery.endpointId !== endpointId) continue
    if (!latest || delivery.timestampMs > latest.timestampMs) latest = delivery
  }
  return latest
}

/** 「发送测试」的展示结果；行内测试归属发起的端点（`endpointId`）。 */
export interface TestReport {
  success: boolean
  text: string
}

/** 测试回执 → 展示文案（与 GPUI `test_result_text` 同规则；行内与对话框共用）。 */
export function testReport(t: TFunction, outcome: { response: WebhookTestResponse } | { error: unknown }): TestReport {
  if ('error' in outcome) return { success: false, text: t('webhookTestFail', { error: errorMessage(outcome.error) }) }
  const { response } = outcome
  if (response.success) {
    return { success: true, text: t('webhookTestOk', { status: response.statusCode === 0 ? 'OK' : response.statusCode, ms: response.latencyMs }) }
  }
  return { success: false, text: t('webhookTestFail', { error: response.error === '' ? `HTTP ${response.statusCode}` : response.error }) }
}

export const NO_DELIVERIES: readonly WebhookDeliveryDto[] = []
export const selectDeliveries = (daemon: { webhookDeliveries: WebhookDeliveryDto[] }) => daemon.webhookDeliveries
