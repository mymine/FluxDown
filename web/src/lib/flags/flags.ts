// 国旗：国家码规整与资源查找的纯逻辑（Vite 资源表见 flagUrls.ts，保持本文件可在 bun test 下运行）。
// 资源 `assets/flags/<小写码>.svg` 与 GPUI 同一份（lipis/flag-icons，MIT）。

/** flag-icons 的「未知」旗；码合法但无对应文件时的占位。 */
export const UNKNOWN_FLAG = 'xx'

/** 后端 `BtPeerDto.country`（ISO 3166-1 alpha-2）→ 大写两位字母；缺省 / 非法 = `''`（不显示国旗）。 */
export function normalizeCountry(country: string | null | undefined): string {
  const code = (country ?? '').trim().toUpperCase()
  return /^[A-Z]{2}$/.test(code) ? code : ''
}

/** 国家码 → 资源文件主名（小写）；非法 = `null`。 */
export function flagKey(country: string | null | undefined): string | null {
  const code = normalizeCountry(country)
  return code === '' ? null : code.toLowerCase()
}

/** 国家码 → 国旗 URL：非法 = `null`（调用方留空位）；码合法但缺文件 = 「未知」旗，再缺则 `null`。 */
export function resolveFlagUrl(urls: Readonly<Record<string, string>>, country: string | null | undefined): string | null {
  const key = flagKey(country)
  if (key === null) return null
  return urls[key] ?? urls[UNKNOWN_FLAG] ?? null
}

const regionNames = new Map<string, Intl.DisplayNames | null>()

function displayNames(locale: string): Intl.DisplayNames | null {
  const cached = regionNames.get(locale)
  if (cached !== undefined) return cached
  let names: Intl.DisplayNames | null = null
  try {
    names = new Intl.DisplayNames([locale], { type: 'region' })
  } catch {
    // locale 不被运行时识别：退回国家码。
  }
  regionNames.set(locale, names)
  return names
}

/** 当前界面语言的地区名；无法本地化时 = 国家码。 */
export function regionName(country: string, locale: string): string {
  try {
    return displayNames(locale)?.of(country) ?? country
  } catch {
    return country
  }
}

/** 悬停提示：`地区名 (CC)`；地区名缺失 / 与码相同时只有码。 */
export function flagTitle(country: string, locale: string): string {
  const name = regionName(country, locale)
  return name === country ? country : `${name} (${country})`
}
