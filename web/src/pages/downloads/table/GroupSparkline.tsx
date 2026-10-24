// 组行进度列：每个通过成员一根柱（超过可画柱数时等距抽样），柱高 = 成员进度，柱色 = 成员状态。

import { cn } from '../../../lib/cn'
import type { GroupFolderRow } from '../model/groupRows'
import { sparklineHeight, sparklineSample } from '../model/groupTree'
import { percentLabel } from '../model/task'
import type { TaskState } from '../model/task'
import { sparklineBarCount, sparklineMetrics } from './groupFacts'

/** 状态文字色多为灰阶，柱状图用语义色（与 GPUI `render_group_sparkline` 一致）。 */
const BAR_COLOR: Record<TaskState, string> = {
  downloading: 'bg-progress-fill',
  pending: 'bg-progress-track',
  paused: 'bg-warning',
  failed: 'bg-status-failed',
  completed: 'bg-success',
}

interface Props {
  row: GroupFolderRow
  /** 柱状图可用宽度（列宽扣掉内边距、百分比与间距）。 */
  availableWidth: number
  compact: boolean
  /** 聚合已完成：百分比留空（与任务行一致），柱状图仍显示。 */
  completed: boolean
  progress: number
}

export function GroupSparkline({ row, availableWidth, compact, completed, progress }: Props) {
  const members = row.orderedRows()
  const { barWidth, gap, areaHeight } = sparklineMetrics(compact)
  const picks = sparklineSample(members.length, sparklineBarCount(members.length, availableWidth, compact))
  return (
    <div className="flex min-w-0 items-center gap-2">
      <div className="flex shrink-0 items-end" style={{ height: areaHeight, gap }}>
        {picks.map((index) => {
          const view = (members[index] as (typeof members)[number]).view
          // 准备中与下载中同色。
          const state: TaskState = view.state === 'pending' && view.preparing ? 'downloading' : view.state
          return (
            <span
              key={view.key}
              title={`${view.name} · ${percentLabel(view.progress)}`}
              className={cn('shrink-0 rounded-[1.5px]', BAR_COLOR[state])}
              style={{ width: barWidth, height: areaHeight * sparklineHeight(view.progress, state) }}
            />
          )
        })}
      </div>
      <span className="tabular w-8 shrink-0 text-right text-xs text-muted-foreground">
        {completed ? '' : percentLabel(progress)}
      </span>
    </div>
  )
}
