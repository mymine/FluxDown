// 状态栏小部件公共件：朝向 / 气泡方向、按钮样式、全局状态读取。
// 小部件既出现在外壳的标题栏 / 活动栏 / 状态栏区域（桌面可自定义布局），也出现在移动端页面级状态栏，
// 因此只依赖全局 agent store / rpc，不依赖下载页的 `useDownloads()`。

import { useAgent, useConnection, useDaemon, shallowEqual } from '../../../lib/rpc'
import type { DaemonRuntimeStatsDto } from '../../../lib/rpc'

/** `horizontal` = 标题栏 / 状态栏里的横排（文字 + 图标）；`rail` = 活动栏里的纯图标（数值进气泡）。 */
export type ChromeOrientation = 'horizontal' | 'rail'
export type TooltipSide = 'top' | 'right' | 'bottom' | 'left'

export interface WidgetProps {
  orientation?: ChromeOrientation
  /** 气泡 / 弹层方向；缺省：横排在上方，活动栏在右侧。 */
  side?: TooltipSide
}

export const resolveSide = (orientation: ChromeOrientation, side: TooltipSide | undefined): TooltipSide =>
  side ?? (orientation === 'rail' ? 'right' : 'top')

/** Popover 只支持 `bottom` / `right`：活动栏向右展开，其余向下（放不下时自动翻上）。 */
export const popoverSide = (orientation: ChromeOrientation): 'bottom' | 'right' => (orientation === 'rail' ? 'right' : 'bottom')

/** 横排小按钮（状态栏 / 标题栏）。 */
export const ACTION_BUTTON =
  'inline-flex h-status-control min-w-status-control shrink-0 items-center justify-center rounded-sm px-1 text-muted-foreground hover:bg-nav-hover hover:text-foreground disabled:pointer-events-none disabled:opacity-50 coarse:min-h-touch coarse:min-w-touch'

/** 活动栏图标按钮，与路由条目同款。 */
export const RAIL_BUTTON =
  'inline-flex size-8 coarse:size-touch shrink-0 items-center justify-center rounded-[var(--fx-components-nav-item-radius)] text-muted-foreground transition-colors hover:bg-nav-hover hover:text-foreground disabled:pointer-events-none disabled:opacity-50'

const EMPTY_STATS: DaemonRuntimeStatsDto = {
  activeTasks: 0,
  pendingTasks: 0,
  totalDownloadBps: 0,
  totalUploadBps: 0,
  diskFreeBytes: null,
  saveDir: '',
}
const selectStats = (daemon: { runtimeStats: DaemonRuntimeStatsDto }) => daemon.runtimeStats
const selectDaemonConnected = (snapshot: { daemonConnected: boolean }) => snapshot.daemonConnected

/** daemon 连接就绪（与下载页 `connected` 同一判定：连接 ready 且 daemon 在线）。 */
export function useDaemonReady(): boolean {
  const phase = useConnection().phase
  const daemonConnected = useAgent(selectDaemonConnected, false)
  return phase === 'ready' && daemonConnected
}

/** 全局运行时统计（速度 / 剩余空间）。 */
export function useRuntimeStats(): DaemonRuntimeStatsDto {
  return useDaemon(selectStats, EMPTY_STATS, shallowEqual)
}
