// 侧栏选中项与状态筛选（移植 crates/downloads/src/model/mod.rs）。

import type { CategoryIndex } from './categories'
import { isSeeding } from './task'
import type { DownloadTaskView, TaskState } from './task'

export type DownloadStatusFilter = 'all' | 'completed' | 'seeding' | 'incomplete' | 'failed' | 'paused'

/** 侧栏状态文件夹顺序（「做种中」紧随「已完成」）。 */
export const STATUS_FILTERS: readonly DownloadStatusFilter[] = ['all', 'incomplete', 'completed', 'seeding', 'failed', 'paused']

/** `seeding` = 任务是否处于做种中（{@link isSeeding}）；做种任务同时属于「已完成」。 */
export function statusMatches(filter: DownloadStatusFilter, state: TaskState, seeding: boolean): boolean {
  switch (filter) {
    case 'all':
      return true
    case 'completed':
      return state === 'completed'
    case 'seeding':
      return seeding
    // 下载中筛选包含下载中与等待中的任务。
    case 'incomplete':
      return state === 'downloading' || state === 'pending'
    case 'failed':
      return state === 'failed'
    case 'paused':
      return state === 'paused'
  }
}

/** 状态 + 分类（分类 id 来自 `CustomCategoryDto.id`）。 */
export interface DownloadFilter {
  status: DownloadStatusFilter
  category: string | null
}

export const FILTER_ALL: DownloadFilter = { status: 'all', category: null }

export function filterMatches(filter: DownloadFilter, view: DownloadTaskView, categories: CategoryIndex): boolean {
  return (
    statusMatches(filter.status, view.state, isSeeding(view)) &&
    (filter.category === null || categories.matches(filter.category, view))
  )
}

export const LOCAL_DEVICE = 'local'

/** 侧栏当前选中项；决定表格的筛选来源。 */
export type SidebarSelection =
  | { kind: 'download'; filter: DownloadFilter }
  | { kind: 'queue'; queueId: string }
  /** `local` = 本机，其余为云设备 id / 配对设备指纹。 */
  | { kind: 'device'; deviceId: string }

export const SELECTION_ALL: SidebarSelection = { kind: 'download', filter: FILTER_ALL }

/** 视图是否属于侧栏当前选中项（筛选 + 分类 / 队列 / 设备）。 */
export function selectionMatches(selection: SidebarSelection, view: DownloadTaskView, categories: CategoryIndex): boolean {
  switch (selection.kind) {
    case 'download':
      return filterMatches(selection.filter, view, categories)
    case 'queue':
      return view.source === 'local' && view.queueId === selection.queueId
    case 'device':
      if (selection.deviceId === LOCAL_DEVICE) return view.source === 'local'
      return view.source === 'remote' && view.toDevice === selection.deviceId
  }
}

/** 选中项不筛掉任何任务（「全部」且不限分类）。 */
export function selectsEverything(selection: SidebarSelection): boolean {
  return selection.kind === 'download' && selection.filter.status === 'all' && selection.filter.category === null
}

export function sameSelection(a: SidebarSelection, b: SidebarSelection): boolean {
  if (a.kind !== b.kind) return false
  switch (a.kind) {
    case 'download':
      return (
        b.kind === 'download' && a.filter.status === b.filter.status && a.filter.category === b.filter.category
      )
    case 'queue':
      return b.kind === 'queue' && a.queueId === b.queueId
    case 'device':
      return b.kind === 'device' && a.deviceId === b.deviceId
  }
}

/** 侧栏分区可见性偏好键（同步偏好）。 */
export const SIDEBAR_SECTION_PREFS = {
  status: 'ui.show_sidebar_status',
  queues: 'ui.show_sidebar_queues',
  devices: 'ui.show_sidebar_devices',
  category: 'ui.show_sidebar_category',
} as const
