// 任务组「文件夹行」的纯文案与聚合取值（计数行各段、统一协议）。

import { SPARKLINE_MAX_BARS } from '../model/groupTree'
import type { GroupAggregate } from '../model/groupTree'
import { MAX_ETA_SECS, PROTOCOL_LABEL } from '../model/task'
import type { DownloadTaskView, TaskProtocol } from '../model/task'
import { formatEta } from './text'
import type { Translate } from './text'

export type CountTone = 'plain' | 'accent' | 'destructive'

export interface CountSegment {
  text: string
  tone: CountTone
}

/** 计数行各段（0 的段省略；剩余时间只在 ≤ 一天时给出）：总数 · 已完成 · 下载中 · 排队 · 暂停 · 失败 · 剩余。 */
export function groupCountSegments(t: Translate, aggregate: GroupAggregate): CountSegment[] {
  const { counts } = aggregate
  const segments: CountSegment[] = [{ text: t('groupItemsCount', { n: counts.total }), tone: 'plain' }]
  if (counts.done > 0) segments.push({ text: t('groupDoneCount', { n: counts.done }), tone: 'plain' })
  if (counts.downloading > 0) segments.push({ text: t('groupDownloadingCount', { n: counts.downloading }), tone: 'accent' })
  if (counts.pending > 0) segments.push({ text: t('groupPendingCount', { n: counts.pending }), tone: 'plain' })
  if (counts.paused > 0) segments.push({ text: t('groupPausedCount', { n: counts.paused }), tone: 'plain' })
  if (counts.failed > 0) segments.push({ text: t('groupFailedCount', { n: counts.failed }), tone: 'destructive' })
  if (aggregate.etaSeconds !== null && aggregate.etaSeconds <= MAX_ETA_SECS) {
    segments.push({ text: t('groupEtaRemaining', { eta: formatEta(t, aggregate.etaSeconds) }), tone: 'plain' })
  }
  return segments
}

/** 通过成员全部相同的取值；成员为空或有差异返回 null。 */
export function uniformValue<T>(members: Iterable<{ view: DownloadTaskView }>, pick: (view: DownloadTaskView) => T): T | null {
  let found: T | null = null
  let first = true
  for (const member of members) {
    const value = pick(member.view)
    if (first) {
      found = value
      first = false
    } else if (value !== found) {
      return null
    }
  }
  return found
}

/** 协议列：通过成员协议全部相同则显示，否则 `—`。 */
export function uniformProtocolLabel(members: Iterable<{ view: DownloadTaskView }>): string {
  const protocol = uniformValue<TaskProtocol>(members, (view) => view.protocol)
  return protocol === null ? '—' : PROTOCOL_LABEL[protocol]
}

/** 进度柱状图的柱宽 / 间距 / 柱高区（px）；紧凑密度用窄柱和矮柱高区。 */
export function sparklineMetrics(compact: boolean): { barWidth: number; gap: number; areaHeight: number } {
  return compact ? { barWidth: 4, gap: 2, areaHeight: 14 } : { barWidth: 5, gap: 2, areaHeight: 18 }
}

/** 可画柱数 = min(24, 成员数, ⌊(可用宽 + 间距) / (柱宽 + 间距)⌋)。 */
export function sparklineBarCount(memberCount: number, availableWidth: number, compact: boolean): number {
  const { barWidth, gap } = sparklineMetrics(compact)
  const fit = Math.floor((Math.max(0, availableWidth) + gap) / (barWidth + gap))
  return Math.max(0, Math.min(SPARKLINE_MAX_BARS, memberCount, fit))
}
