// 账户页「额度与贡献」区块（GPUI `crates/share` 的 `ShareUsageView` 嵌入账户页的 Web 对应）：
// 三条额度条、额度明细、贡献与等级。仅登录且有额度数据时出现；购买入口在上方账户卡片的「升级 / 流量包」。
// 挂载（账户页可见）时向 agent 取一次最新额度（agent 侧 30 s 缓存），结果经 `shareChanged` 推到快照。

import { RefreshCw } from 'lucide-react'
import { useT } from '../../../../i18n'
import type { TFunction } from '../../../../i18n'
import { cn } from '../../../../lib/cn'
import type { ShareEarnDto, ShareUsageDto } from '../../../../lib/rpc'
import { Badge, Button, Icon, ProgressBar } from '../../../../ui'
import { boostLabel, formatQuotaBytes, grantSourceKey, levelLabel, trafficKindKey, usageMeters } from '../../../share/model'
import type { MeterView } from '../../../share/model'
import { useShareAvailable, useShareSignedIn, useShareState } from '../../../share/state'
import { useUsageRefresh } from '../../../share/useUsageRefresh'
import { formatShortTime } from '../../../webhooks/cloudLogic'
import { SettingsSection } from '../../kit'

function Meter({ meter, resetAt }: { meter: MeterView; resetAt: string }) {
  const t = useT()
  const reset = formatShortTime(resetAt)
  const text = meter.unlimited
    ? t('shareUsageUnlimited')
    : meter.exhausted
      ? t('shareUsageExhausted', { time: reset })
      : t('shareUsageRemaining', { remaining: formatQuotaBytes(meter.remaining), allowance: formatQuotaBytes(meter.allowance) })
  return (
    <div id={`share-usage-meter-${meter.kind}`} className="flex min-w-0 flex-col gap-1">
      <div className="flex flex-wrap items-baseline justify-between gap-x-2 text-xs">
        <span className={cn('font-medium', meter.exhausted ? 'text-destructive' : 'text-foreground')}>{t(meter.labelKey)}</span>
        <span className={cn('tabular break-words', meter.exhausted ? 'text-destructive' : 'text-muted-foreground')}>{text}</span>
      </div>
      {meter.unlimited ? null : <ProgressBar value={meter.ratio} className={cn(meter.exhausted && '[&>div]:!bg-destructive')} />}
    </div>
  )
}

function earnLine(t: TFunction, earn: ShareEarnDto): string | null {
  const key = trafficKindKey(earn.kind)
  if (!key) return null
  const kind = t(key)
  const earned = formatQuotaBytes(earn.earnedThisMonth)
  return earn.capBytes > 0
    ? t('shareUsageEarned', { kind, earned, cap: formatQuotaBytes(earn.capBytes), months: earn.validMonths })
    : t('shareUsageEarnedNoCap', { kind, earned })
}

function Body({ usage, failed, onRefresh, refreshing }: { usage: ShareUsageDto; failed: boolean; onRefresh: () => void; refreshing: boolean }) {
  const t = useT()
  const meters = usageMeters(usage)
  const c = usage.contribution
  const boost = boostLabel(c.boostPermille)
  const priority = !c.priorityActive
    ? null
    : c.priority
      ? t('shareUsagePriorityOn')
      : c.priorityThresholdBytes > 0
        ? t('shareUsagePriorityOffContrib', { bytes: formatQuotaBytes(c.priorityThresholdBytes), days: c.priorityWindowDays })
        : t('shareUsagePriorityOffPlan')
  const earnLines = c.earn.map((item) => earnLine(t, item)).filter((line): line is string => line !== null)

  return (
    <SettingsSection title={t('shareTabUsage')}>
      <div className="flex min-w-0 flex-col gap-3 p-4">
        <div className="flex items-center justify-between gap-2">
          <span className="min-w-0 text-sm font-semibold text-foreground">{t('shareUsageMonth', { month: usage.month, time: formatShortTime(usage.resetAt) })}</span>
          <Button id="share-usage-refresh" variant="ghost" iconOnly aria-label={t('accountCloudRefresh')} loading={refreshing} onClick={onRefresh}>
            <Icon icon={RefreshCw} />
          </Button>
        </div>
        {usage.enforced.swarm ? null : <span className="text-xs text-text-tertiary">{t('shareUsageNotEnforced')}</span>}
        {failed ? <span className="text-xs text-destructive">{t('shareUsageLoadFailed')}</span> : null}
        {meters.map((meter) => (
          <Meter key={meter.kind} meter={meter} resetAt={usage.resetAt} />
        ))}
      </div>

      {usage.grants.length > 0 ? (
        <div className="flex min-w-0 flex-col gap-2 p-4">
          <span className="text-sm font-semibold text-foreground">{t('shareUsageGrantsTitle')}</span>
          <div className="overflow-x-auto">
            <table className="w-full min-w-[420px] text-left text-xs">
              <thead className="text-text-tertiary">
                <tr>
                  <th className="py-1 pr-3 font-normal">{t('shareUsageColSource')}</th>
                  <th className="py-1 pr-3 font-normal">{t('shareUsageColKind')}</th>
                  <th className="py-1 pr-3 font-normal">{t('shareUsageColRemaining')}</th>
                  <th className="py-1 font-normal">{t('shareUsageColExpires')}</th>
                </tr>
              </thead>
              <tbody className="text-foreground">
                {usage.grants.map((grant, index) => {
                  const kindKey = trafficKindKey(grant.kind)
                  return (
                    <tr key={index} className="border-t border-hairline">
                      <td className="py-1.5 pr-3">{t(grantSourceKey(grant.source))}</td>
                      <td className="py-1.5 pr-3">{kindKey ? t(kindKey) : '—'}</td>
                      <td className="py-1.5 pr-3 tabular">{formatQuotaBytes(Math.max(0, grant.bytes - grant.usedBytes))}</td>
                      <td className="py-1.5 tabular">{grant.expiresAt ? formatShortTime(grant.expiresAt) : t('shareUsageGrantNoExpiry')}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        </div>
      ) : null}

      <div className="flex min-w-0 flex-col gap-2 p-4">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-semibold text-foreground">{t('shareUsageContributionTitle')}</span>
          <Badge tone="accent">{levelLabel(c.level, c.levelMax)}</Badge>
          {c.nextLevelBytes !== null && c.level < c.levelMax ? (
            <span className="text-xs text-muted-foreground tabular">{t('shareUsageNextLevel', { bytes: formatQuotaBytes(Math.max(0, c.nextLevelBytes - c.lifetimeBytes)) })}</span>
          ) : null}
        </div>
        <span className="text-xs text-muted-foreground tabular">{t('shareUsageContribMonth', { bytes: formatQuotaBytes(c.monthBytes) })}</span>
        {boost ? <span className="text-xs text-muted-foreground">{t('shareUsageBoost', { boost })}</span> : null}
        {earnLines.map((line) => (
          <span key={line} className="text-xs text-muted-foreground tabular break-words">
            {line}
          </span>
        ))}
        {priority ? <span className="text-xs text-muted-foreground">{priority}</span> : null}
      </div>
    </SettingsSection>
  )
}

export function ShareUsageSection() {
  const signedIn = useShareSignedIn()
  const available = useShareAvailable()
  const { usage, usageError } = useShareState()
  const { refreshing, refresh } = useUsageRefresh()

  if (!available || !signedIn || !usage) return null
  return <Body usage={usage} failed={usageError != null} refreshing={refreshing} onRefresh={refresh} />
}
