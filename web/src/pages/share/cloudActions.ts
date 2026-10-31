// 云端暂存的操作与菜单条目（分享面板 / 「我的分享」行菜单共用）：转换（保留期档位）与删除云端副本。

import { CloudOff, CloudUpload } from 'lucide-react'
import { useState } from 'react'
import { useT } from '../../i18n'
import type { TFunction } from '../../i18n'
import { rpc } from '../../lib/rpc'
import type { ShareDto, ShareTierDto } from '../../lib/rpc'
import { confirmDialog, toast } from '../../ui'
import type { MenuEntry } from '../../ui'
import { accountErrorKey } from '../settings/sections/account/errorText'
import { canDeleteCloudCopy, cloudConvertAvailability, retentionTiers, shareDisplayStatus, tierLabel } from './model'
import { useShareSignedIn, useShareState } from './state'

/** 转换 / 删除；错误按类型化 reason 以 toast 呈现。 */
export function useCloudActions(share: ShareDto) {
  const t = useT()
  const [busy, setBusy] = useState(false)

  const run = async (action: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await action()
    } catch (error) {
      toast.error(t(accountErrorKey(error)))
    } finally {
      setBusy(false)
    }
  }

  return {
    busy,
    convert: (tierId?: string) => run(() => rpc.agent.share.convertToCloud({ id: share.id, tierId })),
    remove: async () => {
      if (share.cloudState === 'ready') {
        const ok = await confirmDialog({
          title: t('shareCloudDelete'),
          description: t('shareCloudDeleteConfirm'),
          okLabel: t('shareCloudDelete'),
          intent: 'destructive',
        })
        if (!ok) return
      }
      await run(() => rpc.agent.share.deleteCloudCopy(share.id))
    },
  }
}

/** 「转为云端暂存」入口的菜单条目：每个保留期档位一项。 */
export function convertEntries(t: TFunction, tiers: readonly ShareTierDto[], convert: (tierId?: string) => Promise<void>): MenuEntry[] {
  return tiers.map((tier) => ({
    type: 'item' as const,
    key: tier.id,
    label: tierLabel(t, tier),
    onSelect: () => void convert(tier.id),
  }))
}

/**
 * 「我的分享」行菜单里的云端条目：转为云端暂存（选档位的子菜单；不可转换时置灰并说明原因）/ 删除（取消）云端副本。
 * 没有云端存储（`hidden`）且无副本可删时为空。
 */
export function useCloudMenuEntries(share: ShareDto, nowMs: number): { entries: MenuEntry[]; busy: boolean } {
  const t = useT()
  const { policySummary } = useShareState()
  const signedIn = useShareSignedIn()
  const { busy, convert, remove } = useCloudActions(share)
  const availability = cloudConvertAvailability({ share, displayStatus: shareDisplayStatus(share, nowMs), policy: policySummary, signedIn })
  const entries: MenuEntry[] = []
  if (availability.kind === 'enabled') {
    entries.push({ type: 'sub', key: 'cloud-convert', label: t('shareCloudConvert'), icon: CloudUpload, entries: convertEntries(t, retentionTiers(policySummary), convert) })
  } else if (availability.kind === 'disabled') {
    entries.push({ type: 'item', key: 'cloud-convert', label: t('shareCloudConvert'), icon: CloudUpload, disabled: true, description: t(availability.reasonKey), onSelect: () => undefined })
  }
  if (canDeleteCloudCopy(share)) {
    entries.push({
      type: 'item',
      key: 'cloud-delete',
      label: t(share.cloudState === 'uploading' ? 'shareCloudCancelUpload' : 'shareCloudDelete'),
      icon: CloudOff,
      destructive: true,
      onSelect: () => void remove(),
    })
  }
  return { entries, busy }
}
