// 「更换链接 / 修改线程数」的可用性判定与线程数输入解析（镜像 GPUI `TaskMenuFacts::can_change_url` /
// `can_change_segments` 与 `pages/task_edit.rs::parse_task_segments`，改一处须同步另一处）。

import { MAX_TASK_SEGMENTS } from '../../../lib/threadsRisk'
import type { DownloadTaskView } from './task'

type EditFacts = Pick<DownloadTaskView, 'source' | 'state' | 'url'>

function hasPrefix(url: string, prefixes: readonly string[]): boolean {
  const lower = url.toLowerCase()
  return prefixes.some((prefix) => lower.startsWith(prefix))
}

/** BT 任务源：`magnet:` 链接或 `torrent-file://` 本地种子哨兵。 */
export function isBtUrl(url: string): boolean {
  return hasPrefix(url, ['magnet:', 'torrent-file://'])
}

/** 引擎 `change_task_url` 接受的源：http(s) / ftp。 */
export function isReplaceableUrl(url: string): boolean {
  return hasPrefix(url, ['http://', 'https://', 'ftp://'])
}

/** 「更换链接」：本地任务，暂停 / 失败 / 等待（引擎拒绝下载中与已完成），源为 http(s) / ftp。 */
export function canChangeUrl(view: EditFacts): boolean {
  return (
    view.source === 'local' &&
    (view.state === 'paused' || view.state === 'failed' || view.state === 'pending') &&
    isReplaceableUrl(view.url)
  )
}

/** 「修改线程数」：本地非 BT、未完成的任务（下载中也可，引擎静默暂停后恢复）。 */
export function canChangeSegments(view: EditFacts): boolean {
  return view.source === 'local' && view.state !== 'completed' && !isBtUrl(view.url)
}

/** 线程数输入 → 线程数：`0..=MAX_TASK_SEGMENTS` 的整数（0 = 自动）；空串 / 非数字 / 超限 → null。 */
export function parseTaskSegments(input: string): number | null {
  const text = input.trim()
  if (!/^\d+$/.test(text)) return null
  const value = Number(text)
  return Number.isSafeInteger(value) && value <= MAX_TASK_SEGMENTS ? value : null
}
