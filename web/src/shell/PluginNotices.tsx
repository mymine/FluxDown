// 全局插件提示：熔断器 / 市场撤回自动禁用插件是全局事件，与当前页面无关。挂在 AppShell，任何页面都生效
// （对应 GPUI `crates/app/src/plugin_notices.rs`）。

import { useCallback } from 'react'
import { useT } from '../i18n'
import { rpcStore, useServiceEvents } from '../lib/rpc'
import type { EventFrame } from '../lib/rpc'
import { toast } from '../ui'

export function PluginNotices() {
  const t = useT()
  useServiceEvents(
    useCallback(
      (frame: EventFrame) => {
        const outer = frame.event
        // daemon 事件可直达（`service: 'daemon'`）或经 agent 转发。
        const daemonEvent = outer.service === 'daemon' ? outer.event : outer.service === 'agent' && outer.event.type === 'daemon' ? outer.event.data : null
        if (!daemonEvent || daemonEvent.type !== 'engine') return
        const message = daemonEvent.data
        if (message.type !== 'pluginAutoDisabled') return
        // 引擎先广播最新插件表再发本事件；插件已被卸载时回退显示 identity。
        const plugin = rpcStore.peek().snapshot?.daemon.plugins.find((entry) => entry.identity === message.identity)
        toast.warning(t(message.reason === 'Revoked' ? 'pluginRevokedToast' : 'pluginAutoDisabledToast', { name: plugin?.name ?? message.identity }))
      },
      [t],
    ),
  )
  return null
}
