// 外壳状态栏（桌面）：挂在内容区下方，所有路由可见；左 / 右两个可自定义区域（statusLeading / statusTrailing）。
// 两个区域都没有可见条目时整条隐藏（编辑态除外）。移动端沿用下载页自带的固定状态栏（pages/downloads/statusbar）。

import { STATUS_BAR_CLASS } from '../pages/downloads/statusbar/StatusBar'
import { ChromeMenuArea } from './ChromeEdit'
import { useChromeEdit } from './chromeStore'
import { ChromeZoneView } from './ChromeZone'
import { useChromeZoneItems } from './useChromeZoneItems'

export function ChromeStatusBar() {
  const { editing } = useChromeEdit()
  const leading = useChromeZoneItems('statusLeading')
  const trailing = useChromeZoneItems('statusTrailing')
  if (leading.length === 0 && trailing.length === 0 && !editing) return null
  return (
    <ChromeMenuArea>
      <div className={STATUS_BAR_CLASS}>
        <ChromeZoneView zone="statusLeading" className="flex min-w-0 shrink-0 items-center gap-1" keepEmpty />
        <ChromeZoneView zone="statusTrailing" className="flex shrink-0 items-center gap-1" keepEmpty />
      </div>
    </ChromeMenuArea>
  )
}
