// 推介「我的推荐码」：列表（复制 / 删除）+ 创建；活动关闭或断线时不可创建 / 删除。

import { Copy, Plus, Ticket, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useT } from '../../../../i18n'
import { copyText } from '../../../../lib/copy'
import { rpc } from '../../../../lib/rpc'
import type { CloudReferralCode, CloudReferralSummary } from '../../../../lib/rpc'
import { Button, Card, confirmDialog, EmptyState, Icon, Tooltip, toast } from '../../../../ui'
import { formatMinor, MAX_REFERRAL_CODES, REFERRAL_PAGE_SIZE } from '../account/billing'
import { accountErrorKey } from '../account/errorText'
import { CreateCodeDialog } from './CreateCodeDialog'
import { LoadFailed, LoadingBlock, NoticeCard, Pager } from './parts'
import { usePaged } from './usePaged'

function CodeRow({ item, disabled, onDelete }: { item: CloudReferralCode; disabled: boolean; onDelete: (item: CloudReferralCode) => void }) {
  const t = useT()
  return (
    <div className="flex flex-wrap items-center gap-3 px-4 py-3">
      <div className="flex min-w-0 flex-1 basis-48 flex-col gap-0.5">
        <span className="truncate font-mono text-title font-semibold tracking-wider text-foreground select-text">{item.code}</span>
        <span className="text-xs text-muted-foreground tabular">
          {t('accountReferralCodeStats', { count: item.paidOrderCount, amount: formatMinor(item.rewardMinor, 'CNY') })}
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <Tooltip content={t('accountReferralCodeCopy')}>
          <Button
            variant="ghost"
            iconOnly
            aria-label={t('accountReferralCodeCopy')}
            onClick={() => {
              copyText(item.code)
              toast.key('accountReferralCodeCopied', 'success')
            }}
          >
            <Icon icon={Copy} />
          </Button>
        </Tooltip>
        <Tooltip content={t('delete')}>
          <Button variant="ghost" iconOnly aria-label={t('delete')} className="text-muted-foreground hover:text-destructive" disabled={disabled} onClick={() => onDelete(item)}>
            <Icon icon={Trash2} />
          </Button>
        </Tooltip>
      </div>
    </div>
  )
}

export function CodesTab({ summary, disabled }: { summary: CloudReferralSummary; disabled: boolean }) {
  const t = useT()
  const [creating, setCreating] = useState(false)
  const { state, page, setPage, reload } = usePaged((target) => rpc.agent.referral.listCodes({ page: target, pageSize: REFERRAL_PAGE_SIZE }), '')
  const full = state.total >= MAX_REFERRAL_CODES
  const canWrite = summary.enabled && !disabled

  const remove = async (item: CloudReferralCode) => {
    const ok = await confirmDialog({
      title: t('accountReferralCodeDeleteConfirmTitle'),
      description: t('accountReferralCodeDeleteConfirmDesc'),
      okLabel: t('delete'),
      intent: 'destructive',
    })
    if (!ok) return
    try {
      await rpc.agent.referral.deleteCode({ id: item.id })
      reload()
    } catch (error) {
      toast.key(accountErrorKey(error), 'error')
    }
  }

  return (
    <>
      {summary.enabled ? null : <NoticeCard>{t('accountReferralDisabledNotice')}</NoticeCard>}
      <div className="flex flex-wrap items-center gap-2 px-1">
        <div className="min-w-0 flex-1 basis-40 text-sm font-medium text-foreground">{t('accountReferralTabCode')}</div>
        {full ? <span className="text-xs text-muted-foreground">{t('accountReferralCodeLimit', { max: MAX_REFERRAL_CODES })}</span> : null}
        <Button variant="primary" icon={Plus} disabled={!canWrite || full || state.status === 'loading'} onClick={() => setCreating(true)}>
          {t('accountReferralCodeCreate')}
        </Button>
      </div>
      {state.status === 'loading' && state.items.length === 0 ? <LoadingBlock /> : null}
      {state.status === 'error' ? <LoadFailed message={t(state.errorKey ?? 'accountReferralLoadFailed')} onRetry={reload} /> : null}
      {state.status !== 'error' && !(state.status === 'loading' && state.items.length === 0) ? (
        state.items.length === 0 ? (
          <Card>
            <EmptyState icon={Ticket} title={t('accountReferralCodeEmpty')} />
          </Card>
        ) : (
          <Card className="flex w-full flex-col overflow-hidden [&>*+*]:border-t [&>*+*]:border-hairline">
            {state.items.map((item) => (
              <CodeRow key={item.id} item={item} disabled={disabled} onDelete={(target) => void remove(target)} />
            ))}
          </Card>
        )
      ) : null}
      <Pager page={page} total={state.total} onPage={setPage} />
      {creating ? <CreateCodeDialog onClose={() => setCreating(false)} onCreated={reload} /> : null}
    </>
  )
}
