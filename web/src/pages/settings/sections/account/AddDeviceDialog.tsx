// 添加设备（GPUI add-device 对话框）：账号方式（同账号登录即自动出现）/ 局域网直连配对
// （发现、手动地址含 https、配对码、SAS 确认、本机配对码展示）。

import { Copy, Laptop, RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { copyText } from '../../../../lib/copy'
import { rpc, useAgent } from '../../../../lib/rpc'
import type { LinkDeviceInfo, LinkDiscoveredPeer, LinkPairBeginResponse, LinkPairingCodeDto } from '../../../../lib/rpc'
import { Button, ConfirmFooter, Dialog, FieldError, FieldHint, Form, FormField, Icon, Input, SegmentedTabs, Spinner, Tooltip, toast } from '../../../../ui'
import { accountErrorKey } from './errorText'
import { isManualAddressValid, isPairingCodeComplete, localDeadlineMs, normalizePairingCode, peerAddress, renewDelayMs, secondsUntil, unpairedPeers } from './pairing'
import { useCountdown } from './useCountdown'

const NO_PEERS: readonly LinkDiscoveredPeer[] = []
const NO_LINKED: readonly LinkDeviceInfo[] = []

type Tab = 'account' | 'local'

function AccountTab() {
  const t = useT()
  const email = useAgent((snapshot) => snapshot.session?.user.email ?? null, null)
  return (
    <div className="flex flex-col gap-3">
      <FieldHint>{t('addDeviceHint')}</FieldHint>
      {email ? (
        <div className="rounded-md bg-muted px-3 py-2 text-sm text-foreground">{t('addDeviceAccountSynced', { account: email })}</div>
      ) : (
        <FieldError>{t('addDeviceLoginRequired')}</FieldError>
      )}
      <FieldHint>{t('addDeviceAccountFooter')}</FieldHint>
    </div>
  )
}

const NO_REQUESTS: readonly { sessionId: string }[] = []
/** 发现开启 / 重新搜索后的「搜索中」窗口。 */
const SEARCH_WINDOW_MS = 8000

/**
 * 本机配对码：供对端输入；Web 场景的「本机」即服务器。以 lanActive（本次运行实际可达）为准：
 * 可达 → 自动生成、到期自动续码、入站请求出现后换新码；不可达时才按 lanEnabled 区分「未开启 /
 * 已开启未生效」只提示。server 模式监听地址只由 FLUXDOWN_BIND 决定、lanActive 恒为 true，
 * 持久化的 lanEnabled 与之无关（同 GPUI `render_own_code`）。卸载时撤回广播。
 */
function OwnCodePanel() {
  const t = useT()
  const lanEnabled = useAgent((snapshot) => snapshot.gateway.lanEnabled, false)
  const lanActive = useAgent((snapshot) => snapshot.gateway.lanActive, false)
  const requests = useAgent((snapshot) => snapshot.linkPairingRequests ?? NO_REQUESTS, NO_REQUESTS)
  const [own, setOwn] = useState<LinkPairingCodeDto | null>(null)
  const [busy, setBusy] = useState(false)
  const [errorKey, setErrorKey] = useState<string | null>(null)
  const active = useRef(false)
  const codeRef = useRef<string | undefined>(undefined)
  const alive = useRef(true)
  const expiry = useCountdown()

  const renew = useCallback(async () => {
    setBusy(true)
    setErrorKey(null)
    try {
      const result = await rpc.agent.link.pairingCode()
      if (!alive.current) {
        void rpc.agent.link.stopPairing({ code: result.code }).catch(() => undefined)
        return
      }
      active.current = true
      codeRef.current = result.code
      setOwn(result)
      const next = localDeadlineMs(result, Date.now())
      setDeadline(next)
      expiry.start(secondsUntil(next, Date.now()))
    } catch (error) {
      if (alive.current) setErrorKey(accountErrorKey(error))
    } finally {
      if (alive.current) setBusy(false)
    }
  }, [expiry.start])

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
      if (active.current) void rpc.agent.link.stopPairing({ code: codeRef.current }).catch(() => undefined)
    }
  }, [])

  // lanActive 才生成码；之后到期自动续码。
  const [deadline, setDeadline] = useState(0)
  useEffect(() => {
    if (!lanActive) return
    if (own === null) {
      void renew()
      return
    }
    const timer = window.setTimeout(() => void renew(), renewDelayMs(deadline, Date.now()))
    return () => window.clearTimeout(timer)
  }, [lanActive, own === null, deadline, renew])

  // 本机码被对端使用（出现新的入站请求）后换新码。
  const seen = useRef<Set<string> | null>(null)
  useEffect(() => {
    const ids = new Set(requests.map((request) => request.sessionId))
    const previous = seen.current
    seen.current = ids
    if (!lanActive || previous === null) return
    if ([...ids].some((id) => !previous.has(id))) void renew()
  }, [requests, lanActive, renew])

  const gateHint = lanActive ? null : lanEnabled ? 'localPairingLanRestartHint' : 'localPairingLanDisabledHint'

  return (
    <div className="flex flex-col gap-2 rounded-md border border-hairline p-3">
      <div className="flex items-center justify-between gap-2">
        <div className="text-sm font-medium text-foreground">{t('localPairingMyCodeTitle')}</div>
        {gateHint === null ? (
          <Button loading={busy} onClick={() => void renew()}>
            {t('localPairingMyCodeRefresh')}
          </Button>
        ) : null}
      </div>
      {gateHint !== null ? (
        <FieldHint>{t(gateHint)}</FieldHint>
      ) : (
        <>
          <FieldHint>{t('localPairingMyCodeHint')}</FieldHint>
          {own ? (
            <>
              <div className="flex items-baseline gap-3">
                <span className="tabular font-mono text-title font-semibold tracking-widest text-foreground">{own.code}</span>
                <span className="tabular text-xs text-muted-foreground">{expiry.remaining > 0 ? t('accountCodeExpireIn', { seconds: expiry.remaining }) : t('localPairingMyCodeExpired')}</span>
                <Tooltip content={t('accountReferralCodeCopy')}>
                  <Button
                    variant="ghost"
                    iconOnly
                    aria-label={t('accountReferralCodeCopy')}
                    onClick={() => {
                      copyText(own.code)
                      toast.key('accountReferralCodeCopied', 'success')
                    }}
                  >
                    <Icon icon={Copy} />
                  </Button>
                </Tooltip>
              </div>
              <FieldHint>{t('linkControlPairCodeHint')}</FieldHint>
              {own.addresses.length > 0 ? (
                <div className="flex flex-col gap-0.5 text-xs text-muted-foreground">
                  <span>{t('localPairingMyAddresses')}</span>
                  {own.addresses.map((address) => (
                    <span key={address} className="break-all font-mono text-foreground">
                      {address}
                    </span>
                  ))}
                </div>
              ) : (
                <FieldError>{t('localPairingNoAddress')}</FieldError>
              )}
            </>
          ) : null}
        </>
      )}
      {errorKey ? <FieldError>{t(errorKey)}</FieldError> : null}
    </div>
  )
}

function PeerList({ peers, selected, searching, onSelect }: { peers: readonly LinkDiscoveredPeer[]; selected: string; searching: boolean; onSelect: (peer: LinkDiscoveredPeer) => void }) {
  const t = useT()
  if (peers.length === 0) {
    return searching ? (
      <span className="flex items-center gap-2 text-xs text-muted-foreground">
        <Spinner />
        {t('localPairingDiscovering')}
      </span>
    ) : (
      <FieldHint>{t('localPairingNoDevices')}</FieldHint>
    )
  }
  return (
    <div className="flex flex-col gap-1">
      <div className="text-xs font-medium text-muted-foreground">{t('localPairingNearbyTitle')}</div>
      <div className="flex flex-col overflow-hidden rounded-md border border-hairline [&>*+*]:border-t [&>*+*]:border-hairline">
        {peers.map((peer) => {
          const address = peerAddress(peer)
          return (
            <button
              key={`${peer.fingerprint ?? ''}@${address}`}
              type="button"
              onClick={() => onSelect(peer)}
              aria-pressed={selected === address}
              className={`flex items-center gap-2 px-3 py-2 text-left text-sm coarse:min-h-touch hover:bg-nav-hover ${selected === address ? 'bg-nav-selected' : ''}`}
            >
              <Icon icon={Laptop} size="md" className="text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate text-foreground">{peer.name}</span>
              <span className="truncate text-xs text-muted-foreground">{address}</span>
            </button>
          )
        })}
      </div>
    </div>
  )
}

/** 直连配对页签。挂载 = 开始发现 / 广播本机码；卸载（切走或关闭对话框）= 停止并放弃进行中的流程。 */
function LocalTab({ onDone }: { onDone: () => void }) {
  const t = useT()
  const discovered = useAgent((snapshot) => snapshot.linkDiscovered ?? NO_PEERS, NO_PEERS)
  const linked = useAgent((snapshot) => snapshot.linkedDevices, NO_LINKED)
  const lanActive = useAgent((snapshot) => snapshot.gateway.lanActive, false)
  const peers = unpairedPeers(discovered, linked)
  const [address, setAddress] = useState('')
  const [selected, setSelected] = useState('')
  const [manual, setManual] = useState(false)
  const [code, setCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [errorKey, setErrorKey] = useState<string | null>(null)
  const [pending, setPending] = useState<LinkPairBeginResponse | null>(null)
  const [finishing, setFinishing] = useState(false)
  const [scanning, setScanning] = useState(false)
  const [searching, setSearching] = useState(false)
  const searchTimer = useRef<number | undefined>(undefined)
  const alive = useRef(true)
  // 每次发起 / 取消 / 卸载递增，使迟到的 pairBegin / pairFinish 响应失效。
  const flow = useRef(0)

  const startSearchWindow = useCallback(() => {
    window.clearTimeout(searchTimer.current)
    setSearching(true)
    searchTimer.current = window.setTimeout(() => setSearching(false), SEARCH_WINDOW_MS)
  }, [])

  // 重新搜索：串行 stop → start。
  const scan = async () => {
    if (scanning) return
    setScanning(true)
    try {
      await rpc.agent.link.discoverySet({ enabled: false })
      await rpc.agent.link.discoverySet({ enabled: true })
      startSearchWindow()
    } catch (error) {
      setErrorKey(accountErrorKey(error))
    } finally {
      setScanning(false)
    }
  }

  // 打开即开始发现；离开（含关闭对话框）即停止。
  useEffect(() => {
    startSearchWindow()
    void rpc.agent.link.discoverySet({ enabled: true }).catch((error: unknown) => setErrorKey(accountErrorKey(error)))
    return () => {
      window.clearTimeout(searchTimer.current)
      void rpc.agent.link.discoverySet({ enabled: false }).catch(() => undefined)
    }
  }, [startSearchWindow])

  // 放弃 SAS 确认（关闭对话框 / 切走）时通知对端，避免悬挂会话。
  const pendingRef = useRef<LinkPairBeginResponse | null>(null)
  const setPendingBoth = (value: LinkPairBeginResponse | null) => {
    pendingRef.current = value
    setPending(value)
  }
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
      flow.current += 1
      const token = pendingRef.current?.token
      if (token) void rpc.agent.link.pairFinish({ token, accept: false }).catch(() => undefined)
    }
  }, [])

  const canBegin = isManualAddressValid(address) && isPairingCodeComplete(code) && !busy

  const begin = async () => {
    if (!canBegin) return
    const mine = ++flow.current
    setBusy(true)
    setErrorKey(null)
    try {
      const response = await rpc.agent.link.pairBegin({ address: address.trim(), code })
      if (!alive.current || flow.current !== mine) {
        // 迟到的成功响应：对话框已关闭 / 页签已切走，立即释放会话。
        void rpc.agent.link.pairFinish({ token: response.token, accept: false }).catch(() => undefined)
        return
      }
      setPendingBoth(response)
    } catch (error) {
      if (alive.current && flow.current === mine) setErrorKey(accountErrorKey(error, 'pairing'))
    } finally {
      if (alive.current && flow.current === mine) setBusy(false)
    }
  }

  const finish = async (accept: boolean) => {
    if (!pending) return
    const current = pending
    const mine = ++flow.current
    setBusy(true)
    setFinishing(accept)
    setErrorKey(null)
    try {
      if (!accept) pendingRef.current = null
      const result = await rpc.agent.link.pairFinish({ token: current.token, accept })
      if (!alive.current || flow.current !== mine) return
      pendingRef.current = null
      if (accept && result.paired) {
        toast.key('localPairingPaired', 'success', { device: result.device?.name ?? current.peerName })
        onDone()
        return
      }
      setPendingBoth(null)
      if (accept) setErrorKey('localPairingFailed')
    } catch (error) {
      if (!alive.current || flow.current !== mine) return
      setPendingBoth(null)
      setErrorKey(accountErrorKey(error, 'pairing'))
    } finally {
      if (alive.current && flow.current === mine) {
        setBusy(false)
        setFinishing(false)
      }
    }
  }

  // Finishing（等待对方确认）取消：放弃等待回到表单，并尽力释放会话。
  const cancelFinishing = () => {
    const token = pending?.token
    flow.current += 1
    pendingRef.current = null
    setPendingBoth(null)
    setBusy(false)
    setFinishing(false)
    if (token) void rpc.agent.link.pairFinish({ token, accept: false }).catch(() => undefined)
  }

  const sasView = pending ? (
    <div className="flex flex-col gap-3">
      <div className="text-sm font-medium text-foreground">{t('localPairingSasTitle')}</div>
      <FieldHint>{t('localPairingSasHint')}</FieldHint>
      <div className="flex flex-col items-center gap-1 rounded-md bg-muted py-4">
        <span className="tabular font-mono text-display font-semibold tracking-widest text-foreground">{pending.sas}</span>
        <span className="text-xs text-muted-foreground">{pending.peerName}</span>
      </div>
      {finishing ? <FieldHint>{t('localPairingWaitingPeer')}</FieldHint> : null}
      {errorKey ? <FieldError>{t(errorKey)}</FieldError> : null}
      <div className="flex justify-end gap-2">
        {finishing ? (
          <Button variant="outline" onClick={cancelFinishing}>
            {t('cancel')}
          </Button>
        ) : (
          <>
            <Button variant="outline" disabled={busy} onClick={() => void finish(false)}>
              {t('localPairingReject')}
            </Button>
            <Button variant="primary" loading={busy} onClick={() => void finish(true)}>
              {t('localPairingConfirm')}
            </Button>
          </>
        )}
      </div>
    </div>
  ) : null

  const formView = pending ? null : (
    <Form onSubmit={() => void begin()}>
      {!lanActive ? <FieldHint>{t('localPairingOneWayHint')}</FieldHint> : null}
      <FieldHint>{t('localPairingHint')}</FieldHint>
      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" icon={RefreshCw} disabled={scanning} onClick={() => void scan()}>
          {t('localPairingRetryScan')}
        </Button>
      </div>
      <PeerList
        peers={peers}
        selected={selected}
        searching={searching || scanning}
        onSelect={(peer) => {
          const next = peerAddress(peer)
          setAddress(next)
          setSelected(next)
          setManual(false)
        }}
      />
      <button type="button" className="self-start text-xs text-accent-text coarse:min-h-touch" onClick={() => setManual((value) => !value)}>
        {t('localPairingManualAddress')}
      </button>
      {manual ? (
        <FormField label={t('localPairingAddressLabel')} htmlFor="pair-address" hint={t('localPairingAddressHint')}>
          <Input
            id="pair-address"
            value={address}
            spellCheck={false}
            autoCapitalize="none"
            placeholder="192.168.1.10:17800 / https://host"
            onChange={(event) => {
              setAddress(event.target.value)
              setSelected('')
            }}
          />
        </FormField>
      ) : null}
      <FormField label={t('localPairingCodeLabel')} htmlFor="pair-code" hint={t('localPairingCodeHint')}>
        <Input id="pair-code" value={code} inputMode="numeric" autoComplete="one-time-code" placeholder={t('localPairingCodePlaceholder')} onChange={(event) => setCode(normalizePairingCode(event.target.value))} />
      </FormField>
      {errorKey ? <FieldError>{t(errorKey)}</FieldError> : null}
      <div className="flex justify-end">
        <Button variant="primary" disabled={!canBegin} loading={busy} onClick={() => void begin()}>
          {t('localPairingConnect')}
        </Button>
      </div>
      <button type="submit" className="hidden" />
    </Form>
  )

  // 本机配对码面板始终挂载在固定位置：核对 SAS 期间不卸载（卸载会 stopPairing 作废本机码）。
  return (
    <div className="flex flex-col gap-3">
      {sasView}
      {formView}
      <OwnCodePanel />
    </div>
  )
}

export function AddDeviceDialog({ onClose }: { onClose: () => void }) {
  const t = useT()
  const loggedIn = useAgent((snapshot) => snapshot.session !== null && snapshot.session !== undefined, false)
  // 未登录默认直连页签。
  const [tab, setTab] = useState<Tab>(loggedIn ? 'account' : 'local')
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={t('addDeviceEntry')}
      size="lg"
      footer={<ConfirmFooter okLabel={t('close')} cancelLabel={null} onCancel={onClose} onOk={onClose} />}
    >
      <div className="flex flex-col gap-3 pb-2">
        <SegmentedTabs
          value={tab}
          onValueChange={setTab}
          aria-label={t('addDeviceEntry')}
          items={[
            { value: 'account', label: t('addDeviceTabAccount') },
            { value: 'local', label: t('addDeviceTabLocal') },
          ]}
        />
        {tab === 'account' ? <AccountTab /> : <LocalTab onDone={onClose} />}
      </div>
    </Dialog>
  )
}
