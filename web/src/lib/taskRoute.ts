// 任务链路标签 → 文案（Auto 路由或站点规则标签）；未知值原样返回。

import type { TFunction } from '../i18n'

const RULE_KEYS: Readonly<Record<string, string>> = {
  'rule:direct': 'taskRouteRuleDirect',
  'rule:system': 'taskRouteRuleSystem',
  'rule:manual': 'taskRouteRuleManual',
  'rule:custom': 'taskRouteRuleCustom',
}

const BASE_KEYS: Readonly<Record<string, string>> = {
  direct: 'taskRouteDirect',
  'direct:sampled': 'taskRouteDirectSampled',
  'direct:pinned': 'taskRouteDirectPinned',
  'direct:failover': 'taskRouteDirectFailover',
  'proxy:cached': 'taskRouteProxyCached',
  'proxy:sampled': 'taskRouteProxySampled',
  'proxy:failover': 'taskRouteProxyFailover',
}

export function routeLabel(t: TFunction, route: string): string {
  // 规则标签必须先整串精确匹配，否则 `rule:system` 会被 `:system` 后缀误拆。
  const ruleKey = RULE_KEYS[route]
  if (ruleKey) return t(ruleKey)
  let base = route
  let via: string | null = null
  if (route.endsWith(':system')) {
    base = route.slice(0, -':system'.length)
    via = t('taskRouteViaSystem')
  } else if (route.endsWith(':manual')) {
    base = route.slice(0, -':manual'.length)
    via = t('taskRouteViaManual')
  }
  const key = BASE_KEYS[base]
  const label = key ? t(key) : route
  return via === null ? label : `${label} · ${via}`
}
