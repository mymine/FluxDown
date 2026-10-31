// 「本机做种」表单 → `daemon.task.seedLocal` 参数：去空白、拆分 tracker、必填校验。纯函数，便于单测。

import type { DaemonSeedLocalParams } from '../../../lib/rpc'

export interface SeedLocalForm {
  path: string
  torrentPath: string
  /** 每行一个 announce 地址。 */
  trackers: string
  unlimited: boolean
}

export type SeedLocalBuild = { ok: true; params: DaemonSeedLocalParams } | { ok: false; error: 'pathRequired' }

/** 指定了现成 .torrent 时忽略 trackers（种子自带）；空路径 → `pathRequired`。 */
export function buildSeedLocalParams(form: SeedLocalForm): SeedLocalBuild {
  const path = form.path.trim()
  if (path === '') return { ok: false, error: 'pathRequired' }
  const torrentPath = form.torrentPath.trim()
  const params: DaemonSeedLocalParams = { path, unlimited: form.unlimited }
  if (torrentPath !== '') {
    params.torrentPath = torrentPath
  } else {
    const trackers = form.trackers.split(/\r?\n/).map((line) => line.trim()).filter((line) => line !== '')
    if (trackers.length > 0) params.trackers = trackers
  }
  return { ok: true, params }
}

export type SeedLocalFeedback =
  | { key: 'seedLocalStarted'; level: 'success' }
  | { key: 'duplicateTorrentToast'; level: 'warning'; params: { name: string } }
  | { key: 'duplicateTorrentToastUnnamed'; level: 'warning' }

/** 结果 → 提示：重复（未新建任务）给警告，否则成功。 */
export function seedLocalFeedback(result: { duplicate?: boolean; existingName?: string }): SeedLocalFeedback {
  if (!result.duplicate) return { key: 'seedLocalStarted', level: 'success' }
  const name = result.existingName ?? ''
  if (name === '') return { key: 'duplicateTorrentToastUnnamed', level: 'warning' }
  return { key: 'duplicateTorrentToast', level: 'warning', params: { name } }
}
