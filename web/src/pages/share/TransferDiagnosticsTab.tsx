// 任务详情「传输诊断」页：接收信息行（分享方 / 共享状态 / 停止共享 / 举报）+ FluxMesh接收任务的路径时间线、
// 各路径 RTT / 丢包 / cwnd / 速率、对端 NAT 与运营商。
// 只在任务有FluxMesh事实（`snapshot.share.taskFacts`）时作为 tab 出现；挂载期间按 2 秒轮询该会话。

import { useState } from 'react'
import { useT } from '../../i18n'
import { Badge, Button, Spinner } from '../../ui'
import { PROVIDE_STATE_KEYS, canSwitchBridge, providingRecordId } from './model'
import { ReportDialog } from './ReportDialog'
import { stopProviding } from './receive'
import { SessionDetailView } from './SessionDetail'
import { SwitchBridgeButton } from './SourceBadge'
import { useShareTaskFacts } from './state'
import { useShareDiagnostics } from './useShareDiagnostics'
import type { ShareTaskFactsDto } from '../../lib/rpc'

/** 接收信息行：来自谁、共享状态（共享中可停止）、弱化的举报入口。 */
function ReceiveInfo({ facts }: { facts: ShareTaskFactsDto }) {
  const t = useT()
  const [busy, setBusy] = useState(false)
  const [reporting, setReporting] = useState(false)
  const sender = (facts.senderDeviceName ?? '').trim()
  const provideKey = PROVIDE_STATE_KEYS[facts.provideState ?? 'off']
  const recordId = providingRecordId(facts)
  const shareId = facts.shareId

  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 rounded-md bg-chrome px-3 py-2 text-xs">
      <span className="min-w-0 max-w-full truncate text-foreground" title={sender}>
        {t('shareReceivedFrom', { name: sender === '' ? t('shareSenderUnknown') : sender })}
      </span>
      {provideKey ? <Badge tone={facts.provideState === 'providing' ? 'success' : 'neutral'}>{t(provideKey)}</Badge> : null}
      <span className="ml-auto flex shrink-0 items-center gap-1">
        {recordId ? (
          <Button
            variant="outline"
            className="h-7 px-2 text-xs"
            loading={busy}
            onClick={() => {
              setBusy(true)
              void stopProviding(recordId, t).finally(() => setBusy(false))
            }}
          >
            {t('shareStopProviding')}
          </Button>
        ) : null}
        {shareId !== null ? (
          <Button variant="ghost" className="h-7 px-2 text-xs text-text-tertiary" onClick={() => setReporting(true)}>
            {t('shareReport')}
          </Button>
        ) : null}
      </span>
      {reporting && shareId !== null ? <ReportDialog shareId={shareId} onClose={() => setReporting(false)} /> : null}
    </div>
  )
}

export function TransferDiagnosticsTab({ taskId, taskState }: { taskId: string; taskState: string }) {
  const t = useT()
  const facts = useShareTaskFacts(taskId)
  const sessionId = facts?.sessionId ?? undefined
  const { data } = useShareDiagnostics({ sessionId, enabled: sessionId !== undefined, intervalMs: 2000 })
  const detail = data?.sessions.find((session) => session.sessionId === sessionId)

  if (!facts) return null
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <ReceiveInfo facts={facts} />
      {facts.relayStuck ? <div className="rounded-md bg-warning/15 px-3 py-2 text-xs text-warning">{t('shareDiagRelayStuck')}</div> : null}
      {canSwitchBridge(facts, taskState) ? (
        <div>
          <SwitchBridgeButton taskId={taskId} emphasized={facts.relayStuck} />
        </div>
      ) : null}
      {detail ? (
        <SessionDetailView detail={detail} />
      ) : (
        <div className="flex items-center justify-center gap-2 py-6 text-sm text-muted-foreground">
          {sessionId === undefined ? null : <Spinner size="md" />}
          {t(sessionId === undefined ? 'shareDiagNoSession' : 'shareDiagLoading')}
        </div>
      )}
    </div>
  )
}
