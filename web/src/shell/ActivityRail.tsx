// 活动栏（桌面）：48px 宽、chrome 底；上 / 下两个可自定义区域（railTop / railBottom），
// 默认路由在上、动作（账户 / 设置）在下。两个区域都没有可见条目时整条隐藏（编辑态除外，否则无处可拖回）。
// 选中项：中性底 `navSelected` + 图标 `navSelectedIcon`（强调色只落在图标上）。

import { ChromeMenuArea } from './ChromeEdit'
import { useChromeEdit } from './chromeStore'
import { ChromeZoneView } from './ChromeZone'
import { useChromeZoneItems } from './useChromeZoneItems'

const ZONE = 'flex flex-col items-center gap-1'

export function ActivityRail() {
  const { editing } = useChromeEdit()
  const top = useChromeZoneItems('railTop')
  const bottom = useChromeZoneItems('railBottom')
  if (top.length === 0 && bottom.length === 0 && !editing) return null
  return (
    <ChromeMenuArea>
      <nav aria-label="Activity" className="flex w-rail shrink-0 flex-col items-center justify-between gap-1 bg-chrome py-2 pl-safe">
        <ChromeZoneView zone="railTop" className={ZONE} keepEmpty />
        <ChromeZoneView zone="railBottom" className={ZONE} keepEmpty />
      </nav>
    </ChromeMenuArea>
  )
}
