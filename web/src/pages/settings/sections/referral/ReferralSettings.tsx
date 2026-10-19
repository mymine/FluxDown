// 推介有奖页（GPUI `crates/settings` 推介页）：说明 / 我的推荐码 / 收益记录三标签。
// 进入拉 `agent.referral.summary`；活动关闭时说明与推荐码页提示，收益记录仍可查看。

import { useCallback, useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../../../../i18n'
import { rpc, useConnection } from '../../../../lib/rpc'
import type { CloudReferralSummary } from '../../../../lib/rpc'
import { SettingsPage } from '../../kit'
import { accountErrorKey } from '../account/errorText'
import { CodesTab } from './CodesTab'
import { IntroTab } from './IntroTab'
import { LoadFailed, LoadingBlock } from './parts'
import { RecordsTab } from './RecordsTab'

type SummaryState = { status: 'loading' } | { status: 'error'; key: string } | { status: 'ready'; summary: CloudReferralSummary }

export function ReferralSettings() {
  const t = useT()
  const disabled = useConnection().phase !== 'ready'
  const [state, setState] = useState<SummaryState>({ status: 'loading' })

  const [version, setVersion] = useState(0)
  const load = useCallback(() => setVersion((value) => value + 1), [])

  useEffect(() => {
    let alive = true
    setState({ status: 'loading' })
    rpc.agent.referral
      .summary()
      .then((summary) => {
        if (alive) setState({ status: 'ready', summary })
      })
      .catch((error: unknown) => {
        if (!alive) return
        console.warn('[referral] summary failed', error)
        setState({ status: 'error', key: accountErrorKey(error) })
      })
    return () => {
      alive = false
    }
  }, [version])

  const gated = (render: (summary: CloudReferralSummary) => ReactNode): ReactNode => {
    if (state.status === 'loading') return <LoadingBlock />
    if (state.status === 'error') return <LoadFailed message={`${t('accountReferralLoadFailed')}：${t(state.key)}`} onRetry={load} />
    return render(state.summary)
  }

  return (
    <SettingsPage
      title={t('settingsCatReferral')}
      description={t('settingsCatReferralDesc')}
      tabs={[
        { id: 'intro', label: t('accountReferralTabIntro'), content: gated((summary) => <IntroTab summary={summary} />) },
        { id: 'codes', label: t('accountReferralTabCode'), content: gated((summary) => <CodesTab summary={summary} disabled={disabled} />) },
        { id: 'records', label: t('accountReferralTabRecords'), content: <RecordsTab /> },
      ]}
    />
  )
}
