// 推介「收益记录」：搜索（300ms 防抖，回第 1 页）+ 分页列表。

import { Gift, Search } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useT } from '../../../../i18n'
import { rpc } from '../../../../lib/rpc'
import type { CloudReferralRecord } from '../../../../lib/rpc'
import { Badge, Card, EmptyState, Icon, Input } from '../../../../ui'
import { formatMinor, localDate, recordStatus, REFERRAL_PAGE_SIZE } from '../account/billing'
import { LoadFailed, LoadingBlock, Pager } from './parts'
import { usePaged } from './usePaged'

const SEARCH_DEBOUNCE_MS = 300

function RecordRow({ item }: { item: CloudReferralRecord }) {
  const t = useT()
  const status = recordStatus(item.status)
  const meta = [item.referralCode, localDate(item.createdAt)].filter((part): part is string => !!part)
  return (
    <div className="flex flex-wrap items-center gap-3 px-4 py-3">
      <div className="flex min-w-0 flex-1 basis-44 flex-col gap-0.5">
        <span className="truncate text-sm font-medium text-foreground">{item.buyerLabel}</span>
        <span className="truncate text-xs text-muted-foreground tabular">{meta.join(' · ')}</span>
      </div>
      <div className="flex shrink-0 flex-col items-end gap-0.5">
        <span className="text-sm font-semibold text-accent-text tabular">+{formatMinor(item.rewardMinor, 'CNY')}</span>
        <span className="text-caption text-muted-foreground tabular">
          {t('accountReferralRecordOrderAmount', { amount: formatMinor(item.orderAmountMinor, 'CNY') })} · {item.rewardPercent}%
        </span>
      </div>
      <Badge tone={status.tone}>{t(status.key)}</Badge>
    </div>
  )
}

export function RecordsTab() {
  const t = useT()
  const [query, setQuery] = useState('')
  const [term, setTerm] = useState('')
  const { state, page, setPage, reload } = usePaged(
    (target) => rpc.agent.referral.listRecords({ page: target, pageSize: REFERRAL_PAGE_SIZE, ...(term !== '' ? { search: term } : {}) }),
    term,
  )

  useEffect(() => {
    const timer = window.setTimeout(() => {
      const next = query.trim()
      if (next === term) return
      setTerm(next)
      setPage(1)
    }, SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(timer)
  }, [query, term, setPage])

  return (
    <>
      <Input
        value={query}
        placeholder={t('accountReferralSearchPlaceholder')}
        aria-label={t('accountReferralSearchPlaceholder')}
        onChange={(event) => setQuery(event.target.value)}
        trailing={<Icon icon={Search} className="text-text-tertiary" />}
      />
      {state.status === 'loading' && state.items.length === 0 ? <LoadingBlock /> : null}
      {state.status === 'error' ? <LoadFailed message={t(state.errorKey ?? 'accountReferralLoadFailed')} onRetry={reload} /> : null}
      {state.status !== 'error' && !(state.status === 'loading' && state.items.length === 0) ? (
        state.items.length === 0 ? (
          <Card>
            <EmptyState icon={term === '' ? Gift : Search} title={t(term === '' ? 'accountReferralRecordsEmpty' : 'accountReferralSearchEmpty')} />
          </Card>
        ) : (
          <Card className="flex w-full flex-col overflow-hidden [&>*+*]:border-t [&>*+*]:border-hairline">
            {state.items.map((item) => (
              <RecordRow key={item.id} item={item} />
            ))}
          </Card>
        )
      ) : null}
      <Pager page={page} total={state.total} onPage={setPage} />
    </>
  )
}
