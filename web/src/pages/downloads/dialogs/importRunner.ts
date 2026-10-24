// 批量导入的分块提交：对齐 GPUI `crates/downloads/src/submission.rs` 的 `run_submission`。
//
// - 按 `IMPORT_CHUNK_SIZE`（256）分块，每块一次幂等的 `daemon.task.createMany`，每块一个全新 batchId；
// - 结果不确定（`unavailable` / `timeout`，含客户端 30s 超时）时用**同一个** batchId 指数退避重发
//   （2s / 4s / 8s，最多 3 次）；用尽后该块计失败，后续分块不再发送（计「未提交」）；
// - 取消在每块发出前检查，并与在途分块竞速：在途请求再给 3s 收尾，仍无结果则计「未提交」（RPC 无法中止）；
// - 已创建的任务保留（不回滚）；每块结束 / 进入或离开重试等待时回调一次进度。
//
// 运行时无关：调用、睡眠、取消信号都由调用方注入，便于单测。

import { RpcError } from '../../../lib/rpc/error'
import type { CategoryRouteDto, CreateTaskRequest, DaemonCreateTasksParams, DaemonCreateTasksResult, CreateTaskItem, RpcErrorData } from '../../../lib/rpc/protocol'
import { randomUuid } from '../../../lib/uuid'

/** 每次 `createMany` 的条目数（协议上限 512 的一半）。 */
export const IMPORT_CHUNK_SIZE = 256
const MAX_RETRIES = 3
const RETRY_BASE_DELAY_MS = 2_000
const CANCEL_GRACE_MS = 3_000

/** 等待重发时的状态（供界面显示）。 */
export interface RetryState {
  attempt: number
  max: number
  delaySeconds: number
}

/** 分块提交进度：每块结束 / 进入或离开重试等待时回调一次。 */
export interface ImportProgress {
  total: number
  done: number
  failed: number
  retry: RetryState | null
}

/** 单条结果：与输入 items 同序。 */
export type ItemOutcome =
  | { status: 'created'; taskId: string }
  | { status: 'failed'; error: RpcErrorData }
  | { status: 'notSubmitted' }

export interface ImportReport {
  outcomes: ItemOutcome[]
  cancelled: boolean
  succeeded: number
  failed: number
  notSubmitted: number
  /** 第一个失败条目的错误（用于提示文案）。 */
  firstError: RpcErrorData | null
}

export interface ImportOptions {
  common: CreateTaskRequest
  items: readonly CreateTaskItem[]
  /** 分类目录路由，随每块 `daemon.task.createMany` 下发。 */
  categoryRoute?: CategoryRouteDto | null
  call: (params: DaemonCreateTasksParams) => Promise<DaemonCreateTasksResult>
  sleep: (ms: number) => Promise<void>
  signal?: AbortSignal
  onProgress?: (progress: ImportProgress) => void
  /** 每块的幂等键生成器；默认 `randomUuid()`（非安全上下文也可用）。 */
  newBatchId?: () => string
}

type ChunkResult = { ok: true; value: DaemonCreateTasksResult } | { ok: false; error: RpcErrorData }

type ChunkEnd =
  | { kind: 'settled'; result: ChunkResult }
  | { kind: 'cancelled'; result: ChunkResult | null }

function isUncertain(error: RpcErrorData): boolean {
  return error.code === 'unavailable' || error.code === 'timeout'
}

function internalError(): RpcErrorData {
  return { code: 'internal', retryable: false }
}

/** 单次调用 → 结果（不抛出，含同步异常）；RpcError 带稳定错误详情，其余异常按内部错误处理。 */
function callOnce(send: () => Promise<DaemonCreateTasksResult>): Promise<ChunkResult> {
  return Promise.resolve()
    .then(send)
    .then(
      (value): ChunkResult => ({ ok: true, value }),
      (error: unknown): ChunkResult => ({ ok: false, error: error instanceof RpcError && error.data ? error.data : internalError() }),
    )
}

/** 取消信号 → Promise（已取消则立即就绪）。 */
function whenAborted(signal: AbortSignal | undefined): Promise<void> {
  if (!signal) return new Promise<void>(() => undefined)
  if (signal.aborted) return Promise.resolve()
  return new Promise<void>((resolve) => signal.addEventListener('abort', () => resolve(), { once: true }))
}

/** 与取消竞速；取消后给在途请求一小段收尾时间。同时就绪时请求结果优先（已落库的结果不被取消吞掉）。 */
async function settle(request: Promise<ChunkResult>, options: ImportOptions): Promise<ChunkEnd> {
  const first = await Promise.race([request.then((result) => ({ tag: 'done' as const, result })), whenAborted(options.signal).then(() => ({ tag: 'cancelled' as const }))])
  if (first.tag === 'done') return { kind: 'settled', result: first.result }
  const grace = await Promise.race([request.then((result) => ({ tag: 'done' as const, result })), options.sleep(CANCEL_GRACE_MS).then(() => ({ tag: 'timeout' as const }))])
  return { kind: 'cancelled', result: grace.tag === 'done' ? grace.result : null }
}

/** 提交一块并在结果不确定时用同一个 batchId 退避重发。 */
async function submitChunk(params: DaemonCreateTasksParams, options: ImportOptions, onRetry: (retry: RetryState | null) => void): Promise<ChunkEnd> {
  let attempt = 0
  for (;;) {
    const end = await settle(callOnce(() => options.call(params)), options)
    if (end.kind === 'cancelled') return end
    const { result } = end
    if (result.ok || !isUncertain(result.error) || attempt >= MAX_RETRIES) return end
    const delay = RETRY_BASE_DELAY_MS * 2 ** attempt
    attempt += 1
    onRetry({ attempt, max: MAX_RETRIES, delaySeconds: delay / 1000 })
    const aborted = await Promise.race([options.sleep(delay).then(() => false), whenAborted(options.signal).then(() => true)])
    if (aborted) return { kind: 'cancelled', result: null }
    onRetry(null)
  }
}

/** 按块提交全部条目；取消 / 重发用尽后放弃剩余分块，已创建的任务保留。 */
export async function runImport(options: ImportOptions): Promise<ImportReport> {
  const { items, common } = options
  const total = items.length
  const outcomes: ItemOutcome[] = new Array<ItemOutcome>(total).fill({ status: 'notSubmitted' })
  const report: ImportReport = { outcomes, cancelled: false, succeeded: 0, failed: 0, notSubmitted: 0, firstError: null }
  const newBatchId = options.newBatchId ?? randomUuid

  const fail = (index: number, error: RpcErrorData) => {
    outcomes[index] = { status: 'failed', error }
    report.failed += 1
    report.firstError ??= error
  }
  const record = (start: number, length: number, value: DaemonCreateTasksResult) => {
    for (let offset = 0; offset < length; offset += 1) {
      const outcome = value.results[offset]
      if (outcome?.taskId) {
        outcomes[start + offset] = { status: 'created', taskId: outcome.taskId }
        report.succeeded += 1
      } else {
        // 缺失或空结果按内部错误计失败。
        fail(start + offset, outcome?.error ?? internalError())
      }
    }
  }
  const failChunk = (start: number, length: number, error: RpcErrorData) => {
    for (let offset = 0; offset < length; offset += 1) fail(start + offset, error)
  }

  let position = 0
  while (position < total) {
    if (options.signal?.aborted) {
      report.cancelled = true
      break
    }
    const length = Math.min(IMPORT_CHUNK_SIZE, total - position)
    const start = position
    position += length
    const params: DaemonCreateTasksParams = {
      batchId: newBatchId(),
      common,
      items: items.slice(start, position) as CreateTaskItem[],
      ...(options.categoryRoute ? { categoryRoute: options.categoryRoute } : {}),
    }
    const base = { total, done: report.succeeded + report.failed, failed: report.failed }
    const end = await submitChunk(params, options, (retry) => options.onProgress?.({ ...base, retry }))
    let stop = false
    if (end.kind === 'settled') {
      if (end.result.ok) record(start, length, end.result.value)
      else {
        failChunk(start, length, end.result.error)
        // 重发用尽仍不确定：服务不可达，继续后面的块只会重复同样的等待。
        stop = isUncertain(end.result.error)
      }
    } else {
      report.cancelled = true
      stop = true
      const result = end.result
      if (result?.ok) record(start, length, result.value)
      else if (result && !isUncertain(result.error)) failChunk(start, length, result.error)
    }
    options.onProgress?.({ total, done: report.succeeded + report.failed, failed: report.failed, retry: null })
    if (stop) break
  }

  report.notSubmitted = outcomes.filter((outcome) => outcome.status === 'notSubmitted').length
  return report
}

/** 提交完成后的提示：`{ kind, added, total, failed, error }` 由界面套 i18n。 */
export interface ImportNotice {
  kind: 'created' | 'cancelled' | 'partial' | 'failed'
  added: number
  total: number
  failed: number
  error: RpcErrorData | null
}

export function importNotice(report: ImportReport): ImportNotice {
  const added = report.succeeded
  const total = added + report.failed + report.notSubmitted
  const error = report.firstError
  if (report.cancelled) return { kind: 'cancelled', added, total, failed: report.failed, error }
  if (error === null) return { kind: 'created', added, total, failed: 0, error: null }
  return { kind: added === 0 ? 'failed' : 'partial', added, total, failed: report.failed, error }
}
