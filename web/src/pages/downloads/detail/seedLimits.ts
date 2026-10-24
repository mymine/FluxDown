// 任务级做种限制表单（对齐 qBittorrent 5 单种子分享限制）：每项 = 模式（跟随全局 / 不限制 / 自定义）+ 数值。
// 线上哨兵：-2 跟随全局、-1 不限制（0 同义）、>0 自定义；分享率为千分比（1500 = 1.5）。

import { SEED_LIMIT_INHERIT, SEED_LIMIT_UNLIMITED } from '../../../lib/rpc'

export type SeedMode = 'global' | 'unlimited' | 'custom'
export type SeedLimitKind = 'ratio' | 'minutes'

export interface SeedLimitField {
  mode: SeedMode
  /** 数值输入框文本；非自定义模式显示 qBit 默认值。 */
  text: string
}

/** 非自定义模式下数值框显示的 qBit 默认值。 */
export const SEED_DEFAULT_TEXT: Record<SeedLimitKind, string> = { ratio: '1.00', minutes: '1440' }

/** 千分比 → 两位小数文本（1500 → "1.50"）。 */
export function formatRatio(milli: number): string {
  return (milli / 1000).toFixed(2)
}

/** 小数文本 → 千分比（四舍五入）；非数字 / 非正数 = null。 */
export function parseRatio(text: string): number | null {
  const trimmed = text.trim()
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return null
  const milli = Math.round(Number.parseFloat(trimmed) * 1000)
  return Number.isFinite(milli) && milli > 0 ? milli : null
}

/** 分钟文本 → 正整数；非法 / 非正 = null。 */
export function parseMinutes(text: string): number | null {
  const trimmed = text.trim()
  if (!/^\d+$/.test(trimmed)) return null
  const minutes = Number.parseInt(trimmed, 10)
  return Number.isSafeInteger(minutes) && minutes > 0 ? minutes : null
}

/** 自定义值文本是否合法（非自定义模式恒合法）。 */
export function isFieldValid(kind: SeedLimitKind, field: SeedLimitField): boolean {
  if (field.mode !== 'custom') return true
  return (kind === 'ratio' ? parseRatio(field.text) : parseMinutes(field.text)) !== null
}

/** 任务现值 → 表单状态：缺省 / -2 = 跟随全局；-1 / 0 / 其它非正 = 不限制；>0 = 自定义（回显现值）。 */
export function loadSeedLimit(kind: SeedLimitKind, raw: number | undefined): SeedLimitField {
  if (raw === undefined || raw === SEED_LIMIT_INHERIT) return { mode: 'global', text: SEED_DEFAULT_TEXT[kind] }
  if (raw <= 0) return { mode: 'unlimited', text: SEED_DEFAULT_TEXT[kind] }
  return { mode: 'custom', text: kind === 'ratio' ? formatRatio(raw) : String(raw) }
}

/**
 * 表单状态 → 要发送的线上值；自定义文本非法 = null（调用方禁止保存）。
 * 与加载时的初值完全一致（模式 + 文本都没动）时原样回传基线，避免 1.234 → 1.23 之类的取整漂移。
 */
export function seedLimitToSend(
  kind: SeedLimitKind,
  field: SeedLimitField,
  initial: SeedLimitField,
  baseline: number | undefined,
): number | null {
  if (field.mode === initial.mode && field.text === initial.text) return baseline ?? SEED_LIMIT_INHERIT
  if (field.mode === 'global') return SEED_LIMIT_INHERIT
  if (field.mode === 'unlimited') return SEED_LIMIT_UNLIMITED
  return kind === 'ratio' ? parseRatio(field.text) : parseMinutes(field.text)
}
