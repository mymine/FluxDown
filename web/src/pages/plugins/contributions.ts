// 插件贡献点推导（纯函数）：活动栏页、详情 tab、设置视图、任务右键命令、页头命令。
// 与 GPUI `fluxdown_ui_plugin_ui::Contributions` 同规则：只纳入 enabled + Loaded + 授权齐全的插件。

import { Sparkles } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { PluginCommandDeclDto, PluginDto, PluginViewDeclDto } from '../../lib/rpc'
import type { ChromeItemSpec } from '../../shell/chromeLayout'

/** 与 GPUI `RAIL_TITLE` 族一致：可放活动栏 / 标题栏，不放状态栏。 */
const RAIL_TITLE_FAMILIES: ChromeItemSpec['families'] = ['rail', 'title']

export function pluginUsable(plugin: PluginDto): boolean {
  return plugin.enabled && plugin.loadStatus === 'Loaded' && plugin.consentPending.length === 0 && plugin.consentPendingHosts.length === 0
}

export interface PluginViewRef {
  identity: string
  pluginName: string
  view: PluginViewDeclDto
}

// Web 无法运行桌面脚本：只有桌面实现（`portable: false`）的视图不出现在任何入口。
function viewsAt(plugins: readonly PluginDto[], location: string): PluginViewRef[] {
  return plugins.filter(pluginUsable).flatMap((plugin) =>
    plugin.contributions.views
      .filter((view) => view.portable && view.location === location)
      .map((view) => ({ identity: plugin.identity, pluginName: plugin.name, view })),
  )
}

// ── 路由 id（GPUI 动态路由与 Web 外壳条目共用）──

export function pluginRouteId(identity: string, viewId: string): string {
  return `plugin:${identity}:${viewId}`
}

export function parsePluginRouteId(id: string): { identity: string; viewId: string } | null {
  if (!id.startsWith('plugin:')) return null
  const rest = id.slice('plugin:'.length)
  const split = rest.lastIndexOf(':')
  if (split <= 0 || split === rest.length - 1) return null
  return { identity: rest.slice(0, split), viewId: rest.slice(split + 1) }
}

// ── 活动栏 ──

export interface PluginActivityEntry {
  /** `plugin:<identity>:<viewId>`。 */
  id: string
  /** 原文标签（非 i18n 键）。 */
  label: string
  /** 插件未声明图标时的默认图标。 */
  icon: LucideIcon
  /** 插件声明的图标：可直接用于 CSS `mask-image` 的 `url(...)`；未声明为 null。 */
  iconMask: string | null
  identity: string
  viewId: string
}

const SVG_NAMESPACE = 'http://www.w3.org/2000/svg'

/**
 * 插件 SVG 图标 → CSS `mask-image` 值。浏览器以图片解析 data URL 时要求根元素带 SVG 命名空间
 * （GPUI 渲染器不要求），缺失时补上；图片上下文不执行脚本、不加载外部资源。
 */
export function svgMaskUrl(svg: string): string {
  const normalized = svg.replace(/<svg\b[^>]*>/i, (tag) => (/\sxmlns\s*=/i.test(tag) ? tag : `${tag.slice(0, 4)} xmlns="${SVG_NAMESPACE}"${tag.slice(4)}`))
  return `url("data:image/svg+xml;charset=utf-8,${encodeURIComponent(normalized)}")`
}

export function deriveActivityEntries(plugins: readonly PluginDto[]): PluginActivityEntry[] {
  return viewsAt(plugins, 'activity').map(({ identity, view }) => ({
    id: pluginRouteId(identity, view.id),
    label: view.title,
    icon: Sparkles,
    iconMask: view.iconSvg === undefined ? null : svgMaskUrl(view.iconSvg),
    identity,
    viewId: view.id,
  }))
}

export function deriveChromeSpecs(entries: readonly PluginActivityEntry[]): ChromeItemSpec[] {
  return entries.map((entry) => ({ id: entry.id, defaultZone: 'railTop', families: RAIL_TITLE_FAMILIES }))
}

/** 当前路径是否是该插件页（路径段为 `encodeURIComponent` 后的 identity / viewId）。 */
export function isPluginEntryActive(entry: PluginActivityEntry, pathname: string): boolean {
  const match = /^\/plugins\/([^/]+)\/([^/]+)\/?$/.exec(pathname)
  if (!match) return false
  try {
    return decodeURIComponent(match[1] ?? '') === entry.identity && decodeURIComponent(match[2] ?? '') === entry.viewId
  } catch {
    return false
  }
}

// ── 详情 tab / 设置视图 ──

export interface PluginDetailTab extends PluginViewRef {
  /** 分段标签值：`plugin:<identity>:<viewId>`。 */
  value: string
}

export function deriveDetailTabs(plugins: readonly PluginDto[]): PluginDetailTab[] {
  return viewsAt(plugins, 'taskDetail').map((ref) => ({ ...ref, value: pluginRouteId(ref.identity, ref.view.id) }))
}

/** 插件的可移植设置视图（至多一个；插件未启用 / 授权不全时仍可设置，故不按 usable 过滤）。 */
export function settingsViewOf(plugin: PluginDto): PluginViewDeclDto | undefined {
  return plugin.contributions.views.find((view) => view.portable && view.location === 'settings')
}

/** 设置对话框里实际可运行的设置视图：与 GPUI 一致，仅插件可用（已启用且授权齐全）时才有。 */
export function activeSettingsViewOf(plugin: PluginDto): PluginViewDeclDto | undefined {
  return pluginUsable(plugin) ? settingsViewOf(plugin) : undefined
}

/** 插件的登录视图（可移植，至多一个）。 */
export function authViewOf(plugin: PluginDto): PluginViewDeclDto | undefined {
  return plugin.contributions.views.find((view) => view.portable && view.location === 'auth')
}

/** 登录弹窗实际承载的视图：仅插件可用时才有；有则弹窗只承载它，否则走固定登录流程。 */
export function activeAuthViewOf(plugin: PluginDto): PluginViewDeclDto | undefined {
  return pluginUsable(plugin) ? authViewOf(plugin) : undefined
}

/** 插件行是否显示设置按钮：有设置字段，或有可运行的设置视图（同 GPUI `plugins.rs`）。 */
export function hasPluginSettings(plugin: PluginDto): boolean {
  return plugin.settings.length > 0 || activeSettingsViewOf(plugin) !== undefined
}

// ── 命令 ──

export interface PluginCommandRef {
  identity: string
  pluginName: string
  command: PluginCommandDeclDto
}

function commandsWhere(plugins: readonly PluginDto[], pick: (command: PluginCommandDeclDto) => boolean): PluginCommandRef[] {
  return plugins
    .filter(pluginUsable)
    .flatMap((plugin) =>
      plugin.contributions.commands.filter(pick).map((command) => ({ identity: plugin.identity, pluginName: plugin.name, command })),
    )
}

/** 任务右键菜单命令（`taskMenu: true`）。 */
export function deriveTaskCommands(plugins: readonly PluginDto[]): PluginCommandRef[] {
  return commandsWhere(plugins, (command) => command.taskMenu)
}

/** 某插件的页头命令（`palette: true`；Web 没有命令面板，放在插件页页头）。 */
export function derivePaletteCommands(plugin: PluginDto): PluginCommandDeclDto[] {
  return pluginUsable(plugin) ? plugin.contributions.commands.filter((command) => command.palette) : []
}
