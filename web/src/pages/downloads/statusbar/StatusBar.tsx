import { ArrowDown, ArrowUp } from 'lucide-react'
import { ProxyControl } from './ProxyControl'
import { SpeedLimitControl } from './SpeedLimitControl'
import { DiskFreeCell, PauseAllButton, ResumeAllButton, SpeedCell } from './StatusWidgets'

/** 状态栏容器样式：移动端页面级状态栏与桌面外壳状态栏共用。 */
export const STATUS_BAR_CLASS =
  'flex h-status-bar shrink-0 items-center justify-between gap-3 overflow-x-auto border-t border-hairline bg-chrome px-2 text-caption tabular-nums text-muted-foreground mobile:pb-safe coarse:h-auto coarse:min-h-touch narrow:gap-2 [scrollbar-width:none]'

/**
 * 移动端页面级状态栏（固定布局，不可自定义）：左 = 全局速度 + 全部暂停 / 全部开始；
 * 右 = 下行 / 上行限速、代理模式、剩余空间。桌面端由外壳按自定义布局渲染同一批小部件。
 */
export function StatusBar() {
  return (
    <div className={STATUS_BAR_CLASS}>
      <div className="flex min-w-0 shrink-0 items-center gap-1">
        <SpeedCell direction="down" />
        <SpeedCell direction="up" />
        <PauseAllButton />
        <ResumeAllButton />
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <SpeedLimitControl icon={ArrowDown} titleKey="speedLimitTitle" configKey="speed_limit_bytes" />
        <SpeedLimitControl icon={ArrowUp} titleKey="uploadLimit" configKey="upload_limit_bytes" />
        <ProxyControl />
        <DiskFreeCell />
      </div>
    </div>
  )
}
