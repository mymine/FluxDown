// 连接 / 编辑云端渠道对话框。
// Telegram 新建走「绑定」：二维码 + 深链，2s 轮询绑定状态，成功自动关闭；邮件填名称 + 收件邮箱 + 事件。

import { Copy, ExternalLink, Loader2, Plus, X } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { useT } from '../../i18n'
import { copyText } from '../../lib/copy'
import { rpc } from '../../lib/rpc'
import type { CloudNotifyChannelDto, CloudNotifyOverviewDto, CloudNotifyTelegramBindDto } from '../../lib/rpc'
import { Badge, Button, CheckRow, ConfirmFooter, Dialog, DialogFooter, FormField, Icon, Input, InputWithAction, toast } from '../../ui'
import { encodeQrChallengeImage } from '../settings/sections/extensions/challengeImage'
import {
  DEFAULT_CLOUD_EVENTS,
  EMAIL_MAX_ADDRESSES,
  addEmail,
  canAddMoreEmails,
  canSaveChannel,
  canSendEmailCode,
  canVerifyEmail,
  initialEmailList,
  isAccountEmail,
  isReaddingAccount,
  kindMeta,
  newEmailStatus,
  removeEmail,
} from './cloudLogic'
import { useCountdown } from '../settings/sections/account/useCountdown'
import { CLOUD_EVENTS } from './endpoints'
import { cloudActionError } from './useCloudNotify'

export interface DialogTarget {
  kind: string
  /** null = 新建。 */
  channel: CloudNotifyChannelDto | null
}

const POLL_MS = 2000

type BindPhase = 'starting' | 'pending' | 'expired'

/** Telegram 绑定面板：生成绑定码 → 二维码 / 深链 → 轮询。 */
function TelegramBind({ onBound }: { onBound: () => void }) {
  const t = useT()
  const [bind, setBind] = useState<CloudNotifyTelegramBindDto | null>(null)
  const [phase, setPhase] = useState<BindPhase>('starting')
  const [qr, setQr] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const start = useCallback(async () => {
    setPhase('starting')
    setError(null)
    setQr(null)
    try {
      const next = await rpc.agent.cloudNotify.telegramBindStart()
      setBind(next)
      setPhase('pending')
      setQr(await encodeQrChallengeImage(next.deepLink))
    } catch (cause) {
      setError(cloudActionError(cause, t, 0))
      setPhase('expired')
    }
  }, [t])

  useEffect(() => {
    void start()
  }, [start])

  useEffect(() => {
    if (bind === null || phase !== 'pending') return
    let alive = true
    let inflight = false
    const timer = setInterval(() => {
      if (inflight) return
      if (Date.parse(bind.expiresAt) <= Date.now()) {
        setPhase('expired')
        return
      }
      inflight = true
      rpc.agent.cloudNotify
        .telegramBindStatus({ code: bind.code })
        .then((result) => {
          if (!alive) return
          if (result.status === 'bound') {
            rpc.agent.cloudNotify.refresh().catch(() => undefined)
            onBound()
          } else if (result.status === 'expired') {
            setPhase('expired')
          }
        })
        .catch(() => undefined)
        .finally(() => {
          inflight = false
        })
    }, POLL_MS)
    return () => {
      alive = false
      clearInterval(timer)
    }
  }, [bind, phase, onBound])

  const deepLink = bind?.deepLink ?? ''

  return (
    <div className="flex w-full flex-col items-center gap-3 py-2">
      <p className="text-center text-xs text-muted-foreground">{t('cloudNotifyTelegramStep')}</p>
      <div className="flex size-48 items-center justify-center overflow-hidden rounded-md border border-hairline bg-white">
        {phase === 'starting' ? (
          <Icon icon={Loader2} size="xl" className="animate-spin text-muted-foreground" />
        ) : qr && phase === 'pending' ? (
          <img src={qr} alt={t('cloudNotifyKindTelegram')} className="size-full object-contain" />
        ) : (
          <span className="px-3 text-center text-xs text-muted-foreground">{error ?? t('cloudNotifyTelegramExpired')}</span>
        )}
      </div>
      {phase === 'pending' && deepLink !== '' ? (
        <>
          <div className="w-full max-w-sm">
            <Input
              readOnly
              value={deepLink}
              aria-label={t('cloudNotifyTelegramOpen')}
              onFocus={(event) => event.currentTarget.select()}
              trailing={
                <Button
                  variant="ghost"
                  iconOnly
                  aria-label={t('webhookCopied')}
                  title={t('webhookCopied')}
                  onClick={() => {
                    copyText(deepLink)
                    toast.key('webhookCopied', 'success')
                  }}
                >
                  <Icon icon={Copy} size="md" />
                </Button>
              }
            />
          </div>
          <Button variant="primary" icon={ExternalLink} onClick={() => window.open(deepLink, '_blank', 'noopener,noreferrer')}>
            {t('cloudNotifyTelegramOpen')}
          </Button>
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Icon icon={Loader2} size="sm" className="animate-spin" />
            {t('cloudNotifyTelegramWaiting')}
          </div>
        </>
      ) : null}
      {phase === 'expired' ? (
        <Button variant="outline" onClick={() => void start()}>
          {t('cloudNotifyTelegramRetry')}
        </Button>
      ) : null}
    </div>
  )
}

export function CloudChannelDialog({
  target,
  overview,
  onClose,
}: {
  target: DialogTarget
  overview: CloudNotifyOverviewDto
  onClose: () => void
}) {
  const t = useT()
  const { kind, channel } = target
  const meta = kindMeta(kind)
  const kindName = meta ? t(meta.nameKey) : kind
  const editing = channel !== null
  const telegramBind = kind === 'telegram' && !editing

  const [name, setName] = useState(channel?.name ?? kindName)
  const [events, setEvents] = useState<readonly string[]>(channel ? channel.events : DEFAULT_CLOUD_EVENTS)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [emailList, setEmailList] = useState<string[]>(() => initialEmailList(channel, overview.accountEmail))
  const [adding, setAdding] = useState(false)
  const [newAddress, setNewAddress] = useState('')
  const [newCode, setNewCode] = useState('')
  const [addressTouched, setAddressTouched] = useState(false)
  const [sending, setSending] = useState(false)
  const [verifying, setVerifying] = useState(false)
  const [formError, setFormError] = useState<string | null>(null)
  const [sentInfo, setSentInfo] = useState<{ email: string; minutes: number } | null>(null)
  const cooldown = useCountdown()

  const status = newEmailStatus(newAddress, emailList)
  const readdAccount = isReaddingAccount(newAddress, emailList, overview.accountEmail)
  const addressError =
    addressTouched && status === 'invalid'
      ? t('cloudNotifyEmailAddressInvalid')
      : status === 'exists'
        ? t('cloudNotifyEmailExists')
        : undefined
  const canSave = canSaveChannel(name, events) && (kind !== 'email' || emailList.length > 0) && !saving

  const closeAddForm = () => {
    setAdding(false)
    setNewAddress('')
    setNewCode('')
    setAddressTouched(false)
    setFormError(null)
    setSentInfo(null)
    cooldown.start(0)
  }

  const addAccountBack = () => {
    setEmailList((current) => addEmail(current, newAddress, overview.accountEmail))
    closeAddForm()
  }

  const sendCode = async () => {
    if (!canSendEmailCode(newAddress, emailList, overview.accountEmail) || sending) return
    setSending(true)
    setFormError(null)
    try {
      const result = await rpc.agent.cloudNotify.sendEmailCode({ address: newAddress.trim() })
      cooldown.start(result.resendAfterSecs)
      setSentInfo({ email: newAddress.trim(), minutes: Math.max(1, Math.ceil(result.expiresInSecs / 60)) })
    } catch (cause) {
      setFormError(cloudActionError(cause, t, overview.maxChannels))
    } finally {
      setSending(false)
    }
  }

  const verify = async () => {
    if (!canVerifyEmail(newAddress, newCode, emailList, overview.accountEmail) || verifying) return
    setVerifying(true)
    setFormError(null)
    try {
      await rpc.agent.cloudNotify.verifyEmail({ address: newAddress.trim(), code: newCode.trim() })
      setEmailList((current) => addEmail(current, newAddress, overview.accountEmail))
      closeAddForm()
    } catch (cause) {
      setFormError(cloudActionError(cause, t, overview.maxChannels))
    } finally {
      setVerifying(false)
    }
  }

  const save = async () => {
    if (!canSave) return
    setSaving(true)
    setError(null)
    try {
      const addresses = kind === 'email' ? { addresses: emailList } : {}
      if (channel) {
        await rpc.agent.cloudNotify.updateChannel({
          id: channel.id,
          name: name.trim(),
          events: [...events],
          ...addresses,
        })
      } else {
        await rpc.agent.cloudNotify.createChannel({
          kind,
          name: name.trim(),
          events: [...events],
          ...addresses,
        })
      }
      onClose()
    } catch (cause) {
      setError(cloudActionError(cause, t, overview.maxChannels))
      setSaving(false)
    }
  }

  const title = editing ? t('cloudNotifyEditTitle', { kind: kindName }) : t('cloudNotifyConnectTitle', { kind: kindName })

  if (telegramBind) {
    return (
      <Dialog
        open
        onOpenChange={(open) => !open && onClose()}
        size="sm"
        title={title}
        footer={
          <DialogFooter>
            <Button variant="outline" onClick={onClose}>
              {t('cancel')}
            </Button>
          </DialogFooter>
        }
      >
        <TelegramBind
          onBound={() => {
            toast.key('cloudNotifyStatusConnected', 'success')
            onClose()
          }}
        />
      </Dialog>
    )
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && !saving && onClose()}
      size="md"
      title={title}
      modalLocked={saving}
      footer={
        <ConfirmFooter
          okLabel={t('cloudNotifySave')}
          onCancel={onClose}
          onOk={() => void save()}
          okDisabled={!canSave}
          loading={saving}
        />
      }
    >
      <div className="flex flex-col gap-4 pb-2">
        <FormField label={t('cloudNotifyChannelName')} htmlFor="cn-name">
          <Input id="cn-name" value={name} maxLength={64} onChange={(event) => setName(event.target.value)} />
        </FormField>

        {kind === 'email' ? (
          <FormField
            label={t('cloudNotifyEmailRecipients', { n: emailList.length, max: EMAIL_MAX_ADDRESSES })}
            hint={adding ? undefined : t('cloudNotifyEmailVerifyHint')}
          >
            <div className="flex flex-col gap-2">
              <div className="flex flex-col overflow-hidden rounded-md border border-hairline [&>*+*]:border-t [&>*+*]:border-hairline">
                {emailList.map((address) => (
                  <div key={address} className="flex min-h-control items-center gap-2 px-2.5 py-1 coarse:min-h-touch">
                    <span className="min-w-0 flex-1 truncate text-sm text-foreground" title={address}>
                      {address}
                    </span>
                    <Badge tone={isAccountEmail(address, overview.accountEmail) ? 'accent' : 'success'}>
                      {isAccountEmail(address, overview.accountEmail) ? t('cloudNotifyEmailUseAccount') : t('cloudNotifyEmailVerified')}
                    </Badge>
                    <Button
                      variant="ghost"
                      iconOnly
                      icon={X}
                      aria-label={t('cloudNotifyEmailRemove')}
                      title={t('cloudNotifyEmailRemove')}
                      disabled={emailList.length <= 1}
                      onClick={() => setEmailList((current) => removeEmail(current, address))}
                    />
                  </div>
                ))}
              </div>
              {adding ? (
                <div className="flex flex-col gap-2 rounded-md border border-hairline bg-nav-hover/40 p-2.5">
                  <FormField
                    label={t('cloudNotifyEmailAddress')}
                    htmlFor="cn-address"
                    error={addressError}
                  >
                    <InputWithAction
                      input={
                        <Input
                          id="cn-address"
                          type="email"
                          value={newAddress}
                          placeholder="name@example.com"
                          invalid={addressError !== undefined}
                          autoComplete="off"
                          spellCheck={false}
                          onChange={(event) => {
                            setNewAddress(event.target.value)
                            setAddressTouched(false)
                          }}
                          onBlur={() => setAddressTouched(true)}
                        />
                      }
                      action={
                        readdAccount ? (
                          <Button variant="primary" onClick={addAccountBack}>
                            {t('cloudNotifyEmailAdd')}
                          </Button>
                        ) : (
                          <Button
                            loading={sending}
                            disabled={!canSendEmailCode(newAddress, emailList, overview.accountEmail) || sending || cooldown.remaining > 0}
                            onClick={() => void sendCode()}
                          >
                            {cooldown.remaining > 0 ? t('cloudNotifyEmailResendIn', { s: cooldown.remaining }) : t('cloudNotifyEmailSendCode')}
                          </Button>
                        )
                      }
                    />
                  </FormField>
                  {readdAccount ? null : (
                    <FormField
                      label={t('cloudNotifyEmailCode')}
                      htmlFor="cn-code"
                      hint={sentInfo ? t('cloudNotifyEmailCodeSent', sentInfo) : undefined}
                      error={formError ?? undefined}
                    >
                      <InputWithAction
                        input={
                          <Input
                            id="cn-code"
                            value={newCode}
                            inputMode="numeric"
                            maxLength={6}
                            autoComplete="one-time-code"
                            placeholder="000000"
                            onChange={(event) => setNewCode(event.target.value.replace(/\D/g, ''))}
                          />
                        }
                        action={
                          <Button
                            variant="primary"
                            loading={verifying}
                            disabled={!canVerifyEmail(newAddress, newCode, emailList, overview.accountEmail) || verifying}
                            onClick={() => void verify()}
                          >
                            {t('cloudNotifyEmailVerify')}
                          </Button>
                        }
                      />
                    </FormField>
                  )}
                  <div className="flex justify-end">
                    <Button variant="ghost" onClick={closeAddForm}>
                      {t('cancel')}
                    </Button>
                  </div>
                </div>
              ) : (
                <div>
                  <Button
                    icon={Plus}
                    disabled={!canAddMoreEmails(emailList)}
                    title={canAddMoreEmails(emailList) ? undefined : t('cloudNotifyEmailMaxReached', { max: EMAIL_MAX_ADDRESSES })}
                    onClick={() => setAdding(true)}
                  >
                    {t('cloudNotifyEmailAdd')}
                  </Button>
                  {canAddMoreEmails(emailList) ? null : (
                    <span className="ml-2 text-xs text-text-tertiary">{t('cloudNotifyEmailMaxReached', { max: EMAIL_MAX_ADDRESSES })}</span>
                  )}
                </div>
              )}
            </div>
          </FormField>
        ) : null}

        <FormField label={t('cloudNotifyEvents')} error={events.length === 0 ? t('cloudNotifyEventsEmpty') : undefined}>
          <div className="grid grid-cols-2 gap-x-2 narrow:grid-cols-1">
            {CLOUD_EVENTS.map((event) => (
              <CheckRow
                key={event.wire}
                checked={events.includes(event.wire)}
                onCheckedChange={(checked) =>
                  setEvents((current) => CLOUD_EVENTS.map((item) => item.wire).filter((wire) => (wire === event.wire ? checked : current.includes(wire))))
                }
              >
                {t(event.labelKey)}
              </CheckRow>
            ))}
          </div>
        </FormField>

        {error ? (
          <div role="alert" className="text-xs text-destructive">
            {error}
          </div>
        ) : null}
      </div>
    </Dialog>
  )
}
