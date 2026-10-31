import { useNavigate } from '@tanstack/react-router'
import { useEffect, useMemo } from 'react'
import { useDaemon } from '../../lib/rpc'
import type { DaemonSnapshot, PluginDto } from '../../lib/rpc'
import { setPluginChromeSpecs } from '../../shell/chromeCatalog'
import { ACTIVITY_ENTRIES } from '../../shell/activity'
import { deriveActivityEntries, deriveChromeSpecs, deriveDetailTabs, deriveTaskCommands, parsePluginRouteId } from './contributions'
import { setPluginNavigator, startPluginToasts } from './view/viewStore'
import { NAV_BUILTIN_TARGETS } from '../../lib/rpc/protocol/pluginView'

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
  const navigate = useNavigate()
  useEffect(() => {
    setPluginChromeSpecs(deriveChromeSpecs(entries))
  }, [entries])
  useEffect(() => startPluginToasts(), [])

  // 插件视图的 `ctx.navigate`：内置 id → 对应路由；插件页须是当前已贡献的活动栏条目，否则忽略。
  useEffect(() => {
    setPluginNavigator((target) => {
      const builtin = (NAV_BUILTIN_TARGETS as readonly string[]).includes(target) ? ACTIVITY_ENTRIES.find((entry) => entry.id === target) : undefined
      if (builtin?.to) {
        void navigate({ to: builtin.to })
        return
      }
      const parsed = parsePluginRouteId(target)
      if (parsed && entries.some((entry) => entry.id === target)) {
        void navigate({ to: '/plugins/$pluginId/$viewId', params: { pluginId: parsed.identity, viewId: parsed.viewId } })
        return
      }
      console.warn('plugin navigate ignored: target not available on this client', target)
    })
    return () => setPluginNavigator(null)
  }, [entries, navigate])
}
