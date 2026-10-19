// 节点 prop 读取（daemon 已按 schema 归一化；这里只做类型收窄与缺省值）。

import type { JsonValue } from '../../../../lib/rpc'

export type Props = Record<string, JsonValue>

export function str(p: Props, key: string, fallback = ''): string {
  const value = p[key]
  return typeof value === 'string' ? value : fallback
}

export function optStr(p: Props, key: string): string | undefined {
  const value = p[key]
  return typeof value === 'string' ? value : undefined
}

export function bool(p: Props, key: string, fallback = false): boolean {
  const value = p[key]
  return typeof value === 'boolean' ? value : fallback
}

export function num(p: Props, key: string): number | undefined {
  const value = p[key]
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

export function record(value: JsonValue | undefined): Record<string, JsonValue> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value : undefined
}

export function records(p: Props, key: string): Record<string, JsonValue>[] {
  const value = p[key]
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    const rec = record(item)
    return rec ? [rec] : []
  })
}

/** 只放行 http(s) 链接（daemon 已校验，这里再防一道）。 */
export function safeHref(href: string): string | null {
  try {
    const url = new URL(href)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : null
  } catch {
    return null
  }
}
