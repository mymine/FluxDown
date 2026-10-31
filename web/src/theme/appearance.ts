// 外观偏好：镜像 `crates/theme/src/appearance.rs`（键名、默认值、解析规则）。
// 偏好来自 agent 的 `AgentPreferencesDto.values`，与 GPUI 客户端共用同一组键。

export const THEME_MODE_KEY = 'appearance.theme_mode'
export const DARK_THEME_KEY = 'appearance.dark_theme'
export const LIGHT_THEME_KEY = 'appearance.light_theme'
export const COLOR_SCHEME_KEY = 'appearance.color_scheme'
export const CUSTOM_COLOR_KEY = 'appearance.custom_color'
export const UI_SCALE_KEY = 'ui_scale'

export type ThemePreference = 'system' | 'light' | 'dark'
export type ThemeModeName = 'light' | 'dark'
export type AccentScheme = 'blue' | 'green' | 'violet' | 'rose' | 'custom'

export type BuiltinThemeId = 'defaultDark' | 'defaultLight' | 'midnightBlue' | 'nord' | 'warmLight' | 'graphite' | 'graphiteLight'

export const BUILTIN_THEMES: readonly BuiltinThemeId[] = ['defaultDark', 'defaultLight', 'midnightBlue', 'nord', 'warmLight', 'graphite', 'graphiteLight']

const BUILTIN_APPEARANCE: Readonly<Record<BuiltinThemeId, ThemeModeName>> = {
  defaultDark: 'dark',
  defaultLight: 'light',
  midnightBlue: 'dark',
  nord: 'dark',
  warmLight: 'light',
  graphite: 'dark',
  graphiteLight: 'light',
}

/** 主题库 `extends` 取值（registry.json 的 `extends`）。 */
const BUILTIN_EXTENDS: Readonly<Record<BuiltinThemeId, string>> = {
  defaultDark: 'builtin:default-dark',
  defaultLight: 'builtin:default-light',
  midnightBlue: 'builtin:midnight-blue',
  nord: 'builtin:nord',
  warmLight: 'builtin:warm-light',
  graphite: 'builtin:graphite',
  graphiteLight: 'builtin:graphite-light',
}

/** i18n 文案键（`themeDefaultDark` 等）。 */
export const BUILTIN_THEME_LABEL_KEYS: Readonly<Record<BuiltinThemeId, string>> = {
  defaultDark: 'themeDefaultDark',
  defaultLight: 'themeDefaultLight',
  midnightBlue: 'themeMidnightBlue',
  nord: 'themeNord',
  warmLight: 'themeWarmLight',
  graphite: 'themeGraphite',
  graphiteLight: 'themeGraphiteLight',
}

export const ACCENT_SCHEMES: readonly AccentScheme[] = ['blue', 'green', 'violet', 'rose', 'custom']

export const ACCENT_LABEL_KEYS: Readonly<Record<AccentScheme, string>> = {
  blue: 'colorBlue',
  green: 'colorGreen',
  violet: 'colorViolet',
  rose: 'colorRose',
  custom: 'colorCustom',
}

const ACCENT_PRESET_RGB: Readonly<Record<AccentScheme, number>> = {
  blue: 0x3b82f6,
  green: 0x22c55e,
  violet: 0x8b5cf6,
  rose: 0xf43f5e,
  custom: 0x6366f1,
}

const DEFAULT_CUSTOM_ARGB = 0xff6366f1

export interface AppearancePreferences {
  themeMode: ThemePreference
  darkTheme: BuiltinThemeId
  lightTheme: BuiltinThemeId
  colorScheme: AccentScheme
  /** 自定义强调色 RGB（仅 `colorScheme === 'custom'` 生效）。 */
  customRgb: number
  /** 80..150，步进 10。 */
  uiScalePercent: number
}

export const DEFAULT_APPEARANCE: AppearancePreferences = {
  themeMode: 'system',
  darkTheme: 'defaultDark',
  lightTheme: 'defaultLight',
  colorScheme: 'blue',
  customRgb: DEFAULT_CUSTOM_ARGB & 0xffffff,
  uiScalePercent: 100,
}

/** 内置主题本身的明暗外观（Rust `BuiltinThemeId::appearance`）。 */
export function builtinAppearance(id: BuiltinThemeId): ThemeModeName {
  return BUILTIN_APPEARANCE[id]
}

function parseBuiltin(value: unknown, mode: ThemeModeName): BuiltinThemeId | null {
  if (typeof value !== 'string') return null
  const name = value.startsWith('builtin:') ? value.slice('builtin:'.length) : value
  const id = BUILTIN_THEMES.find((candidate) => candidate === name)
  return id !== undefined && BUILTIN_APPEARANCE[id] === mode ? id : null
}

/** 接受 ARGB 整数或 6/8 位十六进制串（同 Rust `parse_argb`）；返回 RGB。 */
function parseArgbToRgb(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) {
    const int = Math.trunc(value)
    return int >= 0 && int <= 0xffffffff ? int & 0xffffff : null
  }
  if (typeof value === 'string') {
    const hex = value.trim().replace(/^#/, '')
    if (!/^[0-9a-fA-F]+$/.test(hex)) return null
    if (hex.length === 6) return Number.parseInt(hex, 16)
    if (hex.length === 8) return Number.parseInt(hex, 16) & 0xffffff
  }
  return null
}

function parseUiScalePercent(value: unknown): number | null {
  const scale = typeof value === 'number' ? value : typeof value === 'string' ? Number.parseFloat(value.trim()) : Number.NaN
  if (!Number.isFinite(scale)) return null
  const percent = Math.round(scale * 100)
  if (percent < 80 || percent > 150) return null
  return Math.floor((percent + 5) / 10) * 10
}

/** 偏好快照 → 外观选项；缺失/非法键回退默认（同 Rust `from_values`）。 */
export function parseAppearance(values: Readonly<Record<string, unknown>>): AppearancePreferences {
  const mode = values[THEME_MODE_KEY]
  const scheme = ACCENT_SCHEMES.find((candidate) => candidate === values[COLOR_SCHEME_KEY])
  return {
    themeMode: mode === 'light' || mode === 'dark' ? mode : 'system',
    darkTheme: parseBuiltin(values[DARK_THEME_KEY], 'dark') ?? DEFAULT_APPEARANCE.darkTheme,
    lightTheme: parseBuiltin(values[LIGHT_THEME_KEY], 'light') ?? DEFAULT_APPEARANCE.lightTheme,
    colorScheme: scheme ?? DEFAULT_APPEARANCE.colorScheme,
    customRgb: parseArgbToRgb(values[CUSTOM_COLOR_KEY]) ?? DEFAULT_APPEARANCE.customRgb,
    uiScalePercent: parseUiScalePercent(values[UI_SCALE_KEY]) ?? DEFAULT_APPEARANCE.uiScalePercent,
  }
}

/** 生效强调色 RGB（预设固定色，Custom 取用户色）。 */
export function accentRgb(prefs: AppearancePreferences): number {
  return prefs.colorScheme === 'custom' ? prefs.customRgb : ACCENT_PRESET_RGB[prefs.colorScheme]
}

export function accentHex(prefs: AppearancePreferences): string {
  return `#${accentRgb(prefs).toString(16).padStart(6, '0')}`
}

export function builtinExtends(id: BuiltinThemeId): string {
  return BUILTIN_EXTENDS[id]
}

/** 本地缓存的外观相关偏好键（登录页/首屏无快照时使用，避免闪烁）。 */
export const APPEARANCE_KEYS: readonly string[] = [
  THEME_MODE_KEY,
  DARK_THEME_KEY,
  LIGHT_THEME_KEY,
  COLOR_SCHEME_KEY,
  CUSTOM_COLOR_KEY,
  UI_SCALE_KEY,
]
