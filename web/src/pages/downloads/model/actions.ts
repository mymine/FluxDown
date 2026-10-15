// 下载页命令：对应 GPUI 的 DownloadsCommand（downloads_port.rs），全部走 agent /rpc。
// 本地任务 → `daemon.*`；远程任务 → `agent.remote.command`。失败统一 toast。
// 「打开文件 / 在文件夹中显示」只在连接带 `agent.openTaskFiles` 能力（agent 与浏览器同机）时提供，
// 其余情况由浏览器下载已完成文件替代。

import { copyText } from '../../../lib/copy'
import { t } from '../../../i18n'
import { CAPABILITY_AGENT_OPEN_TASK_FILES, RpcError, downloadTaskFile, rpc, rpcStore } from '../../../lib/rpc'
import type { CreateTaskRequest, RemoteCommandParams } from '../../../lib/rpc'
import { confirmDialog, toast } from '../../../ui'
import { toastRpcError } from '../../../lib/rpcToast'
import { planTaskCommand } from './batchPlan'
import { PLUGIN_ERROR_PREFIX, shareUrl } from './task'
import type { DownloadTaskView } from './task'

async function guarded(work: () => Promise<unknown>): Promise<boolean> {
  try {
    await work()
    return true
  } catch (error) {
    toastRpcError(error)
    return false
  }
}

async function guardedAll(jobs: readonly (() => Promise<unknown>)[]): Promise<boolean> {
  if (jobs.length === 0) return true
  const results = await Promise.allSettled(jobs.map((job) => job()))
  const failed = results.find((result) => result.status === 'rejected')
  if (failed && failed.status === 'rejected') {
    toastRpcError(failed.reason)
    return false
  }
  return true
}

function remoteCommand(view: DownloadTaskView, action: RemoteCommandParams['action'], deleteFiles = false) {
  return () =>
    rpc.agent.remote.command(
      action === 'delete' ? { taskId: view.taskId, action, deleteFiles } : { taskId: view.taskId, action },
    )
}

export function pauseViews(views: readonly DownloadTaskView[]): Promise<boolean> {
  const plan = planTaskCommand(views, 'pause')
  const [only] = plan.localIds
  const local = plan.batch
    ? [() => rpc.daemon.task.pauseMany(plan.localIds)]
    : only !== undefined
      ? [() => rpc.daemon.task.pause({ taskId: only })]
      : []
  return guardedAll([...local, ...plan.remote.map((view) => remoteCommand(view, 'pause'))])
}

/** 继续（失败任务同为 resume，UI 上叫重试）。 */
export function resumeViews(views: readonly DownloadTaskView[]): Promise<boolean> {
  const plan = planTaskCommand(views, 'resume')
  const [only] = plan.localIds
  const local = plan.batch
    ? [() => rpc.daemon.task.resumeMany(plan.localIds)]
    : only !== undefined
      ? [() => rpc.daemon.task.resume({ taskId: only })]
      : []
  return guardedAll([...local, ...plan.remote.map((view) => remoteCommand(view, 'resume'))])
}

export const pauseAll = () => guarded(() => rpc.daemon.task.pauseAll())
export const resumeAll = () => guarded(() => rpc.daemon.task.resumeAll())

export function deleteViews(views: readonly DownloadTaskView[], deleteFiles: boolean): Promise<boolean> {
  const plan = planTaskCommand(views, 'delete')
  const [only] = plan.localIds
  const local = plan.batch
    ? [() => rpc.daemon.task.deleteMany(plan.localIds, deleteFiles)]
    : only !== undefined
      ? [() => rpc.daemon.task.delete({ taskId: only, deleteFiles })]
      : []
  return guardedAll([...local, ...plan.remote.map((view) => remoteCommand(view, 'delete', deleteFiles))])
}

/** 「删除任务及文件」二次确认；确认后执行。 */
export async function confirmDeleteWithFiles(views: readonly DownloadTaskView[]): Promise<boolean> {
  if (views.length === 0) return false
  const [only] = views
  const description =
    views.length === 1 && only
      ? t('deleteConfirmDescWithFile', { fileName: only.name })
      : t('batchDeleteConfirmDescWithFile', { count: views.length })
  const ok = await confirmDialog({
    title: t('deleteTaskAndFile'),
    description,
    okLabel: t('delete'),
    intent: 'destructive',
  })
  return ok ? deleteViews(views, true) : false
}

export function moveViewsToQueue(views: readonly DownloadTaskView[], queueId: string): Promise<boolean> {
  return guardedAll(
    views
      .filter((view) => view.source === 'local')
      .map((view) => () => rpc.daemon.queue.moveTask({ taskId: view.taskId, queueId })),
  )
}

/** Boost：同 id 再次调用即取消。 */
export const toggleBoost = (taskId: string) => guarded(() => rpc.daemon.queue.boost({ id: taskId }))

/** 更换链接 / 修改线程数成功后，失败任务立即重试（暂停 / 等待保持原状态）；重试失败 toast。 */
export async function resumeIfFailed(taskId: string, wasFailed: boolean): Promise<boolean> {
  return wasFailed ? guarded(() => rpc.daemon.task.resume({ taskId })) : true
}

export function isPluginRetryError(view: DownloadTaskView): boolean {
  return view.state === 'failed' && view.errorMessage.startsWith(PLUGIN_ERROR_PREFIX)
}

export async function confirmIgnorePluginRetry(taskId: string): Promise<boolean> {
  const ok = await confirmDialog({
    title: t('taskIgnorePluginRetryTitle'),
    description: t('taskIgnorePluginRetryMsg'),
    okLabel: t('taskIgnorePluginRetry'),
  })
  return ok ? guarded(() => rpc.daemon.plugin.ignoreRetry({ taskId })) : false
}

/** 可重新下载：本地已完成 / 失败且非 `torrent-file://` 哨兵。 */
export function canRedownload(view: DownloadTaskView): boolean {
  return (
    view.source === 'local' &&
    (view.state === 'completed' || view.state === 'failed') &&
    !view.url.startsWith('torrent-file://')
  )
}

/** 删除（含文件）后按原参数重建。 */
export function redownloadViews(views: readonly DownloadTaskView[]): Promise<boolean> {
  return guardedAll(
    views.filter(canRedownload).map((view) => async () => {
      const dto = view.dto
      if (!dto) return
      const request: CreateTaskRequest = {
        url: dto.originUrl === '' ? dto.url : dto.originUrl,
        fileName: dto.fileName,
        saveDir: dto.saveDir,
        segments: 0,
        cookies: '',
        referrer: dto.referrer,
        proxyUrl: dto.proxyUrl,
        userAgent: '',
        queueId: dto.queueId,
        checksum: dto.checksum,
        ignoreTlsErrors: dto.ignoreTlsErrors,
        startPaused: false,
      }
      await rpc.daemon.task.delete({ taskId: dto.taskId, deleteFiles: true })
      await rpc.daemon.task.create({ request })
    }),
  )
}

export function copyUrls(views: readonly DownloadTaskView[]): void {
  const urls = views.map(shareUrl).filter((url) => url !== '')
  if (urls.length === 0) return
  copyText(urls.join('\n'))
  toast.key('urlCopied', 'success')
}

/** 只有本地已完成且文件仍在磁盘上的任务有可下载的最终文件。 */
export const isDownloadable = (view: DownloadTaskView): boolean =>
  view.source === 'local' && view.state === 'completed' && !view.fileMissing

/** 浏览器下载已完成文件（替代 GPUI 的「打开文件」）；多文件间隔触发以免被浏览器合并拦截。 */
export function downloadViewsFiles(views: readonly DownloadTaskView[]): void {
  views.filter(isDownloadable).forEach((view, index) => {
    setTimeout(() => downloadTaskFile(view.taskId), index * 400)
  })
}

/**
 * 本连接能否在 agent 所在主机上打开 / 定位任务产物：agent 按连接在 `system.hello` 下发
 * `agent.openTaskFiles`（桌面宿主、headless server 的字面本机来源或运维显式放行）。
 * 菜单在打开时构建，读最新握手结果即可，重连后自动跟随新连接。
 */
const hostOpensTaskFiles = (): boolean =>
  rpcStore.peek().hello?.capabilities.includes(CAPABILITY_AGENT_OPEN_TASK_FILES) ?? false

/** 「打开文件」：本地已完成且文件仍在磁盘上。 */
export const canOpenTaskFile = (view: DownloadTaskView): boolean => hostOpensTaskFiles() && isDownloadable(view)

/** 「在文件夹中显示」：任意本地任务（对齐 GPUI：未完成或文件缺失时定位临时文件 / 保存目录）。 */
export const canRevealTaskFile = (view: DownloadTaskView): boolean => hostOpensTaskFiles() && view.source === 'local'

/** 打开失败且文件已不在：立即重扫，让行上的丢失标记跟上磁盘现状（对齐 GPUI `rescan_files_now`）。 */
export async function openTaskFile(taskId: string): Promise<void> {
  try {
    await rpc.agent.platform.openTask({ taskId })
  } catch (error) {
    if (error instanceof RpcError && error.is('notFound')) rpc.daemon.task.rescan().catch(() => {})
    toastRpcError(error)
  }
}

export const revealTaskFile = (taskId: string) => guarded(() => rpc.agent.platform.revealTask({ taskId }))

// ── 任务组 ──

export const pauseGroup = (groupId: string) => guarded(() => rpc.daemon.group.pause({ groupId }))
export const resumeGroup = (groupId: string) => guarded(() => rpc.daemon.group.resume({ groupId }))

/** 组内失败成员逐个恢复。 */
export function retryFailedInGroup(views: readonly DownloadTaskView[], groupId: string): Promise<boolean> {
  return resumeViews(views.filter((view) => view.source === 'local' && view.groupId === groupId && view.state === 'failed'))
}

export const deleteGroup = (groupId: string, deleteFiles: boolean) =>
  guarded(() => rpc.daemon.group.delete({ groupId, deleteFiles }))

export async function confirmDeleteGroupWithFiles(groupId: string, groupName: string): Promise<boolean> {
  const ok = await confirmDialog({
    title: t('groupDeleteWithFiles'),
    description: t('deleteConfirmDescWithFile', { fileName: groupName }),
    okLabel: t('delete'),
    intent: 'destructive',
  })
  return ok ? deleteGroup(groupId, true) : false
}

/** 复制任务组来源链接。 */
export function copySourceLink(url: string): void {
  if (url === '') return
  copyText(url)
  toast.key('urlCopied', 'success')
}
