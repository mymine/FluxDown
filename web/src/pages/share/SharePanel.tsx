// 分享面板（对应 GPUI 分享面板）：链接 + 「复制链接」主操作、6 位取件码 + 二维码、就地修改的 chip
// （有效期 / 领取次数 / 领取通知）、本机网络状态行、每个接收方的连接行（路径、速率、进度、诊断跳转）。
// 打开即为任务创建分享（同一批任务已有有效分享则复用，改设置不新建）；窄屏为底部抽屉。

import { useNavigate } from '@tanstack/react-router'
import { Copy, Share2, Square } from 'lucide-react'
import { useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../../i18n'
import { copyText } from '../../lib/copy'
import { rpc } from '../../lib/rpc'
import type { ShareDto, ShareReceiverDto, ShareUpdateParams } from '../../lib/rpc'
import { Badge, Button, FieldHint, Input, ProgressBar, Select, Spinner, Switch, confirmDialog, toast } from '../../ui'
import { accountErrorKey } from '../settings/sections/account/errorText'
import { formatBytes } from '../downloads/dialogs/utils'
import { useNowSeconds } from '../rss/useNow'
import {
  SHARE_STATUS_KEYS,
  claimOptions,
  dateTimeLabel,
  findExistingShare,
  formatPickupCode,
  netStatusLine,
  progressPercent,
  rateLabel,
  shareDisplayStatus,
  shareTitleText,
  swarmToggle,
  tierLabel,
} from './model'
import { CloudSection } from './CloudStorage'
import { QrImage } from './QrImage'
import { PathBadge } from './SessionDetail'
import { ShareSurface } from './ShareSurface'
import { useShareById, useShareState, useShares } from './state'
import type { SharePanelRequest } from './store'
import { requestDiagnostics } from './diagnosticsRequest'

/** 创建要先给文件算块哈希，大文件（NAS 机械盘上数十 GB）远超默认 30 秒超时。 */
const CREATE_TIMEOUT_MS = 30 * 60_000

/** 同一请求（nonce）只创建一次：StrictMode 会把 effect 跑两遍。 */
const creations = new Map<number, Promise<ShareDto>>()

function createOnce(request: SharePanelRequest): Promise<ShareDto> {
  const existing = creations.get(request.nonce)
  if (existing) return existing
  const created = rpc.agent.share.create({ taskIds: [...request.taskIds] }, { timeoutMs: CREATE_TIMEOUT_MS })
  creations.set(request.nonce, created)
  return created
}

/** 系统分享：只分享链接；浏览器不支持 Web Share 时复制链接。 */
async function systemShare(share: ShareDto, copiedText: string): Promise<void> {
  if (typeof navigator.share === 'function') {
    try {
      await navigator.share({ title: share.title, url: share.url })
      return
    } catch (error) {
      // 用户关掉系统分享面板不是错误；其余失败回退为复制。
      if (error instanceof DOMException && error.name === 'AbortError') return
    }
  }
  copyText(share.url)
  toast.info(copiedText)
}

export function SharePanel({ request, onClose }: { request: SharePanelRequest; onClose: () => void }) {
  const t = useT()
  const shares = useShares()
  const nowMs = useNowSeconds() * 1000
  const [created, setCreated] = useState<ShareDto | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)

  // 打开既有分享直接用；同一批任务已有有效分享时复用；否则创建。
  const adopted = request.shareId === null ? findExistingShare(shares, request.taskIds, nowMs) : undefined
  const shareId = request.shareId ?? adopted?.id ?? created?.id ?? null

  useEffect(() => {
    if (shareId !== null) return
    let cancelled = false
    setFailure(null)
    createOnce(request)
      .then((share) => {
        if (!cancelled) setCreated(share)
      })
      .catch((error: unknown) => {
        creations.delete(request.nonce)
        if (!cancelled) setFailure(t(accountErrorKey(error)))
      })
    return () => {
      cancelled = true
    }
  }, [shareId, request, attempt, t])

  // 创建回执早于 `shareChanged` 到达时，先用回执里的分享渲染。
  const share = useShareById(shareId) ?? (created !== null && created.id === shareId ? created : undefined)

  return (
    <ShareSurface open onOpenChange={(open) => !open && onClose()} title={t('sharePanelTitle')} size="lg">
      {share ? (
        <ShareBody share={share} nowMs={nowMs} onClose={onClose} />
      ) : failure !== null ? (
        <div className="flex flex-col items-start gap-3 py-4">
          <div role="alert" className="break-words text-sm text-destructive">
            {failure}
          </div>
          <Button variant="primary" onClick={() => setAttempt((value) => value + 1)}>
            {t('shareRetry')}
          </Button>
        </div>
      ) : (
        <div className="flex flex-col items-center gap-2 py-8 text-center">
          <div className="flex items-center gap-2 text-sm text-foreground">
            <Spinner size="md" />
            {t('shareCreating')}
          </div>
          <FieldHint>{t('shareHashing')}</FieldHint>
        </div>
      )}
    </ShareSurface>
  )
}

function Chip({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 items-center gap-2 rounded-md border border-hairline bg-chrome py-1 pr-1 pl-2.5" title={hint}>
      <span className="shrink-0 text-xs text-muted-foreground">{label}</span>
      {children}
    </div>
  )
}

function ShareBody({ share, nowMs, onClose }: { share: ShareDto; nowMs: number; onClose: () => void }) {
  const t = useT()
  const navigate = useNavigate()
  const { policySummary, netStatus } = useShareState()
  const status = shareDisplayStatus(share, nowMs)
  const live = status === 'active'
  const [pending, setPending] = useState<'tier' | 'claims' | 'notify' | 'swarm' | null>(null)
  const [stopping, setStopping] = useState(false)
  const expires = dateTimeLabel(share.expiresAt)
  const swarmView = swarmToggle(share, live, pending === 'swarm')

  const update = async (kind: 'tier' | 'claims' | 'notify' | 'swarm', patch: Omit<ShareUpdateParams, 'id'>) => {
    setPending(kind)
    try {
      await rpc.agent.share.update({ id: share.id, ...patch })
    } catch (error) {
      toast.error(t(accountErrorKey(error)))
    } finally {
      setPending(null)
    }
  }

  const stop = async () => {
    const ok = await confirmDialog({ title: t('shareStop'), description: t('shareStopConfirm'), okLabel: t('shareStop'), intent: 'destructive' })
    if (!ok) return
    setStopping(true)
    try {
      await rpc.agent.share.stop({ id: share.id })
      onClose()
    } catch (error) {
      toast.error(t(accountErrorKey(error)))
      setStopping(false)
    }
  }

  const tiers = policySummary.liveTiers
  const tierOptions = tiers.some((tier) => tier.id === share.tierId)
    ? tiers.map((tier) => ({ value: tier.id, label: tierLabel(t, tier) }))
    : [...tiers.map((tier) => ({ value: tier.id, label: tierLabel(t, tier) })), { value: share.tierId, label: share.tierId }]
  const claimChoices = claimOptions(share.maxClaims).map((count) => ({
    value: String(count),
    label: count === 0 ? t('shareClaimsUnlimited') : t('shareClaimsTimes', { count }),
  }))

  return (
    <>
      <div className="flex min-w-0 items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="break-words text-sm font-medium text-foreground" title={share.title}>
            {shareTitleText(t, share)}
          </div>
          <div className="tabular text-xs text-text-tertiary">
            {t('shareFilesCount', { count: share.files.length })} · {formatBytes(share.totalBytes)}
          </div>
        </div>
        <Badge tone={status === 'active' ? 'success' : 'neutral'}>{t(SHARE_STATUS_KEYS[status])}</Badge>
      </div>

      <div className="flex flex-col gap-1.5">
        <div className="text-xs font-medium text-foreground">{t('shareLinkLabel')}</div>
        <div className="flex min-w-0 gap-2">
          <Input
            readOnly
            value={share.url}
            aria-label={t('shareLinkLabel')}
            spellCheck={false}
            onFocus={(event) => event.currentTarget.select()}
            className="min-w-0 flex-1 font-mono text-xs"
          />
          <Button
            variant="primary"
            icon={Copy}
            disabled={!live}
            onClick={() => {
              copyText(share.url)
              toast.success(t('shareLinkCopied'))
            }}
          >
            {t('shareCopyLink')}
          </Button>
        </div>
      </div>

      <div className="flex items-center gap-4 rounded-md border border-hairline p-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="text-xs text-muted-foreground">{t('sharePickupCode')}</div>
          <div className="tabular text-3xl font-semibold tracking-[0.2em] text-foreground">{formatPickupCode(share.pickupCode) || '—'}</div>
          <div>
            <Button
              variant="ghost"
              icon={Copy}
              disabled={!live || share.pickupCode === ''}
              onClick={() => {
                copyText(share.pickupCode)
                toast.success(t('shareCodeCopied'))
              }}
            >
              {t('shareCopyCode')}
            </Button>
          </div>
        </div>
        {live ? <QrImage value={share.url} label={t('shareQrTitle')} /> : null}
      </div>

      <div className="flex flex-wrap gap-2">
        <Chip label={t('shareChipMode')} hint={t(share.mode === 'cloud' ? 'shareModeCloudHint' : 'shareModeLiveHint')}>
          <span className="pr-1.5 text-xs text-foreground">{t(share.mode === 'cloud' ? 'shareModeCloud' : 'shareModeLive')}</span>
        </Chip>
        <Chip label={t('shareChipExpiry')}>
          <Select
            value={share.tierId}
            options={tierOptions}
            disabled={!live || pending === 'tier'}
            aria-label={t('shareChipExpiry')}
            className="h-7 w-auto min-w-24 border-0 bg-transparent"
            onValueChange={(tierId) => void update('tier', { tierId })}
          />
        </Chip>
        <Chip label={t('shareChipClaims')}>
          <Select
            value={String(share.maxClaims)}
            options={claimChoices}
            disabled={!live || pending === 'claims'}
            aria-label={t('shareChipClaims')}
            className="h-7 w-auto min-w-24 border-0 bg-transparent"
            onValueChange={(value) => void update('claims', { maxClaims: Number(value) })}
          />
        </Chip>
        <Chip label={t('shareChipNotify')}>
          <span className="flex items-center gap-1.5 pr-1.5">
            <Switch
              checked={share.notifyOnClaim}
              disabled={!live || pending === 'notify'}
              aria-label={t('shareChipNotify')}
              onCheckedChange={(notifyOnClaim) => void update('notify', { notifyOnClaim })}
            />
            <span className="text-xs text-foreground">{t(share.notifyOnClaim ? 'shareNotifyOn' : 'shareNotifyOff')}</span>
          </span>
        </Chip>
        <Chip label={t('shareChipSwarm')}>
          <span className="flex items-center gap-1.5 pr-1.5">
            <Switch
              checked={swarmView.checked}
              disabled={swarmView.disabled}
              aria-label={t('shareChipSwarm')}
              onCheckedChange={(swarm) => void update('swarm', { swarm })}
            />
            <span className="text-xs text-foreground">{t(swarmView.labelKey)}</span>
          </span>
        </Chip>
      </div>

      <div className="tabular flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-text-tertiary">
        {expires === '' ? null : <span>{t('shareExpiresAt', { time: expires })}</span>}
        <span>
          {share.maxClaims > 0
            ? t('shareClaimedOf', { count: share.claimCount, max: share.maxClaims })
            : t('shareClaimedCount', { count: share.claimCount })}
        </span>
      </div>

      <CloudSection share={share} nowMs={nowMs} />

      <div className="flex min-w-0 flex-col gap-0.5 rounded-md bg-chrome px-3 py-2">
        <div className="text-xs font-medium text-foreground">{t('shareNetTitle')}</div>
        <div className="break-words text-xs text-muted-foreground">{netStatusLine(t, netStatus)}</div>
      </div>

      <div className="flex flex-col gap-1.5">
        <div className="text-xs font-medium text-foreground">
          {t('shareReceiversTitle')}
          {share.activeReceivers.length > 0 ? <span className="ml-1.5 font-normal text-muted-foreground">{t('shareActiveReceivers', { count: share.activeReceivers.length })}</span> : null}
        </div>
        {share.activeReceivers.length === 0 ? (
          <FieldHint>{t('shareReceiversEmpty')}</FieldHint>
        ) : (
          <ul className="flex flex-col gap-2">
            {share.activeReceivers.map((receiver) => (
              <ReceiverRow key={receiver.sessionId} receiver={receiver} onClose={onClose} />
            ))}
          </ul>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t border-hairline pt-3">
        <Button variant="outline" icon={Share2} disabled={!live} onClick={() => void systemShare(share, t('shareSystemShareCopied'))}>
          {t('shareSystemShare')}
        </Button>
        <Button
          variant="ghost"
          onClick={() => {
            onClose()
            void navigate({ to: '/share' })
          }}
        >
          {t('shareManageAll')}
        </Button>
        {live ? (
          <Button variant="ghost" icon={Square} className="ml-auto text-destructive" loading={stopping} onClick={() => void stop()}>
            {t('shareStop')}
          </Button>
        ) : null}
      </div>
    </>
  )
}

/** 接收方行：「诊断此会话」关闭面板并跳到设置 › FluxMesh › 网络诊断，聚焦该会话（对齐 GPUI `open_diagnostics`）。 */
export function ReceiverRow({ receiver, onClose }: { receiver: ShareReceiverDto; onClose: () => void }) {
  const t = useT()
  const navigate = useNavigate()
  const [killing, setKilling] = useState(false)
  const name = receiver.deviceName.trim()
  const percent = progressPercent(receiver.progress)

  const kill = async () => {
    setKilling(true)
    try {
      await rpc.agent.share.sessionKill({ sessionId: receiver.sessionId })
    } catch (error) {
      setKilling(false)
      toast.error(t(accountErrorKey(error)))
    }
  }

  return (
    <li className="flex min-w-0 flex-col gap-1.5 rounded-md border border-hairline p-2.5">
      <div className="flex min-w-0 items-center gap-2">
        <span className="min-w-0 flex-1 truncate text-sm text-foreground" title={name}>
          {name === '' ? t('shareReceiverGuest') : name}
        </span>
        <PathBadge kind={receiver.pathKind} />
      </div>
      <div className="flex min-w-0 items-center gap-2">
        <ProgressBar value={receiver.progress} className="flex-1" />
        <span className="tabular w-9 shrink-0 text-right text-xs text-muted-foreground">{percent}%</span>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="tabular text-xs text-text-tertiary">
          {rateLabel(receiver.rateBps)} · {formatBytes(receiver.bytes)}
        </span>
        <span className="ml-auto flex gap-1">
          <Button
            variant="ghost"
            className="h-7 px-2 text-xs"
            onClick={() => {
              requestDiagnostics(receiver.sessionId)
              onClose()
              void navigate({ to: '/settings/$category', params: { category: 'share' } })
            }}
          >
            {t('shareDiagnose')}
          </Button>
          <Button variant="ghost" className="h-7 px-2 text-xs text-destructive" loading={killing} onClick={() => void kill()}>
            {t('shareDisconnectReceiver')}
          </Button>
        </span>
      </div>
    </li>
  )
}
