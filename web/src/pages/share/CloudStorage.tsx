// 云端暂存（分享面板 / 我的分享共用）：转换（保留期档位选择）、上传进度行、云端状态、删除云端副本。
// 上传在 agent 内进行；进度经全量 `shareChanged` 下发（约 2 次 / 秒），这里只读 `share.upload`。

import { CloudOff, CloudUpload, RotateCcw } from 'lucide-react'
import { useT } from '../../i18n'
import type { ShareDto } from '../../lib/rpc'
import { ActionMenu, Badge, Button, ProgressBar, Tooltip } from '../../ui'
import { formatBytes } from '../downloads/dialogs/utils'
import { convertEntries, useCloudActions } from './cloudActions'
import {
  CLOUD_STATE_KEYS,
  canDeleteCloudCopy,
  cloudConvertAvailability,
  cloudRetryTierId,
  reasonTextKey,
  retentionTiers,
  shareDisplayStatus,
  uploadPercent,
  uploadProgressText,
} from './model'
import { useShareSignedIn, useShareState } from './state'

/** 云端状态角标（上传中 / 云端就绪 / 已删除）；没有云端副本不渲染。 */
export function CloudStateBadge({ share }: { share: Pick<ShareDto, 'cloudState' | 'upload'> }) {
  const t = useT()
  const key = CLOUD_STATE_KEYS[share.cloudState]
  if (key === null) return null
  const text = share.cloudState === 'uploading' && share.upload ? `${t(key)} ${uploadPercent(share.upload)}%` : t(key)
  return <Badge tone={share.cloudState === 'ready' ? 'success' : share.cloudState === 'uploading' ? 'accent' : 'neutral'}>{text}</Badge>
}

/** 分享面板里的云端暂存区块：状态、上传进度行（暂停 / 失败带原因与重试）、转换 / 删除按钮。 */
export function CloudSection({ share, nowMs }: { share: ShareDto; nowMs: number }) {
  const t = useT()
  const { policySummary } = useShareState()
  const signedIn = useShareSignedIn()
  const { busy, convert, remove } = useCloudActions(share)
  const tiers = retentionTiers(policySummary)
  const availability = cloudConvertAvailability({ share, displayStatus: shareDisplayStatus(share, nowMs), policy: policySummary, signedIn })
  const upload = share.upload
  const reasonKey = upload?.status === 'failed' || upload?.status === 'paused' ? reasonTextKey(upload.reason) : null

  if (availability.kind === 'hidden' && share.cloudState === 'none') return null
  return (
    <div className="flex min-w-0 flex-col gap-2 rounded-md border border-hairline p-3">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="text-xs font-medium text-foreground">{t('shareCloudTitle')}</span>
        <CloudStateBadge share={share} />
        {share.cloudState === 'ready' && share.cloudBytes > 0 ? <span className="tabular text-xs text-text-tertiary">{formatBytes(share.cloudBytes)}</span> : null}
      </div>

      {upload ? (
        <div className="flex min-w-0 flex-col gap-1">
          <ProgressBar value={upload.totalBytes > 0 ? uploadPercent(upload) / 100 : null} />
          <div className="tabular break-words text-xs text-muted-foreground">{uploadProgressText(t, upload)}</div>
          {reasonKey !== null ? <div className="break-words text-xs text-destructive">{t(reasonKey)}</div> : null}
        </div>
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        {availability.kind === 'enabled' ? (
          <ActionMenu
            title={t('shareCloudConvert')}
            entries={convertEntries(t, tiers, convert)}
            trigger={
              <Button variant="outline" icon={CloudUpload} loading={busy} disabled={busy}>
                {t('shareCloudConvert')}
              </Button>
            }
          />
        ) : availability.kind === 'disabled' ? (
          <Tooltip content={t(availability.reasonKey)}>
            <span>
              <Button variant="outline" icon={CloudUpload} disabled>
                {t('shareCloudConvert')}
              </Button>
            </span>
          </Tooltip>
        ) : null}
        {upload?.status === 'failed' ? (
          <Button variant="outline" icon={RotateCcw} loading={busy} disabled={busy} onClick={() => void convert(cloudRetryTierId(share, tiers))}>
            {t('shareCloudRetry')}
          </Button>
        ) : null}
        {canDeleteCloudCopy(share) ? (
          <Button variant="ghost" icon={CloudOff} className="text-destructive" loading={busy} onClick={() => void remove()}>
            {t(share.cloudState === 'uploading' ? 'shareCloudCancelUpload' : 'shareCloudDelete')}
          </Button>
        ) : null}
      </div>
    </div>
  )
}
