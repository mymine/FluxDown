// 插件详情对话框的数据模型：已安装插件与市场条目各自映射到同一形状。

import type { MarketEntryDto, PluginDto, PluginPageScriptDeclDto } from '../../../../lib/rpc'

/** 详情数据；调用侧从各自 DTO 拆字段。 */
export interface PluginDetail {
  name: string
  version: string
  identity: string
  description: string
  homepage: string
  author: string
  tags: readonly string[]
  publishTime: string
  minAppVersion: string
  settingsCount: number
  permissions: readonly string[]
  yanked: string
  /** 页面脚本声明（标题 + 匹配站点）；市场条目不携带。 */
  pageScripts: readonly PluginPageScriptDeclDto[]
}

export function detailFromPlugin(plugin: PluginDto): PluginDetail {
  return {
    name: plugin.name,
    version: plugin.version,
    identity: plugin.identity,
    description: plugin.description,
    homepage: plugin.homepage,
    author: '',
    tags: [],
    publishTime: '',
    minAppVersion: '',
    settingsCount: plugin.settings.length,
    permissions: plugin.permissions,
    yanked: '',
    pageScripts: plugin.contributions.pageScripts,
  }
}

export function detailFromMarket(entry: MarketEntryDto): PluginDetail {
  return {
    name: entry.name === '' ? entry.pluginId : entry.name,
    version: entry.version,
    identity: entry.pluginId,
    description: entry.description,
    homepage: entry.homepage,
    author: entry.author,
    tags: entry.tags,
    publishTime: entry.publishTime,
    minAppVersion: entry.minAppVersion,
    settingsCount: 0,
    permissions: entry.permissions,
    yanked: entry.yanked,
    pageScripts: [],
  }
}
