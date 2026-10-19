// 受信任设备（GPUI devices.rs）：列表 + 重试 + 详情 / 重命名 / 删除（agent.device.*；删除当前设备 = 登出）。

import { Info, Pencil, Power, RefreshCw, TriangleAlert, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useT } from '../../../../i18n'
import { rpc } from '../../../../lib/rpc'
import type { CloudConnectionDto, CloudDevice } from '../../../../lib/rpc'
import { canDisconnectDevice, cloudConnectionLabelKey, cloudPresenceKnown, devicePresenceKey, onlineLimitWaiting } from '../../../../lib/cloud-presence'
import { Badge, Button, Card, ConfirmFooter, Dialog, FieldError, Form, FormField, Icon, Input, confirmDialog, toast } from '../../../../ui'
import { accountErrorKey, REASON_KEYS } from './errorText'
import { sortedDevices } from './deviceList'

const PLATFORM_KEYS: Record<string, string> = {
  windows: 'accountDevicePlatformWindows',
  macos: 'accountDevicePlatformMacos',
  linux: 'accountDevicePlatformLinux',
  android: 'accountDevicePlatformAndroid',
  ios: 'accountDevicePlatformIos',
  web: 'accountDevicePlatformWeb',
}

function RenameDialog({ device, onClose }: { device: CloudDevice; onClose: () => void }) {
  const t = useT()
  const [name, setName] = useState(device.name)
  const [busy, setBusy] = useState(false)
  const [errorKey, setErrorKey] = useState<string | null>(null)
  const trimmed = name.trim()
  const invalid = trimmed.length < 1 || [...trimmed].length > 64

  const submit = async () => {
    if (busy || invalid) return
    setBusy(true)
    setErrorKey(null)
    try {
      await rpc.agent.device.rename({ id: device.id, name: trimmed })
      onClose()
    } catch (error) {
      setErrorKey(accountErrorKey(error))
      setBusy(false)
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && !busy && onClose()}
      title={t('accountDeviceRenameTitle')}
      modalLocked={busy}
      footer={<ConfirmFooter okLabel={t('confirm')} onCancel={onClose} onOk={() => void submit()} okDisabled={invalid} loading={busy} />}
    >
      <Form onSubmit={() => void submit()}>
        <FormField label={t('accountDeviceRenameTitle')} htmlFor="account-device-name" {...(invalid && name !== '' ? { error: t('accountDeviceRenameInvalid') } : {})}>
          <Input id="account-device-name" value={name} onChange={(event) => setName(event.target.value)} invalid={invalid && name !== ''} autoFocus />
        </FormField>
        {errorKey ? <FieldError>{t(errorKey)}</FieldError> : null}
        <button type="submit" className="hidden" />
      </Form>
    </Dialog>
  )
}

function formatDate(value: string): string {
  return Number.isNaN(Date.parse(value)) ? '—' : new Date(value).toLocaleString()
}

function DetailDialog({ device, presenceKnown, onClose }: { device: CloudDevice; presenceKnown: boolean; onClose: () => void }) {
  const t = useT()
  const platformKey = device.platform ? PLATFORM_KEYS[device.platform.toLowerCase()] : undefined
  const rows: [string, string][] = [
    [t('accountDeviceFieldPlatform'), platformKey ? t(platformKey) : (device.platform ?? '—')],
    [t('accountDeviceFieldAppVersion'), device.appVersion ?? '—'],
    [t('accountDeviceFieldLastIp'), device.lastIp ?? '—'],
    [t('accountDeviceFieldCreatedAt'), formatDate(device.createdAt)],
    [t('accountDeviceFieldLastSeenAt'), formatDate(device.lastSeenAt)],
    [t('accountDeviceFieldOnline'), t(devicePresenceKey(device, presenceKnown))],
    [t('accountDeviceFieldId'), device.deviceId],
  ]
  if (device.defaultSaveDir) rows.push([t('accountDeviceFieldSaveDir'), device.defaultSaveDir])
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()} title={device.name || t('accountDeviceDetailTitle')} description={t('accountDeviceDetailTitle')} footer={<ConfirmFooter okLabel={t('close')} cancelLabel={null} onCancel={onClose} onOk={onClose} />}>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
        {rows.map(([label, value]) => (
          <div key={label} className="contents">
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="min-w-0 break-all text-foreground">{value}</dd>
          </div>
        ))}
      </dl>
    </Dialog>
  )
}

function DeviceRow({ device, disabled, presenceKnown, waiting }: { device: CloudDevice; disabled: boolean; presenceKnown: boolean; waiting: boolean }) {
  const t = useT()
  const [renaming, setRenaming] = useState(false)
  const [detail, setDetail] = useState(false)
  const platformKey = device.platform ? PLATFORM_KEYS[device.platform.toLowerCase()] : undefined
  const platform = platformKey ? t(platformKey) : (device.platform ?? '')
  const lastSeen = Number.isNaN(Date.parse(device.lastSeenAt)) ? '' : new Date(device.lastSeenAt).toLocaleString()
  const meta = [t(devicePresenceKey(device, presenceKnown)), platform, device.appVersion, lastSeen].filter((part): part is string => Boolean(part)).join(' · ')

  const remove = async () => {
    const description = device.isCurrent
      ? `${t('accountDeviceDeleteConfirmDesc')}\n${t('accountDeviceDeleteCurrentWarning')}`
      : t('accountDeviceDeleteConfirmDesc')
    const ok = await confirmDialog({
      title: t('accountDeviceDeleteConfirmTitle'),
      description,
      intent: 'destructive',
      okLabel: t('accountDeviceDeleteAction'),
    })
    if (!ok) return
    try {
      await rpc.agent.device.delete({ id: device.id })
    } catch (error) {
      toast.key(accountErrorKey(error), 'error')
    }
  }

  const disconnect = async () => {
    try {
      await rpc.agent.device.disconnect({ id: device.id })
    } catch (error) {
      toast.key(accountErrorKey(error), 'error')
    }
  }

  return (
    <div className="flex items-center gap-2 px-3 py-2 coarse:min-h-touch">
      <span className={`size-2 shrink-0 rounded-full ${presenceKnown && device.isOnline ? 'bg-success' : 'bg-text-tertiary/50'}`} aria-hidden />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="min-w-0 truncate text-sm font-medium text-foreground">{device.name}</span>
          {device.isCurrent ? <Badge tone="accent">{t('accountDeviceCurrent')}</Badge> : null}
        </div>
        {meta ? <div className="truncate text-xs text-muted-foreground">{meta}</div> : null}
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        {canDisconnectDevice(device, presenceKnown, waiting) ? (
          <Button variant="ghost" iconOnly aria-label={t('accountDeviceDisconnectAction')} title={t('accountDeviceDisconnectAction')} className="text-muted-foreground hover:text-foreground" disabled={disabled} onClick={() => void disconnect()}>
            <Icon icon={Power} size="md" />
          </Button>
        ) : null}
        <Button variant="ghost" iconOnly aria-label={t('accountDeviceDetailTitle')} title={t('accountDeviceDetailTitle')} className="text-muted-foreground hover:text-foreground" onClick={() => setDetail(true)}>
          <Icon icon={Info} size="md" />
        </Button>
        <Button variant="ghost" iconOnly aria-label={t('accountDeviceRenameTitle')} title={t('accountDeviceRenameTitle')} className="text-muted-foreground hover:text-foreground" disabled={disabled} onClick={() => setRenaming(true)}>
          <Icon icon={Pencil} size="md" />
        </Button>
        <Button
          variant="ghost"
          iconOnly
          aria-label={t('accountDeviceDeleteAction')}
          title={t('accountDeviceDeleteAction')}
          className="text-muted-foreground hover:text-destructive"
          disabled={disabled}
          onClick={() => void remove()}
        >
          <Icon icon={Trash2} size="md" />
        </Button>
      </div>
      {detail ? <DetailDialog device={device} presenceKnown={presenceKnown} onClose={() => setDetail(false)} /> : null}
      {renaming ? <RenameDialog device={device} onClose={() => setRenaming(false)} /> : null}
    </div>
  )
}

function OnlineLimitNotice({ devices, maxOnline, disabled }: { devices: readonly CloudDevice[]; maxOnline: number | null; disabled: boolean }) {
  const t = useT()
  const occupying = devices.filter((device) => device.isOnline && !device.isCurrent)
  const disconnect = async (device: CloudDevice) => {
    try {
      await rpc.agent.device.disconnect({ id: device.id })
    } catch (error) {
      toast.key(accountErrorKey(error), 'error')
    }
  }
  return (
    <div role="status" aria-live="polite" className="flex flex-col gap-2 rounded-md border border-warning/30 bg-warning/8 px-3 py-2">
      <div className="flex items-start gap-2 text-sm text-foreground">
        <Icon icon={TriangleAlert} size="md" className="mt-0.5 shrink-0 text-warning" />
        <span>{maxOnline === null ? t('cloudOnlineLimitNoticeNoMax') : t('cloudOnlineLimitNotice', { max: maxOnline })}</span>
      </div>
      {occupying.length > 0 ? (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">{t('cloudOnlineLimitListTitle')}</div>
          {occupying.map((device) => (
            <div key={device.id} className="flex items-center justify-between gap-2">
              <span className="min-w-0 truncate text-sm text-foreground">{device.name}</span>
              <Button disabled={disabled} onClick={() => void disconnect(device)}>
                {t('accountDeviceDisconnectAction')}
              </Button>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  )
}

export function DevicesCard({ devices, disabled, connection, maxOnline }: {
  devices: readonly CloudDevice[]
  disabled: boolean
  connection: CloudConnectionDto | undefined
  maxOnline: number | null
}) {
  const t = useT()
  const [refreshing, setRefreshing] = useState(false)
  const [errorKey, setErrorKey] = useState<string | null>(null)
  const presenceKnown = cloudPresenceKnown(connection, !disabled)
  const waiting = onlineLimitWaiting(connection, !disabled)
  const connectionErrorKey = connection?.lastErrorReason
    ? (REASON_KEYS[connection.lastErrorReason] ?? 'accountErrorNetwork')
    : connection?.lastError ? 'accountErrorNetwork' : null

  const refresh = async () => {
    if (refreshing || disabled) return
    setRefreshing(true)
    setErrorKey(null)
    try {
      if (!presenceKnown) await rpc.agent.remote.reconnect()
      await rpc.agent.device.list()
      toast.key(presenceKnown ? 'accountCloudRefreshDone' : 'cloudConnectionRetryStarted', 'success')
    } catch (error) {
      setErrorKey(accountErrorKey(error))
    } finally {
      setRefreshing(false)
    }
  }

  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <div className="text-sm font-medium text-foreground">{t('accountDevicesTitle')}</div>
          <div className="text-xs text-muted-foreground">{t('accountDevicesDesc')}</div>
        </div>
        <Button icon={RefreshCw} disabled={disabled} loading={refreshing} onClick={() => void refresh()}>
          {t(presenceKnown ? 'accountDevicesRetry' : 'cloudConnectionRetry')}
        </Button>
      </div>
      <div className="flex flex-col gap-1 text-xs" role="status" aria-live="polite">
        <span className={presenceKnown ? 'text-success' : 'text-muted-foreground'}>
          {t(cloudConnectionLabelKey(connection, !disabled))}
        </span>
        {!presenceKnown ? <span className="text-muted-foreground">{t('cloudConnectionStatusHint')}</span> : null}
        {!disabled && connectionErrorKey && !waiting ? <FieldError>{t(connectionErrorKey)}</FieldError> : null}
      </div>
      {waiting ? <OnlineLimitNotice devices={devices} maxOnline={maxOnline} disabled={disabled} /> : null}
      {errorKey ? <FieldError>{t(errorKey)}</FieldError> : null}
      <Card className="w-full overflow-hidden [&>*+*]:border-t [&>*+*]:border-hairline">
        {devices.length === 0 ? (
          <div className="flex justify-center p-4 text-xs text-muted-foreground">{t('accountDevicesEmpty')}</div>
        ) : (
          sortedDevices(devices, presenceKnown).map((device) => <DeviceRow key={device.id} device={device} disabled={disabled} presenceKnown={presenceKnown} waiting={waiting} />)
        )}
      </Card>
    </section>
  )
}
