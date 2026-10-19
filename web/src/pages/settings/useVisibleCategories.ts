// 设置分类的条件显隐（对齐 GPUI `build_pages`）：「推介有奖」= 已登录 && 设备本地偏好 `ui.show_settings_referral`。

import { useAgentSnapshot } from '../../lib/rpc'
import { SETTINGS_CATEGORIES } from './categories'
import type { SettingsCategory } from './categories'
import { usePrefBoolean } from './kit'
import { SHOW_REFERRAL_PREF } from './sections/referral/pref'

/** `ready=false` 表示快照尚未到达，不能据此判定隐藏（避免直达链接被误重定向）。 */
export function useVisibleCategories(): { ready: boolean; categories: readonly SettingsCategory[] } {
  const snapshot = useAgentSnapshot()
  const referralPref = usePrefBoolean(SHOW_REFERRAL_PREF, false)
  const referral = snapshot !== null && snapshot.session !== null && snapshot.session !== undefined && referralPref
  return { ready: snapshot !== null, categories: SETTINGS_CATEGORIES.filter((category) => category.gate !== 'referral' || referral) }
}
