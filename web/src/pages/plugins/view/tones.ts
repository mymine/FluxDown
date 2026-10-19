// 色调 → 主题 token（与 `native/protocol/src/plugin_view_tones.json` 一致，测试断言同源）。
// 渲染时落到 `--fx-colors-*` CSS 变量，随主题切换自动同步。

import { varName } from '../../../theme/tokens'

export type Tone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger' | 'info' | 'muted'

export const TONE_TOKENS: Readonly<Record<Tone, string>> = {
  neutral: 'colors.foreground',
  accent: 'colors.primary',
  success: 'colors.success',
  warning: 'colors.warning',
  danger: 'colors.destructive',
  info: 'colors.info',
  muted: 'colors.mutedForeground',
}

export function isTone(value: unknown): value is Tone {
  return typeof value === 'string' && Object.hasOwn(TONE_TOKENS, value)
}

/** 色调对应的 CSS 颜色值（未知色调按 neutral）。 */
export function toneColor(tone: unknown): string {
  return `var(${varName(TONE_TOKENS[isTone(tone) ? tone : 'neutral'])})`
}

/** 徽标 / 提示底色：色调的 15% 透明度。 */
export function toneBackground(tone: unknown): string {
  return `color-mix(in srgb, ${toneColor(tone)} 15%, transparent)`
}
