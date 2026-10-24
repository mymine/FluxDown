// 任务表 / 队列位置表的分页载入状态机（对应 Rust `fluxdown_protocol::TableSync`，传输无关）。
//
// 一轮载入从某个基准帧（快照或宣告重载的事件）开始：调用方先把基准帧应用到投影，之后到达的帧缓冲
// 不应用；本状态机负责「先任务、后队列位置」逐页翻到末页；全部页到齐后调用方装入表
// （`apply.ts` 的 `installTasks` / `installQueue`），再以基准序号为游标回放缓冲帧。
//
// 游标：任务 = 上一页最后一个 taskId，队列 = 上一页最后一个 order；都与翻页期间的增删无关。
// 页读取发生在基准帧之后的不同时刻，回放仍然收敛：任务事件都是整行 / 绝对值写入，回放完全部后续帧后
// 每个任务取自它最后一次被触及的帧，没被触及的取自页内容；任务按 id 唯一，页内容天然无重复。

import type { DaemonSnapshot, QueueOrderEntryDto, TablePage, TablePageParams, TableSection, TaskDto } from './protocol'

/** 载入失败的原因：调用方一律放弃本轮并整体重同步（重连 / 重新拉快照）。 */
export type TableSyncFailure =
  /** 页所属 epoch 与基准帧不同：服务端已重启。 */
  | 'epochChanged'
  /** 页的分区与请求不符。 */
  | 'unexpectedSection'
  /** 服务端返回了与请求相同的游标：翻页不前进。 */
  | 'stalled'
  /** 缓冲帧超过 `TABLE_SYNC_BACKLOG_LIMIT`。 */
  | 'backlogOverflow'

export class TableSyncError extends Error {
  constructor(readonly failure: TableSyncFailure) {
    super(`table sync failed: ${failure}`)
    this.name = 'TableSyncError'
  }
}

interface Section<T> {
  wanted: boolean
  done: boolean
  cursor: string | undefined
  rows: T[]
}

function section<T>(total: number | undefined): Section<T> {
  return { wanted: total !== undefined, done: total === undefined, cursor: undefined, rows: [] }
}

function accept<T>(target: Section<T>, rows: readonly T[], requested: string | undefined, next: string | undefined): void {
  if (next !== undefined && next === requested) throw new TableSyncError('stalled')
  for (const row of rows) target.rows.push(row)
  target.done = next === undefined
  target.cursor = next
}

/** 载入完成后的产物（`TableLoad.parts`）。 */
export interface LoadedTables {
  /** 有值 = 任务表已载入（按 id 升序）。 */
  tasks: TaskDto[] | null
  /** 有值 = 队列位置表已载入（按 `order` 升序）。 */
  queue: QueueOrderEntryDto[] | null
}

/** 一轮分页载入。 */
export class TableLoad {
  private readonly tasks: Section<TaskDto>
  private readonly queue: Section<QueueOrderEntryDto>
  private requested: { section: TableSection; cursor: string | undefined } | null = null

  /** `epoch` 为基准帧所属 epoch；两个总数都缺省时无需载入（`wanted === false`）。 */
  constructor(
    readonly epoch: string,
    taskTotal: number | undefined,
    queueTotal: number | undefined,
  ) {
    this.tasks = section(taskTotal)
    this.queue = section(queueTotal)
  }

  /** 按投影里的待载入标记建立载入；无标记返回 null。 */
  static forSnapshot(epoch: string, snapshot: DaemonSnapshot): TableLoad | null {
    if (snapshot.taskTotal === undefined && snapshot.queueTotal === undefined) return null
    return new TableLoad(epoch, snapshot.taskTotal, snapshot.queueTotal)
  }

  /** 已载入任务行数（加载态显示进度）。 */
  get loadedTasks(): number {
    return this.tasks.rows.length
  }

  /** 下一个要发的页请求；全部载入完毕返回 null。任务表先于队列位置表。 */
  nextRequest(): TablePageParams | null {
    let target: TableSection
    let cursor: string | undefined
    if (this.tasks.wanted && !this.tasks.done) {
      target = 'tasks'
      cursor = this.tasks.cursor
    } else if (this.queue.wanted && !this.queue.done) {
      target = 'queue'
      cursor = this.queue.cursor
    } else {
      this.requested = null
      return null
    }
    this.requested = { section: target, cursor }
    return cursor === undefined ? { section: target } : { section: target, cursor }
  }

  /** 吸收最近一次 `nextRequest` 对应的页。 */
  accept(page: TablePage): void {
    if (page.epoch !== this.epoch) throw new TableSyncError('epochChanged')
    const requested = this.requested
    this.requested = null
    if (requested === null || page.section !== requested.section) throw new TableSyncError('unexpectedSection')
    if (requested.section === 'tasks') accept(this.tasks, page.tasks, requested.cursor, page.nextCursor)
    else accept(this.queue, page.queue, requested.cursor, page.nextCursor)
  }

  /** 全部页到齐后取出产物。 */
  parts(): LoadedTables {
    return {
      tasks: this.tasks.wanted ? this.tasks.rows : null,
      queue: this.queue.wanted ? this.queue.rows : null,
    }
  }
}
