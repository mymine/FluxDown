import { useEffect, useMemo } from 'react'
import { useDaemon } from '../../lib/rpc'
import type { DaemonSnapshot, PluginDto } from '../../lib/rpc'
import { setPluginChromeSpecs } from '../../shell/chromeCatalog'
import { deriveActivityEntries, deriveChromeSpecs, deriveDetailTabs, deriveTaskCommands } from './contributions'
import { startPluginToasts } from './view/viewStore'

const selectPlugins = (daemon: DaemonSnapshot) => daemon.plugins
const EMPTY_PLUGINS: readonly PluginDto[] = []

export function usePlugins(): readonly PluginDto[] {
  return useDaemon(selectPlugins, EMPTY_PLUGINS)
}

/** 活动栏插件条目（enabled + Loaded + 授权齐全 + location == activity）。 */
export function usePluginActivityEntries() {
  const plugins = usePlugins()
  return useMemo(() => deriveActivityEntries(plugins), [plugins])
}

export function usePluginDetailTabs() {
  const plugins = usePlugins()
  return useMemo(() => deriveDetailTabs(plugins), [plugins])
}

export function usePluginTaskCommands() {
  const plugins = usePlugins()
  return useMemo(() => deriveTaskCommands(plugins), [plugins])
}

/** 外壳级接线（挂一次）：插件活动页并入外壳条目目录；`pluginToast` 事件转 toast。 */
export function usePluginShellBridge(): void {
  const entries = usePluginActivityEntries()
  useEffect(() => {
    setPluginChromeSpecs(deriveChromeSpecs(entries))
  }, [entries])
  useEffect(() => startPluginToasts(), [])
}
