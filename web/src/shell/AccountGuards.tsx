// 全局账号 / 互联守卫：入站局域网配对请求确认 + 会话被撤销的一次性提示。挂在 AppShell，任何页面都生效。

import { useCallback, useEffect, useRef, useState } from 'react'
import { useT } from '../i18n'
import { rpc, useAgent, useServiceEvents } from '../lib/rpc'
import type { EventFrame, LinkDeviceInfo, LinkPairingRequestDto } from '../lib/rpc'
import { accountErrorKey } from '../pages/settings/sections/account/errorText'
import { sessionRevokedKey } from '../pages/settings/sections/account/sessionNotice'
import { Button, Dialog, DialogFooter, FieldHint, toast } from '../ui'
import { localDeadlineMs, secondsUntil } from '../pages/settings/sections/account/pairing'

const NO_REQUESTS: readonly LinkPairingRequestDto[] = []
const NO_LINKED: readonly LinkDeviceInfo[] = []

function useSecondsLeft(deadlineMs: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    setNow(Date.now())
    const id = setInterval(() => setNow(Date.now()), 500)
    return () => clearInterval(id)
  }, [deadlineMs])
  return secondsUntil(deadlineMs, now)
}

function RequestDialog({ request, deadlineMs, onApproved }: { request: LinkPairingRequestDto; deadlineMs: number; onApproved: (request: LinkPairingRequestDto) => void }) {
  const t = useT()
  const [busy, setBusy] = useState(false)
  const seconds = useSecondsLeft(deadlineMs)
  const decided = useRef(false)

  const decide = useCallback(
    async (accept: boolean) => {
      if (decided.current) return
      decided.current = true
      setBusy(true)
      try {
        await rpc.agent.link.approve({ sessionId: request.sessionId, accept })
        // 批准后不提前提示：由宿主等 linkedDevices 出现该指纹再报「已配对」。
        if (accept) onApproved(request)
      } catch (error) {
        decided.current = false
        toast.key(accountErrorKey(error), 'error')
      } finally {
        setBusy(false)
      }
    },
    [request, onApproved],
  )

  // 到期自动拒绝。
  useEffect(() => {
    if (seconds === 0) void decide(false)
  }, [seconds, decide])

  return (
    <Dialog
      open
      // Esc / 遮罩不误拒：只有显式按钮才决定。
      onOpenChange={() => undefined}
      modalLocked
      title={t('localPairingIncomingTitle')}
      description={t('localPairingIncomingDesc', { device: request.peerName })}
      footer={
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => void decide(false)}>
            {t('localPairingReject')}
          </Button>
          <Button variant="primary" loading={busy} onClick={() => void decide(true)}>
            {request.requestsControl ? t('linkControlApprove') : t('localPairingConfirm')}
          </Button>
        </DialogFooter>
      }
    >
      <div className="flex flex-col gap-3 pb-2">
        {request.requestsControl ? (
          <div className="flex flex-col gap-1 rounded-md border border-destructive/40 bg-destructive/10 p-3">
            <span className="text-sm font-semibold text-destructive">{t('linkControlRequestTag')}</span>
            <span className="text-xs text-foreground">{t('linkControlRequestWarning')}</span>
          </div>
        ) : null}
        <FieldHint>{t('localPairingSasHint')}</FieldHint>
        <div className="flex flex-col items-center gap-1 rounded-md bg-muted py-4">
          <span className="tabular font-mono text-display font-semibold tracking-widest text-foreground">{request.sas}</span>
          <span className="text-xs text-muted-foreground">{[request.peerName, request.peerPlatform].filter(Boolean).join(' · ')}</span>
        </div>
        <FieldHint>{t('accountCodeExpireIn', { seconds })}</FieldHint>
      </div>
    </Dialog>
  )
}

/** 批准后等待 `linkedDevicesChanged` 出现该指纹的最长时间。 */
const APPROVE_CONFIRM_WINDOW_MS = 30_000

/** 入站配对请求：一次只处理队首，处理后自动轮到下一条；批准的请求等设备名册出现该指纹再提示已配对。 */
function PairingRequestHost() {
  const requests = useAgent((snapshot) => snapshot.linkPairingRequests ?? NO_REQUESTS, NO_REQUESTS)
  const linked = useAgent((snapshot) => snapshot.linkedDevices, NO_LINKED)
  const awaiting = useRef(new Map<string, { name: string; deadline: number; basePairedAt: number | null }>())
  const linkedRef = useRef(linked)
  linkedRef.current = linked
  const first = requests[0]

  // 截止时刻 = 本机首次看到该请求的时刻 + ttlMs（免疫主机时钟偏差）；旧主机无 ttlMs 才用绝对时间。
  const deadlines = useRef(new Map<string, number>())
  const live = new Set(requests.map((request) => request.sessionId))
  for (const id of deadlines.current.keys()) if (!live.has(id)) deadlines.current.delete(id)
  for (const request of requests) {
    if (!deadlines.current.has(request.sessionId)) deadlines.current.set(request.sessionId, localDeadlineMs(request, Date.now()))
  }

  // 记录批准时该指纹的 pairedAt 基线：已存在（重新配对）时 pairedAt 变化也算成功，仅新出现不够。
  const onApproved = useCallback((request: LinkPairingRequestDto) => {
    const existing = linkedRef.current.find((device) => device.fingerprint === request.peerFingerprint && !device.revoked)
    awaiting.current.set(request.peerFingerprint, {
      name: request.peerName,
      deadline: Date.now() + APPROVE_CONFIRM_WINDOW_MS,
      basePairedAt: existing ? existing.pairedAt : null,
    })
  }, [])

  useEffect(() => {
    for (const [fingerprint, entry] of awaiting.current) {
      const device = linked.find((item) => item.fingerprint === fingerprint && !item.revoked)
      if (device && (entry.basePairedAt === null || device.pairedAt !== entry.basePairedAt)) {
        awaiting.current.delete(fingerprint)
        toast.key('localPairingPaired', 'success', { device: entry.name })
      } else if (entry.deadline < Date.now()) {
        awaiting.current.delete(fingerprint)
      }
    }
  }, [linked])

  return first ? <RequestDialog key={first.sessionId} request={first} deadlineMs={deadlines.current.get(first.sessionId) ?? first.expiresAtUnixMs} onApproved={onApproved} /> : null
}

/** agent 推送的会话撤销通知（主动登出 / 删本设备不会发）：按 reason 一次性提示。 */
function SessionRevokedNotice() {
  useServiceEvents(
    useCallback((frame: EventFrame) => {
      const { event } = frame
      if (event.service === 'agent' && event.event.type === 'sessionRevoked') {
        toast.key(sessionRevokedKey(event.event.data), 'warning')
      }
    }, []),
  )
  return null
}

/** BT 重复添加：占位任务已被引擎删除，提示用户已有任务（协议要求宿主提示）。 */
function DuplicateTorrentNotice() {
  useServiceEvents(
    useCallback((frame: EventFrame) => {
      const { event } = frame
      if (event.service !== 'daemon' || event.event.type !== 'engine') return
      const message = event.event.data
      if (message.type !== 'duplicateTorrent') return
      if (message.existingName === '') toast.key('duplicateTorrentToastUnnamed', 'warning')
      else toast.key('duplicateTorrentToast', 'warning', { name: message.existingName })
    }, []),
  )
  return null
}

export function AccountGuards() {
  return (
    <>
      <PairingRequestHost />
      <SessionRevokedNotice />
      <DuplicateTorrentNotice />
    </>
  )
}
