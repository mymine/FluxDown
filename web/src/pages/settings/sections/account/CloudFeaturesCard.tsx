// 云功能：配置同步（agent.sync.enable/disable/now，状态来自快照）+ 多设备协同在线数。

import { Network, RefreshCw } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useT } from '../../../../i18n'
import { rpc } from '../../../../lib/rpc'
import type { CloudConnectionDto, CloudDevice, Entitlements, SyncStatusDto } from '../../../../lib/rpc'
import { cloudPresenceKnown, maxOnlineDevices, onlineDeviceCount } from '../../../../lib/cloud-presence'
import { Badge, Button, Card, Icon, Switch, Tooltip, toast } from '../../../../ui'
import { accountErrorKey } from './errorText'
import { useSyncSubtitle } from './syncSubtitle'
import { SYNC_GROUPS, syncGroupState, syncPhase, toggleGroupParams } from './syncGroups'
import type { SyncGroup, SyncGroupState } from './syncGroups'

function RowIcon({ icon }: { icon: LucideIcon }) {
  return (
    <div className="flex size-control shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground coarse:size-11">
      <Icon icon={icon} />
    </div>
  )
}

function ScopeRow({ group, state, disabled, onToggle }: { group: SyncGroup; state: SyncGroupState; disabled: boolean; onToggle: () => void }) {
  const t = useT()
  return (
    <div className="flex items-center gap-3 px-3 py-2 pl-14">
      <div className="min-w-0 flex-1 text-sm text-foreground">{t(group.labelKey)}</div>
      {state === 'mixed' ? <Badge>{t('syncScopeMixed')}</Badge> : null}
      <Switch checked={state === 'sync'} disabled={disabled} onCheckedChange={onToggle} aria-label={t(group.labelKey)} />
    </div>
  )
}

export function CloudFeaturesCard({
  loggedIn,
  sync,
  devices,
  entitlements,
  disabled,
  connection,
}: {
  loggedIn: boolean
  sync: SyncStatusDto
  devices: readonly CloudDevice[]
  entitlements: Entitlements | undefined
  disabled: boolean
  connection: CloudConnectionDto | undefined
}) {
  const t = useT()
  const active = loggedIn && sync.enabled
  const phase = syncPhase(sync)
  const subtitle = useSyncSubtitle(loggedIn, sync)
  const online = onlineDeviceCount(devices)
  const maxOnline = maxOnlineDevices(entitlements)
  const localOnlyKeys = sync.localOnlyKeys ?? []

  const run = async (action: () => Promise<unknown>) => {
    try {
      await action()
    } catch (error) {
      toast.key(accountErrorKey(error), 'error')
    }
  }

  const toggle = (checked: boolean) => void run(() => (checked ? rpc.agent.sync.enable() : rpc.agent.sync.disable()))

  return (
    <section className="flex flex-col gap-2">
      <div className="flex flex-col gap-0.5">
        <div className="text-sm font-medium text-foreground">{t('accountGroupCloudFeatures')}</div>
        <div className="text-xs text-muted-foreground">{t('accountCloudFeaturesDesc')}</div>
      </div>
      <Card className="flex w-full flex-col [&>*+*]:border-t [&>*+*]:border-hairline">
        <div className="flex items-center gap-3 px-3 py-2">
          <RowIcon icon={RefreshCw} />
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <div className="text-sm font-medium text-foreground">{t('cloudSyncTitle')}</div>
            <div className={phase === 'halted' || phase === 'error' ? 'text-xs text-warning' : 'text-xs text-muted-foreground'}>{subtitle}</div>
          </div>
          {active ? (
            <Button disabled={disabled} onClick={() => void run(() => rpc.agent.sync.now())}>
              {t('cloudSyncNow')}
            </Button>
          ) : null}
          <Tooltip content={loggedIn ? null : t('cloudSyncLoginRequired')}>
            <span>
              <Switch checked={sync.enabled} disabled={disabled || !loggedIn} onCheckedChange={toggle} aria-label={t('cloudSyncTitle')} />
            </span>
          </Tooltip>
        </div>
        {active ? (
          <div className="flex flex-col py-1">
            <div className="flex flex-col gap-0.5 px-3 py-1 pl-14">
              <div className="text-xs font-medium text-foreground">{t('syncScopeTitle')}</div>
              <div className="text-xs text-muted-foreground">{t('syncScopeDesc')}</div>
            </div>
            {SYNC_GROUPS.map((group) => {
              const state = syncGroupState(group, localOnlyKeys)
              return (
                <ScopeRow
                  key={group.id}
                  group={group}
                  state={state}
                  disabled={disabled}
                  onToggle={() => void run(() => rpc.agent.sync.setLocalOnly(toggleGroupParams(group, state)))}
                />
              )
            })}
          </div>
        ) : null}
        <div className="flex items-center gap-3 px-3 py-2">
          <RowIcon icon={Network} />
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <div className="text-sm font-medium text-foreground">{t('multiDeviceTitle')}</div>
            <div className="text-xs text-muted-foreground">{t('multiDeviceDesc')}</div>
          </div>
          {loggedIn ? (
            <Badge>
              {cloudPresenceKnown(connection, !disabled)
                ? maxOnline === null
                  ? t('devicesOnlineCount', { count: online })
                  : t('devicesOnlineCountOf', { count: online, max: maxOnline })
                : t('devicePresenceUnknown')}
            </Badge>
          ) : null}
        </div>
      </Card>
    </section>
  )
}
