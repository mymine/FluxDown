// 任务行 / 分组头右键菜单（移植 task_table.rs 的 `context_menu_items` 与 `group_context_menu`）。
// 「打开文件 / 在文件夹中显示」仅在 agent 下发 `agent.openTaskFiles`（同机）时出现，其余情况用浏览器下载已完成文件。

import {
  AppWindow,
  CircleAlert,
  CircleCheck,
  Copy,
  Download,
  Layers,
  Link2,
  FolderOpen,
  Pause,
  PanelRight,
  Pen,
  Play,
  SquareCheck,
  RotateCw,
  Rows3,
  Trash2,
  Zap,
} from 'lucide-react'
import type { QueueDto } from '../../../lib/rpc'
import type { MenuEntry } from '../../../ui'
import {
  canOpenTaskFile,
  canRedownload,
  canRevealTaskFile,
  confirmDeleteGroupWithFiles,
  confirmDeleteWithFiles,
  confirmIgnorePluginRetry,
  copySourceLink,
  copyUrls,
  deleteGroup,
  deleteViews,
  downloadViewsFiles,
  isDownloadable,
  isPluginRetryError,
  moveViewsToQueue,
  openTaskFile,
  pauseGroup,
  pauseViews,
  redownloadViews,
  resumeGroup,
  resumeViews,
  retryFailedInGroup,
  revealTaskFile,
  toggleBoost,
} from '../model/actions'
import { remoteCan } from '../model/batchPlan'
import { canChangeSegments, canChangeUrl } from '../model/taskEdit'
import type { DownloadTaskView } from '../model/task'
import type { GroupSummary } from '../state'
import { openChangeSegments, openChangeUrl, openGroupDetail, openRename } from '../dialogs'

type Translate = (key: string, params?: Record<string, string | number>) => string

const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent)

/** 多选模式的右键入口（桌面表格专用；卡片视图有自己的点选方式，不传）。 */
interface SelectionEntries {
  /** 已处于显式多选模式时不再显示「选择 / 全选」。 */
  multiSelect: boolean
  enterMultiSelect: () => void
  selectAll: () => void
}

interface TaskMenuContext {
  t: Translate
  views: readonly DownloadTaskView[]
  queues: readonly QueueDto[]
  queueName: (queueId: string) => string
  showDetail: (taskId: string) => void
  selection?: SelectionEntries
}

/** 任务行右键菜单：多选时每一项都要求对全部选中任务成立（交集语义），resume/pause 例外。 */
export function buildTaskMenu({ t, views, queues, queueName, showDetail, selection }: TaskMenuContext): MenuEntry[] {
  if (views.length === 0) return []
  const entries: MenuEntry[] = []
  const only = views.length === 1 ? views[0] : undefined
  const allLocal = views.every((view) => view.source === 'local')

  if (views.some((view) => view.state !== 'completed' && view.state !== 'downloading' && remoteCan(view, 'resume'))) {
    entries.push({ type: 'item', key: 'resume', label: t('resume'), icon: Play, onSelect: () => void resumeViews(views) })
  }
  if (views.some((view) => (view.state === 'downloading' || view.state === 'pending') && remoteCan(view, 'pause'))) {
    entries.push({ type: 'item', key: 'pause', label: t('pause'), icon: Pause, onSelect: () => void pauseViews(views) })
  }
  if (only) {
    if (isPluginRetryError(only)) {
      entries.push({
        type: 'item',
        key: 'ignore-plugin-retry',
        label: t('taskIgnorePluginRetry'),
        icon: CircleAlert,
        onSelect: () => void confirmIgnorePluginRetry(only.taskId),
      })
    }
    if (only.source === 'local') {
      entries.push({
        type: 'item',
        key: 'boost',
        label: only.boosted ? t('cancelBoost') : t('boostDownload'),
        icon: Zap,
        onSelect: () => void toggleBoost(only.taskId),
      })
    }
  }
  if (views.every(isDownloadable)) {
    entries.push({
      type: 'item',
      key: 'download-file',
      label: t('webDownloadFile'),
      icon: Download,
      onSelect: () => downloadViewsFiles(views),
    })
  }
  if (only && canOpenTaskFile(only)) {
    entries.push({
      type: 'item',
      key: 'open-file',
      label: t('openFile'),
      icon: AppWindow,
      onSelect: () => void openTaskFile(only.taskId),
    })
  }
  if (only && canRevealTaskFile(only)) {
    entries.push({
      type: 'item',
      key: 'reveal-file',
      label: t('openFolder'),
      icon: FolderOpen,
      onSelect: () => void revealTaskFile(only.taskId),
    })
  }
  if (only && only.source === 'local') {
    entries.push({ type: 'item', key: 'rename', label: t('renameTask'), icon: Pen, onSelect: () => openRename(only.taskId) })
  }
  if (only && canChangeUrl(only)) {
    entries.push({ type: 'item', key: 'change-url', label: t('webChangeUrl'), icon: Link2, onSelect: () => openChangeUrl(only.taskId) })
  }
  if (only && canChangeSegments(only)) {
    entries.push({
      type: 'item',
      key: 'change-segments',
      label: t('editThreads'),
      icon: Layers,
      onSelect: () => openChangeSegments(only.taskId),
    })
  }
  if (views.every(canRedownload)) {
    entries.push({
      type: 'item',
      key: 'redownload',
      label: t('redownloadTask'),
      icon: RotateCw,
      onSelect: () => void redownloadViews(views),
    })
  }
  entries.push({ type: 'item', key: 'copy-url', label: t('copyUrl'), icon: Copy, onSelect: () => copyUrls(views) })
  if (allLocal && queues.length > 0) {
    entries.push({
      type: 'sub',
      key: 'move-to-queue',
      label: t('moveToQueueAction'),
      icon: Rows3,
      entries: queues.map((queue) => ({
        type: 'item' as const,
        key: `queue:${queue.queueId}`,
        label: queueName(queue.queueId),
        onSelect: () => void moveViewsToQueue(views, queue.queueId),
      })),
    })
  }
  // 与 GPUI `task_menu_entries` 对齐：未处于多选模式时在删除组前插入「选择 / 全选」。
  if (selection && !selection.multiSelect) {
    entries.push({ type: 'separator', key: 'sep-select' })
    entries.push({ type: 'item', key: 'select', label: t('selectTasks'), icon: CircleCheck, onSelect: selection.enterMultiSelect })
    entries.push({
      type: 'item',
      key: 'select-all',
      label: t('selectAll'),
      icon: SquareCheck,
      shortcut: IS_MAC ? '⌘A' : 'Ctrl+A',
      onSelect: selection.selectAll,
    })
  }
  entries.push({ type: 'separator', key: 'sep-delete' })
  entries.push({
    type: 'item',
    key: 'delete',
    label: t('deleteTask'),
    icon: Trash2,
    destructive: true,
    onSelect: () => void deleteViews(views, false),
  })
  entries.push({
    type: 'item',
    key: 'delete-with-files',
    label: t('deleteTaskAndFile'),
    icon: Trash2,
    destructive: true,
    onSelect: () => void confirmDeleteWithFiles(views),
  })
  const firstLocal = views.find((view) => view.source === 'local')
  if (firstLocal) {
    entries.push({
      type: 'item',
      key: 'detail',
      label: t('detail'),
      icon: PanelRight,
      onSelect: () => showDetail(firstLocal.taskId),
    })
  }
  return entries
}

/** 分组头右键菜单；`groupId` 为空（未分组桶）不显示菜单。 */
export function buildGroupMenu(
  t: Translate,
  groupId: string,
  summary: GroupSummary | undefined,
  allViews: readonly DownloadTaskView[],
): MenuEntry[] {
  if (groupId === '') return []
  const hasFailed = allViews.some((view) => view.source === 'local' && view.groupId === groupId && view.state === 'failed')
  const name = summary?.name ?? groupId
  const entries: MenuEntry[] = [
    { type: 'item', key: 'pause-all', label: t('groupPauseAll'), icon: Pause, onSelect: () => void pauseGroup(groupId) },
    { type: 'item', key: 'resume-all', label: t('groupResumeAll'), icon: Play, onSelect: () => void resumeGroup(groupId) },
  ]
  if (hasFailed) {
    entries.push({
      type: 'item',
      key: 'retry-failed',
      label: t('groupRetryFailed'),
      icon: RotateCw,
      onSelect: () => void retryFailedInGroup(allViews, groupId),
    })
  }
  if (summary && summary.originUrl !== '') {
    entries.push({
      type: 'item',
      key: 'copy-source',
      label: t('groupCopySourceLink'),
      icon: Copy,
      onSelect: () => copySourceLink(summary.originUrl),
    })
  }
  entries.push({ type: 'separator', key: 'sep-delete' })
  entries.push({
    type: 'item',
    key: 'delete',
    label: t('groupDelete'),
    icon: Trash2,
    destructive: true,
    onSelect: () => void deleteGroup(groupId, false),
  })
  entries.push({
    type: 'item',
    key: 'delete-with-files',
    label: t('groupDeleteWithFiles'),
    icon: Trash2,
    destructive: true,
    onSelect: () => void confirmDeleteGroupWithFiles(groupId, name),
  })
  entries.push({
    type: 'item',
    key: 'open-group',
    label: t('openGroupInWindowAction'),
    icon: AppWindow,
    onSelect: () => openGroupDetail(groupId),
  })
  return entries
}
