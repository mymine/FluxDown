// 任务行来源徽标（直连·局域网 / 直连·公网 / 中继·限速 / 云端 / 桥接 / 连接中）+ 进行中接收任务的「切换桥接」。
// 只有FluxMesh接收任务有；事实来自 `snapshot.share.taskFacts`。

import { useState } from 'react'
import { useT } from '../../i18n'
import { rpc } from '../../lib/rpc'
import { Badge, toast } from '../../ui'
import { accountErrorKey } from '../settings/sections/account/errorText'
import { useNavigate } from '@tanstack/react-router'
import { canSwitchBridge, shareSourceBadge } from './model'
import { useShareTaskFacts } from './state'

/** 「切换桥接」：agent 不可用时返回类型化错误（如 `shareBridgeUnavailable`），原因经 toast 如实告知；`relayStuck` 时强调。 */
export function SwitchBridgeButton({ taskId, emphasized, className }: { taskId: string; emphasized?: boolean; className?: string }) {
  const t = useT()
  const [busy, setBusy] = useState(false)
  return (
    <button
      type="button"
      disabled={busy}
      className={
        className ??
        `shrink-0 text-xs underline-offset-2 hover:underline disabled:opacity-50 ${emphasized ? 'font-medium text-warning' : 'text-primary'}`
      }
      onClick={(event) => {
        event.stopPropagation()
        setBusy(true)
        rpc.agent.share
          .switchToBridge({ taskId })
          .catch((error: unknown) => toast.error(t(accountErrorKey(error))))
          .finally(() => setBusy(false))
      }}
      onDoubleClick={(event) => event.stopPropagation()}
    >
      {t('shareSwitchBridge')}
    </button>
  )
}

export function SourceBadge({ taskId, taskState }: { taskId: string; taskState: string }) {
  const t = useT()
  const facts = useShareTaskFacts(taskId)
  const navigate = useNavigate()
  const badge = shareSourceBadge(facts, taskState)
  if (!badge) return null
  return (
    <span className="flex shrink-0 items-center gap-1.5">
      <Badge tone={badge.tone}>{t(badge.labelKey)}
        {badge.peers > 0 ? ` ${t('shareBadgePeers', { count: badge.peers })}` : ''}
      </Badge>
      {badge.swarmBlockKey ? (
        <button
          type="button"
          className="shrink-0 whitespace-nowrap text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
          onClick={(event) => {
            event.stopPropagation()
            void navigate({ to: '/settings/$category', params: { category: 'account' } })
          }}
        >
          {t(badge.swarmBlockKey)}
        </button>
      ) : null}
      {canSwitchBridge(facts, taskState) ? <SwitchBridgeButton taskId={taskId} emphasized={facts?.relayStuck === true} /> : null}
    </span>
  )
}
