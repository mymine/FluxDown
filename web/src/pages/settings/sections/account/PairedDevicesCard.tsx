// 已配对设备（局域网直连）：列表 + 在线探测 + 重命名 + 解除配对 + 添加设备入口。

import { Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useT } from '../../../../i18n'
import type { TFunction } from '../../../../i18n'
import { rpc, useAgent } from '../../../../lib/rpc'
import type { CloudDevice, LinkDeviceInfo } from '../../../../lib/rpc'
import { Badge, Button, Card, ConfirmFooter, Dialog, FieldError, FieldHint, Form, FormField, Icon, Input, confirmDialog, toast } from '../../../../ui'
import { AddDeviceDialog } from './AddDeviceDialog'
import { accountErrorKey } from './errorText'

const NO_CLOUD: readonly CloudDevice[] = []

/** 备注名重命名：留空 = 清除备注名（恢复对方设备名）；非空 1–64 字符。 */
export function RenameLinkDialog({ device, onClose }: { device: LinkDeviceInfo; onClose: () => void }) {
  const t = useT()
  const [name, setName] = useState(device.customName ?? '')
  const [busy, setBusy] = useState(false)
  const [errorKey, setErrorKey] = useState<string | null>(null)
  const trimmed = name.trim()
  const invalid = trimmed !== '' && [...trimmed].length > 64

  const submit = async () => {
    if (busy || invalid) return
    setBusy(true)
    setErrorKey(null)
    try {
      await rpc.agent.link.rename({ fingerprint: device.fingerprint, ...(trimmed !== '' ? { name: trimmed } : {}) })
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
        <FormField label={t('accountDeviceRenameTitle')} htmlFor="link-device-name" {...(invalid ? { error: t('accountDeviceRenameInvalid') } : {})}>
          <Input id="link-device-name" value={name} placeholder={device.name} onChange={(event) => setName(event.target.value)} invalid={invalid} autoFocus />
        </FormField>
        <FieldHint>{t('linkedDeviceRenameHint')}</FieldHint>
        {errorKey ? <FieldError>{t(errorKey)}</FieldError> : null}
        <button type="submit" className="hidden" />
      </Form>
    </Dialog>
  )
}

/** 解除配对：确认后调用 `agent.link.remove`；失败 toast。设置页与下载页侧栏共用。 */
export async function confirmRemoveLinkDevice(t: TFunction, device: LinkDeviceInfo): Promise<void> {
  const ok = await confirmDialog({
    title: t('linkedDeviceRemoveTitle'),
    description: t('linkedDeviceRemoveDesc', { name: device.name }),
    intent: 'destructive',
    okLabel: t('linkedDeviceRemove'),
  })
  if (!ok) return
  try {
    await rpc.agent.link.remove({ fingerprint: device.fingerprint })
  } catch (error) {
    toast.key(accountErrorKey(error), 'error')
  }
}

function PairedRow({ device, disabled, sameAccount }: { device: LinkDeviceInfo; disabled: boolean; sameAccount: boolean }) {
  const t = useT()
  const [renaming, setRenaming] = useState(false)
  const seen = device.lastSeenAt > 0 ? new Date(device.lastSeenAt * (device.lastSeenAt < 1e12 ? 1000 : 1)).toLocaleString() : ''
  const meta = [device.platform, device.defaultSaveDir, seen].filter((part): part is string => Boolean(part)).join(' · ')

  return (
    <div className="flex items-center gap-2 px-3 py-2 coarse:min-h-touch">
      <span className={`size-2 shrink-0 rounded-full ${!disabled && !device.revoked && device.online ? 'bg-success' : 'bg-text-tertiary/50'}`} aria-hidden />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="min-w-0 truncate text-sm font-medium text-foreground">{device.name}</span>
          <Badge>{t('deviceLocalTag')}</Badge>
          {device.revoked ? <Badge>{t('linkedDeviceRevokedTag')}</Badge> : null}
          {sameAccount && !device.revoked ? <Badge>{t('linkedDeviceSameAccountTag')}</Badge> : null}
          {device.grantsControl && !device.revoked ? <Badge>{t('linkControlGrantedTag')}</Badge> : null}
          {device.revoked ? null : (
            <span className="text-xs text-muted-foreground">{t(disabled ? 'devicePresenceUnknown' : device.online ? 'deviceOnline' : 'deviceOffline')}</span>
          )}
        </div>
        {device.revoked ? <div className="truncate text-xs text-muted-foreground">{t('linkedDeviceRevokedDesc')}</div> : null}
        {meta ? <div className="truncate text-xs text-muted-foreground">{meta}</div> : null}
      </div>
      {device.revoked ? null : (
        <Button variant="ghost" iconOnly aria-label={t('linkedDeviceRename')} title={t('linkedDeviceRename')} className="text-muted-foreground" disabled={disabled} onClick={() => setRenaming(true)}>
          <Icon icon={Pencil} size="md" />
        </Button>
      )}
      <Button variant="ghost" iconOnly aria-label={t('linkedDeviceRemove')} title={t('linkedDeviceRemove')} className="text-muted-foreground hover:text-destructive" disabled={disabled} onClick={() => void confirmRemoveLinkDevice(t, device)}>
        <Icon icon={Trash2} size="md" />
      </Button>
      {renaming ? <RenameLinkDialog device={device} onClose={() => setRenaming(false)} /> : null}
    </div>
  )
}

export function PairedDevicesCard({ devices, disabled }: { devices: readonly LinkDeviceInfo[]; disabled: boolean }) {
  const cloudDevices = useAgent((snapshot) => snapshot.cloudDevices, NO_CLOUD)
  const cloudIds = new Set(cloudDevices.map((device) => device.deviceId))
  const t = useT()
  const [adding, setAdding] = useState(false)
  const [refreshing, setRefreshing] = useState(false)

  const refresh = async () => {
    if (refreshing) return
    setRefreshing(true)
    try {
      await rpc.agent.link.refresh()
    } catch (error) {
      toast.key(accountErrorKey(error), 'error')
    } finally {
      setRefreshing(false)
    }
  }

  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <div className="text-sm font-medium text-foreground">{t('linkedDevicesTitle')}</div>
          <div className="text-xs text-muted-foreground">{t('linkedDevicesDesc')}</div>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button icon={RefreshCw} disabled={disabled} loading={refreshing} onClick={() => void refresh()}>
            {t('linkedDeviceRefresh')}
          </Button>
          <Button variant="primary" icon={Plus} disabled={disabled} onClick={() => setAdding(true)}>
            {t('addDeviceEntry')}
          </Button>
        </div>
      </div>
      <Card className="w-full overflow-hidden [&>*+*]:border-t [&>*+*]:border-hairline">
        {devices.length === 0 ? (
          <div className="flex justify-center p-4 text-xs text-muted-foreground">{t('linkedDevicesEmpty')}</div>
        ) : (
          devices.map((device) => (
            <PairedRow key={device.fingerprint} device={device} disabled={disabled} sameAccount={Boolean(device.cloudDeviceId) && cloudIds.has(device.cloudDeviceId ?? '')} />
          ))
        )}
      </Card>
      {adding ? <AddDeviceDialog onClose={() => setAdding(false)} /> : null}
    </section>
  )
}
