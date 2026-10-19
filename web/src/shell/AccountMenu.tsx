// 活动栏账户卡片（GPUI `crates/account/src/rail.rs` 的 `render_card`）：点击头像在右侧展开
// （移动端为底部 Sheet）。已登录 = 头像 / 展示名 / 邮箱 / 完整套餐徽标、配置同步开关与状态、
// 云端连接状态，以及「添加设备 / 账户设置 / 退出登录」；未登录 = 说明 + 登录 / 注册 + 账户设置。
// 对话框挂在卡片外：卡片关闭后对话框仍在。

import { useNavigate } from '@tanstack/react-router'
import { CircleUser, LogOut, Plus, Settings } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../i18n'
import { cloudConnectionLabelKey, cloudPresenceKnown } from '../lib/cloud-presence'
import { cn } from '../lib/cn'
import { rpc, useAgent, useConnection } from '../lib/rpc'
import type { SyncStatusDto } from '../lib/rpc'
import { LoginDialog, RegisterDialog } from '../pages/settings/sections/account/AuthDialogs'
import { AddDeviceDialog } from '../pages/settings/sections/account/AddDeviceDialog'
import { useSyncSubtitle } from '../pages/settings/sections/account/syncSubtitle'
import { accountErrorKey } from '../pages/settings/sections/account/errorText'
import { PlanBadge } from '../pages/settings/sections/account/PlanBadge'
import { Button, Icon, Popover, Switch, toast } from '../ui'
import { AccountAvatar } from './AccountAvatar'
import type { AccountAvatarState } from './accountAvatarState'

const NO_SYNC: SyncStatusDto = { enabled: false, revision: 0, dirtyKeys: [], lastError: null }

type AccountDialog = 'login' | 'register' | 'addDevice'

function MenuRow({ icon, label, onClick, disabled, destructive }: { icon: LucideIcon; label: string; onClick: () => void; disabled?: boolean; destructive?: boolean }) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={cn(
        'flex h-8 w-full items-center gap-2 rounded-[var(--fx-components-nav-item-radius)] px-2 text-sm transition-colors hover:bg-nav-hover disabled:pointer-events-none disabled:opacity-50 coarse:h-touch',
        destructive ? 'text-destructive' : 'text-muted-foreground hover:text-foreground',
      )}
    >
      <Icon icon={icon} />
      <span className="truncate">{label}</span>
    </button>
  )
}

async function runAction(action: () => Promise<unknown>): Promise<void> {
  try {
    await action()
  } catch (error) {
    toast.key(accountErrorKey(error), 'error')
  }
}

function AccountCard({ avatar, close, openDialog }: { avatar: AccountAvatarState; close: () => void; openDialog: (dialog: AccountDialog) => void }) {
  const t = useT()
  const navigate = useNavigate()
  const session = useAgent((snapshot) => snapshot.session, null)
  const sync = useAgent((snapshot) => snapshot.sync, NO_SYNC)
  const connection = useAgent((snapshot) => snapshot.cloudConnection, undefined)
  const disabled = useConnection().phase !== 'ready'
  const syncSubtitle = useSyncSubtitle(session !== null, sync)
  const act = (then: () => void) => () => {
    close()
    then()
  }
  const settingsRow = (
    <MenuRow icon={Settings} label={t('accountOpenSettings')} onClick={act(() => void navigate({ to: '/settings/$category', params: { category: 'account' } }))} />
  )

  if (!session) {
    return (
      <div className="flex w-full flex-col gap-3 p-3">
        <div className="flex flex-col items-center gap-1 text-center">
          <div className="flex size-10 items-center justify-center rounded-full bg-accent text-accent-text">
            <CircleUser strokeWidth={1.5} className="size-6" />
          </div>
          <div className="text-sm font-semibold text-foreground">{t('accountLoginDialogTitle')}</div>
          <div className="text-xs text-muted-foreground">{t('accountHeroSubtitle')}</div>
        </div>
        <div className="flex gap-2">
          <Button variant="primary" className="flex-1" disabled={disabled} onClick={act(() => openDialog('login'))}>
            {t('accountLogin')}
          </Button>
          <Button className="flex-1" disabled={disabled} onClick={act(() => openDialog('register'))}>
            {t('accountRegister')}
          </Button>
        </div>
        <div className="h-px bg-hairline" />
        {settingsRow}
      </div>
    )
  }

  const name = session.user.nickname.trim() || (session.user.email.split('@')[0] ?? session.user.email)
  const connected = cloudPresenceKnown(connection, !disabled)
  return (
    <div className="flex w-full flex-col gap-2 p-3">
      <div className="flex items-center gap-3">
        <AccountAvatar state={avatar} large />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-sm font-semibold text-foreground">{name}</span>
          <span className="truncate text-xs text-muted-foreground">{session.user.email}</span>
        </div>
      </div>
      {session.currentPlan ? (
        <div className="flex">
          <PlanBadge plan={session.currentPlan} ordinal={session.user.membershipOrdinal} />
        </div>
      ) : null}
      <div className="h-px bg-hairline" />
      <div className="flex items-center gap-2">
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="text-sm text-foreground">{t('cloudSyncTitle')}</span>
          <span className="text-xs text-muted-foreground">{syncSubtitle}</span>
        </div>
        <Switch
          checked={sync.enabled}
          disabled={disabled}
          aria-label={t('cloudSyncTitle')}
          onCheckedChange={(checked) => void runAction(() => (checked ? rpc.agent.sync.enable() : rpc.agent.sync.disable()))}
        />
      </div>
      <div className="flex items-center gap-2 text-xs text-muted-foreground" role="status">
        <span className={cn('size-1.5 shrink-0 rounded-full', connected ? 'bg-success' : 'bg-muted-foreground/50')} />
        {t(cloudConnectionLabelKey(connection, !disabled))}
      </div>
      <div className="h-px bg-hairline" />
      <div className="flex flex-col">
        <MenuRow icon={Plus} label={t('addDeviceEntry')} disabled={disabled} onClick={act(() => openDialog('addDevice'))} />
        {settingsRow}
        <MenuRow icon={LogOut} label={t('accountLogout')} disabled={disabled} destructive onClick={act(() => void runAction(() => rpc.agent.auth.logout()))} />
      </div>
    </div>
  )
}

/** 账户入口：`trigger(open)` 渲染触发器（活动栏头像 / 底部标签 / 标题栏头像），点击展开账户卡片（活动栏向右展开，标题栏向下）。 */
export function AccountMenu({ avatar, trigger, side = 'right' }: { avatar: AccountAvatarState; trigger: (open: boolean) => ReactNode; side?: 'bottom' | 'right' }) {
  const t = useT()
  const [open, setOpen] = useState(false)
  const [dialog, setDialog] = useState<AccountDialog | null>(null)
  const closeDialog = () => setDialog(null)
  return (
    <>
      <Popover trigger={trigger(open)} title={t('settingsCatAccount')} side={side} align="end" open={open} onOpenChange={setOpen} className="w-72">
        {(close) => <AccountCard avatar={avatar} close={close} openDialog={setDialog} />}
      </Popover>
      {dialog === 'login' ? <LoginDialog onClose={closeDialog} /> : null}
      {dialog === 'register' ? <RegisterDialog onClose={closeDialog} /> : null}
      {dialog === 'addDevice' ? <AddDeviceDialog onClose={closeDialog} /> : null}
    </>
  )
}
