// 「多插件择一」对话框：2+ 个已启用插件的 resolver 同时命中同一 URL 时，任务首次启动前询问使用哪个插件。
// 回答走 `daemon.selection.resolve`；`cancelled` = 取消任务（下次启动会再次询问）；
// 超过 `deadlineUnixMs` 服务端按 `defaultChoice` 自动决定（且不记住）。
// 与 GPUI / 变体选择对话框对齐：默认候选预选并带「默认」徽标，勾选「记住」后该站点以后不再询问。

import { Check } from 'lucide-react'
import { useState } from 'react'
import { useT } from '../../../i18n'
import { rpc } from '../../../lib/rpc'
import type { PluginCandidateDto, SelectionOutcome, SelectionRequestDto } from '../../../lib/rpc'
import { cn } from '../../../lib/cn'
import { toastRpcError } from '../../../lib/rpcToast'
import { Badge, Button, CheckRow, Dialog, DialogFooter, FieldHint, Icon } from '../../../ui'
import { buildPluginOutcome, candidateTitle, defaultPluginIdentity } from './pluginChoice'
import { useNow } from './utils'

export function PluginChoiceDialog({
  request,
  site,
  url,
  candidates,
}: {
  request: SelectionRequestDto
  site: string
  url: string
  candidates: readonly PluginCandidateDto[]
}) {
  const t = useT()
  const now = useNow(1000)
  const defaultIdentity = defaultPluginIdentity(request, candidates)
  const [identity, setIdentity] = useState(defaultIdentity)
  const [remember, setRemember] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const remaining = Math.max(0, Math.floor((request.deadlineUnixMs - now) / 1000))

  const resolve = async (outcome: SelectionOutcome | null) => {
    if (submitting || outcome === null) return
    setSubmitting(true)
    try {
      await rpc.daemon.selection.resolve({ requestId: request.requestId, outcome })
    } catch (error) {
      toastRpcError(error)
      setSubmitting(false)
    }
  }
  const cancel = () => void resolve({ kind: 'cancelled' })
  const outcome = buildPluginOutcome(identity, remember, candidates)

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) cancel()
      }}
      size="md"
      title={t('pluginChoiceTitle')}
      description={
        <span className="flex flex-col gap-1">
          <span>{t('pluginChoiceDesc', { count: candidates.length, site })}</span>
          <span className="truncate text-xs text-text-tertiary" title={url}>
            {url}
          </span>
        </span>
      }
      footer={
        <div className="flex flex-wrap items-center justify-between gap-2">
          <FieldHint className="tabular">{t('selectionAutoDefaultIn', { seconds: remaining })}</FieldHint>
          <DialogFooter className="ml-auto">
            <Button variant="outline" disabled={submitting} onClick={cancel}>
              {t('cancel')}
            </Button>
            <Button variant="primary" disabled={submitting || outcome === null} onClick={() => void resolve(outcome)}>
              {t('pluginChoiceConfirm')}
            </Button>
          </DialogFooter>
        </div>
      }
    >
      <div className="flex flex-col gap-3">
        <div role="radiogroup" aria-label={t('pluginChoiceTitle')} className="flex flex-col gap-2">
          {candidates.map((candidate) => (
            <CandidateCard
              key={candidate.identity}
              candidate={candidate}
              selected={candidate.identity === identity}
              isDefault={candidate.identity === defaultIdentity}
              disabled={submitting}
              onSelect={() => setIdentity(candidate.identity)}
            />
          ))}
        </div>
        <div className="flex flex-col gap-0.5">
          <CheckRow checked={remember} onCheckedChange={setRemember} disabled={submitting}>
            {t('pluginChoiceRemember', { site })}
          </CheckRow>
          <FieldHint className="pl-7">{t('pluginChoiceRememberHint')}</FieldHint>
        </div>
      </div>
    </Dialog>
  )
}

function CandidateCard({
  candidate,
  selected,
  isDefault,
  disabled,
  onSelect,
}: {
  candidate: PluginCandidateDto
  selected: boolean
  isDefault: boolean
  disabled: boolean
  onSelect: () => void
}) {
  const t = useT()
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        'flex w-full items-start gap-3 rounded-md border px-3 py-2.5 text-left transition-colors coarse:min-h-touch',
        'focus-visible:border-ring disabled:cursor-not-allowed disabled:opacity-50',
        selected ? 'border-primary bg-accent' : 'border-hairline hover:bg-row-hover',
      )}
    >
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="min-w-0 break-words text-sm font-medium text-foreground">{candidateTitle(candidate)}</span>
          {isDefault ? <Badge tone="accent">{t('pluginChoiceDefaultBadge')}</Badge> : null}
        </span>
        <span className="tabular truncate font-mono text-xs text-muted-foreground">
          v{candidate.version} · {candidate.identity}
        </span>
        {candidate.description ? <span className="line-clamp-2 break-words text-xs text-muted-foreground">{candidate.description}</span> : null}
      </span>
      <span
        aria-hidden
        className={cn(
          'mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border',
          selected ? 'border-primary bg-primary text-primary-foreground' : 'border-input',
        )}
      >
        {selected ? <Icon icon={Check} size="sm" /> : null}
      </span>
    </button>
  )
}
