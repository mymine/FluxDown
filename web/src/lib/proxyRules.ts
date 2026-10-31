// 站点代理规则：daemon 配置键 `proxy_rules`（JSON 数组字符串，数组顺序即优先级）。
// 解析行为与 engine / protocol / Android / iOS 的宽松解析逐条一致；匹配只在 engine 里做，
// 这里不做任何本地匹配，只通过 `daemon.config.proxyRoute` 询问。

import type { TFunction } from '../i18n'
import { createConfigListPatcher } from './configList'
import type { ProxyRouteResponse, ProxyRuleIssue } from './rpc'

export const PROXY_RULES_KEY = 'proxy_rules'

export type RuleMatch = 'domain' | 'regex'
export type RuleAction = 'direct' | 'system' | 'manual' | 'custom'

export const RULE_ACTIONS: readonly RuleAction[] = ['direct', 'system', 'manual', 'custom']

export interface ProxyRule {
  id: string
  enabled: boolean
  name: string
  match: RuleMatch
  pattern: string
  action: RuleAction
  proxy: string
}

const str = (value: unknown): string => (typeof value === 'string' ? value : '')

/** 非对象元素返回 null；缺失 / 类型错误 / 未知取值回落到模型表的默认值。 */
function parseRule(raw: unknown): ProxyRule | null {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null
  const item = raw as Record<string, unknown>
  return {
    id: str(item.id),
    enabled: typeof item.enabled === 'boolean' ? item.enabled : true,
    name: str(item.name),
    match: item.match === 'regex' ? 'regex' : 'domain',
    pattern: str(item.pattern),
    action: item.action === 'system' || item.action === 'manual' || item.action === 'custom' ? item.action : 'direct',
    proxy: str(item.proxy),
  }
}

/** 空串 / 非法 JSON / 非数组 → 空列表；非对象元素跳过。 */
export function parseProxyRules(raw: string | undefined): ProxyRule[] {
  if (!raw || raw.trim() === '') return []
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed.map(parseRule).filter((rule): rule is ProxyRule => rule !== null)
  } catch {
    return []
  }
}

/** 总是写出全部 7 个字段。 */
export function encodeProxyRules(list: readonly ProxyRule[]): string {
  return JSON.stringify(
    list.map((rule) => ({
      id: rule.id,
      enabled: rule.enabled,
      name: rule.name,
      match: rule.match,
      pattern: rule.pattern,
      action: rule.action,
      proxy: rule.proxy,
    })),
  )
}

export function newProxyRule(id: string): ProxyRule {
  return { id, enabled: true, name: '', match: 'domain', pattern: '', action: 'direct', proxy: '' }
}

/** 启用的规则条数（供状态栏 / 模式提示）。 */
export function enabledRuleCount(raw: string | undefined): number {
  return parseProxyRules(raw).filter((rule) => rule.enabled).length
}

/** 展示用代理端点：去掉凭据与末尾 `/`。 */
export function displayEndpoint(url: string): string {
  const trimmed = url.trim()
  const sep = trimmed.indexOf('://')
  const scheme = sep >= 0 ? trimmed.slice(0, sep) : null
  const rest = sep >= 0 ? trimmed.slice(sep + 3) : trimmed
  const at = rest.lastIndexOf('@')
  const host = (at >= 0 ? rest.slice(at + 1) : rest).replace(/\/+$/, '')
  return scheme === null ? host : `${scheme}://${host}`
}

/** 全局手动代理的端点；host 为空或 port 不在 1..=65535 时返回 `''`（视为未配置）。 */
export function manualEndpoint(type: string, host: string, port: string): string {
  const h = host.trim()
  const p = port.trim()
  const n = /^\d+$/.test(p) ? Number.parseInt(p, 10) : 0
  if (h === '' || n < 1 || n > 65535) return ''
  return `${type}://${h.includes(':') && !h.startsWith('[') ? `[${h}]` : h}:${n}`
}

/** 按 id 覆盖，不存在则追加。 */
export function upserted(list: readonly ProxyRule[], draft: ProxyRule): ProxyRule[] {
  const index = list.findIndex((rule) => rule.id === draft.id)
  if (index < 0) return [...list, draft]
  return list.map((rule, i) => (i === index ? draft : rule))
}

/** 上移 / 下移一位；id 缺失或越界返回 null（无需写入）。 */
export function moved(list: readonly ProxyRule[], id: string, delta: -1 | 1): ProxyRule[] | null {
  const from = list.findIndex((rule) => rule.id === id)
  const to = from + delta
  if (from < 0 || to < 0 || to >= list.length) return null
  const next = [...list]
  const [item] = next.splice(from, 1)
  if (!item) return null
  next.splice(to, 0, item)
  return next
}

export const patchProxyRules = createConfigListPatcher(PROXY_RULES_KEY, parseProxyRules, encodeProxyRules)

export const upsertRule = (draft: ProxyRule) => patchProxyRules((list) => upserted(list, draft))

export const setRuleEnabled = (id: string, enabled: boolean) =>
  patchProxyRules((list) => {
    if (!list.some((rule) => rule.id === id)) return null
    return list.map((rule) => (rule.id === id ? { ...rule, enabled } : rule))
  })

export const removeRule = (id: string) => patchProxyRules((list) => list.filter((rule) => rule.id !== id))

export const moveRule = (id: string, delta: -1 | 1) => patchProxyRules((list) => moved(list, id, delta))

export interface RouteLines {
  title?: { text: string; tone: 'accent' | 'normal' }
  detail: { text: string; tone: 'warning' | 'normal' }
}

const DETAIL_KEYS: Readonly<Record<string, string>> = {
  direct: 'proxyRouteDirect',
  systemUndetected: 'proxyRouteSystemUndetected',
  manualIncomplete: 'proxyRouteManualIncomplete',
  modeNone: 'proxyRouteModeNone',
  bypass: 'proxyRouteBypass',
  pac: 'proxyRouteModePac',
  auto: 'proxyRouteModeAuto',
  loopback: 'proxyRouteLoopback',
  unsupported: 'proxyRouteUnsupported',
  invalidUrl: 'proxyRouteInvalidUrl',
}

/** 「测试网址」结果两行；`reason === ''`（未给出 url）返回 null。 */
export function routeLines(t: TFunction, response: ProxyRouteResponse): RouteLines | null {
  const { reason } = response
  if (reason === '') return null
  const detailText =
    reason === 'proxy'
      ? t('proxyRouteProxy', { endpoint: response.endpoint })
      : t(DETAIL_KEYS[reason] ?? 'proxyRouteDirect')
  const detail: RouteLines['detail'] = {
    text: detailText,
    tone: reason === 'systemUndetected' || reason === 'manualIncomplete' || reason === 'invalidUrl' ? 'warning' : 'normal',
  }
  if (reason === 'loopback' || reason === 'unsupported' || reason === 'invalidUrl') return { detail }
  const matched = response.matched
  if (!matched) return { title: { text: t('proxyRouteNoRule'), tone: 'normal' }, detail }
  const index = matched.index + 1
  const text =
    matched.name === '' ? t('proxyRouteRuleHit', { index }) : t('proxyRouteRuleHitNamed', { index, name: matched.name })
  return { title: { text, tone: 'accent' }, detail }
}

/** 校验 issue → 文案。 */
export function issueText(t: TFunction, issue: ProxyRuleIssue): string {
  switch (issue.code) {
    case 'emptyPattern':
      return t('proxyRuleErrEmpty')
    case 'invalidDomain':
      return t('proxyRuleErrDomain', { detail: issue.detail })
    case 'invalidRegex':
      return t('proxyRuleErrRegex', { detail: issue.detail })
    case 'regexTooLarge':
      return t('proxyRuleErrRegex', { detail: '1 MiB' })
    case 'tooLong':
      return t('proxyRuleErrTooLong', { max: issue.detail })
    case 'invalidProxy':
      return t('proxyRuleErrProxy', { detail: issue.detail })
    default:
      return t('proxyRuleErrGeneric', { detail: issue.code })
  }
}
