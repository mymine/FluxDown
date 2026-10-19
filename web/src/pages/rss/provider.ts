// 订阅来源（provider）纯模型：GPUI `crates/rss/src/provider.rs` 的镜像，
// 同一组用例见 `provider.test.ts`，改动必须两端同步。

import type { PluginDto } from '../../lib/rpc'

/** 内置 RSS / Atom / JSON Feed provider 的稳定 ID（与引擎 `RSS_PROVIDER_ID` 一致）。 */
export const BUILTIN_PROVIDER_ID = 'rss'

export type ProviderKind = 'builtin' | 'plugin' | 'missing'

export interface ProviderChoice {
  id: string
  /** 插件名；内置与 `missing` 项为空（调用方按 `kind` 取本地化文案或 `id`）。 */
  name: string
  kind: ProviderKind
}

/** 把空值归一为内置 provider，与引擎 `RssSourceInfo::normalize` 一致。 */
export function normalizeProviderId(id: string): string {
  const trimmed = id.trim()
  return trimmed === '' ? BUILTIN_PROVIDER_ID : trimmed
}

/** 插件 provider 当前能否被引擎调用：已启用、已加载、权限与网络主机均已授权。 */
function pluginUsable(plugin: PluginDto): boolean {
  return plugin.enabled && plugin.loadStatus === 'Loaded' && plugin.consentPending.length === 0 && plugin.consentPendingHosts.length === 0
}

/**
 * 来源下拉的选项：内置 RSS 固定在首位，随后是可用插件 provider（按插件 identity 升序，
 * 同一 provider ID 只取 identity 最小者——与引擎的冲突裁决一致），最后在 `current`
 * 不可用时追加一个 `missing` 项，避免编辑既有订阅时悄悄切回 RSS。
 */
export function providerChoices(plugins: readonly PluginDto[], current: string): ProviderChoice[] {
  const usable = plugins.filter(pluginUsable).sort((a, b) => (a.identity < b.identity ? -1 : a.identity > b.identity ? 1 : 0))
  const choices: ProviderChoice[] = [{ id: BUILTIN_PROVIDER_ID, name: '', kind: 'builtin' }]
  for (const plugin of usable) {
    for (const id of plugin.subscriptionProviderIds) {
      if (id === '' || choices.some((choice) => choice.id === id)) continue
      choices.push({ id, name: plugin.name === '' ? plugin.identity : plugin.name, kind: 'plugin' })
    }
  }
  const normalized = normalizeProviderId(current)
  if (!choices.some((choice) => choice.id === normalized)) choices.push({ id: normalized, name: '', kind: 'missing' })
  return choices
}

/**
 * provider 配置是不透明字符串；界面要求留空或为合法 JSON（插件通常 `JSON.parse`）。
 * 返回去掉首尾空白后的文本；非法返回 null。
 */
export function validateProviderConfig(text: string): string | null {
  const trimmed = text.trim()
  if (trimmed === '') return ''
  try {
    JSON.parse(trimmed)
    return trimmed
  } catch {
    return null
  }
}
