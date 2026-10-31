// FluxMesh页顶部摘要行（GPUI 分享页同语义）：最受限的一项额度 + 贡献等级 / 本月贡献 +「共享已接收的文件」开关 +
// 「额度详情」链接（完整额度视图在设置 › 账户）。未登录时只显示登录引导链接。

import { useNavigate } from '@tanstack/react-router'
import { useT } from '../../i18n'
import { Switch } from '../../ui'
import { saveLocalConfig } from '../settings/sections/share/localConfig'
import { formatShortTime } from '../webhooks/cloudLogic'
import { formatQuotaBytes, levelLabel, usageMeters, usageSummaryLine } from './model'
import { useShareSignedIn, useShareState } from './state'
import { useUsageRefresh } from './useUsageRefresh'

const LINK = 'shrink-0 text-xs text-primary underline-offset-2 hover:underline'

export function ShareSummary() {
  const t = useT()
  const navigate = useNavigate()
  const signedIn = useShareSignedIn()
  const { usage, localConfig } = useShareState()
  useUsageRefresh()
  const toAccount = () => void navigate({ to: '/settings/$category', params: { category: 'account' } })

  const line = usage ? usageSummaryLine(usageMeters(usage)) : null
  const quota =
    usage && line
      ? line.kind === 'unlimited'
        ? t('shareUsageUnlimited')
        : `${t(line.meter.labelKey)} · ${
            line.kind === 'exhausted'
              ? t('shareUsageExhausted', { time: formatShortTime(usage.resetAt) })
              : t('shareUsageRemaining', { remaining: formatQuotaBytes(line.meter.remaining), allowance: formatQuotaBytes(line.meter.allowance) })
          }`
      : null

  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 rounded-md bg-chrome px-3 py-2">
      {signedIn ? (
        <>
          {quota ? (
            <span className={`min-w-0 break-words text-xs tabular ${line?.kind === 'exhausted' ? 'text-destructive' : 'text-foreground'}`}>{quota}</span>
          ) : null}
          {usage ? (
            <span className="min-w-0 break-words text-xs text-muted-foreground tabular">
              {levelLabel(usage.contribution.level, usage.contribution.levelMax)} · {t('shareUsageContribMonth', { bytes: formatQuotaBytes(usage.contribution.monthBytes) })}
            </span>
          ) : null}
        </>
      ) : (
        <button type="button" className={LINK} onClick={toAccount}>
          {t('shareUsageLogin')}
        </button>
      )}
      <label className="ml-auto flex min-w-0 items-center gap-2 text-xs text-foreground">
        <span className="min-w-0 truncate">{t('shareSetAutoProvide')}</span>
        <Switch
          checked={localConfig.autoProvide !== false}
          aria-label={t('shareSetAutoProvide')}
          onCheckedChange={(on) => void saveLocalConfig({ autoProvide: on ? null : false })}
        />
      </label>
      {signedIn ? (
        <button type="button" className={LINK} onClick={toAccount}>
          {t('shareSummaryDetails')}
        </button>
      ) : null}
    </div>
  )
}
