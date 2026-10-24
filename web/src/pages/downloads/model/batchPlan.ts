// 下载页命令的分派规划（纯函数）：按来源拆分本地 / 远程任务；本地任务 ≥2 个时合并为一次批量 RPC，
// 避免逐任务并发请求撑满 agent 的 daemon 通道并造成 N 次快照重发。

import type { RemoteCommandParams } from '../../../lib/rpc'
import { canResumeSeeding } from './task'
import type { DownloadTaskView } from './task'

export type LocalTaskAction = 'pause' | 'resume' | 'delete'

/**
 * 远程任务的云端命令是否适用（镜像 crates/downloads/src/model/dispatch.rs
 * `remote_action_applies`）：状态未知一律不控制；Pause 只对进行中，Resume 只对已暂停；
 * Delete 对任何已知状态成立。本地任务恒为 true。
 */
export function remoteCan(view: DownloadTaskView, action: RemoteCommandParams['action']): boolean {
  if (view.source === 'local') return true
  switch (view.remoteStatus) {
    case null:
    case 'unknown':
      return false
    case 'pending':
    case 'accepted':
    case 'downloading':
      return action === 'pause' || action === 'cancel' || action === 'delete'
    case 'paused':
      return action === 'resume' || action === 'cancel' || action === 'delete'
    default:
      return action === 'delete'
  }
}

export interface TaskCommandPlan {
  /** 本地任务 id（去重、保持选择顺序）；单个走单任务方法，多个走批量方法。 */
  localIds: string[]
  batch: boolean
  /** 适用的远程任务，逐个经 `agent.remote.command` 发送。 */
  remote: DownloadTaskView[]
}

/**
 * 已完成的本地任务只有处于可恢复做种态（`canResumeSeeding`）才参与「继续」：引擎对其余完成态的
 * resume 会从头重新下载（#935，镜像 crates/downloads `task_command`）。
 */
export function planTaskCommand(views: readonly DownloadTaskView[], action: LocalTaskAction): TaskCommandPlan {
  const localIds: string[] = []
  const seen = new Set<string>()
  const remote: DownloadTaskView[] = []
  for (const view of views) {
    if (!remoteCan(view, action)) continue
    if (view.source === 'local') {
      if (action === 'resume' && view.state === 'completed' && !canResumeSeeding(view)) continue
      if (!seen.has(view.taskId)) {
        seen.add(view.taskId)
        localIds.push(view.taskId)
      }
    } else {
      remote.push(view)
    }
  }
  return { localIds, batch: localIds.length > 1, remote }
}
