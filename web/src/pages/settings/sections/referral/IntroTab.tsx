// 推介「说明」：统计卡 + 活动说明 + 规则卡 + 兑付联系方式。

import { useT } from '../../../../i18n'
import type { CloudReferralSummary } from '../../../../lib/rpc'
import { Card } from '../../../../ui'
import { formatMinor } from '../account/billing'
import { NoticeCard } from './parts'

function StatCard({ label, value, accent }: { label: string; value: string; accent?: boolean }) {
  return (
    <Card className="flex min-w-0 flex-col gap-1 px-4 py-3">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className={accent ? 'truncate text-[24px] leading-8 font-semibold text-accent-text tabular' : 'truncate text-[24px] leading-8 font-semibold text-foreground tabular'}>{value}</span>
    </Card>
  )
}

export function IntroTab({ summary }: { summary: CloudReferralSummary }) {
  const t = useT()
  const description = summary.description.trim()
  const contact = summary.contact.trim()
  return (
    <>
      {summary.enabled ? null : <NoticeCard>{t('accountReferralDisabledNotice')}</NoticeCard>}
      <div className="grid grid-cols-3 gap-3 narrow:grid-cols-1">
        <StatCard label={t('accountReferralStatPending')} value={formatMinor(summary.pendingRewardMinor, 'CNY')} />
        <StatCard label={t('accountReferralStatPaid')} value={formatMinor(summary.paidRewardMinor, 'CNY')} accent />
        <StatCard label={t('accountReferralStatInvited')} value={String(summary.invitedCount)} />
      </div>
      {summary.rewardEnabled ? null : <NoticeCard>{t('accountReferralRewardDisabledNotice')}</NoticeCard>}
      {description ? <div className="whitespace-pre-wrap break-words px-1 text-sm text-foreground">{description}</div> : null}
      {summary.rules.length > 0 ? (
        <Card className="flex flex-col gap-2 p-4">
          <div className="text-sm font-medium text-foreground">{t('accountReferralRulesTitle')}</div>
          <ul className="flex flex-col gap-1.5">
            {summary.rules.map((rule) => (
              <li key={rule.planCode} className="flex items-start gap-2 text-xs text-muted-foreground">
                <span className="mt-1.5 size-1 shrink-0 rounded-full bg-accent-text" />
                <span className="min-w-0 break-words">
                  {t('accountReferralRuleItem', { planName: rule.planName, percent: rule.rewardPercent, discount: formatMinor(rule.discountMinor, 'CNY') })}
                </span>
              </li>
            ))}
          </ul>
        </Card>
      ) : null}
      {contact ? <div className="px-1 text-xs text-muted-foreground">{t('accountReferralRedeemHint', { contact })}</div> : null}
    </>
  )
}
