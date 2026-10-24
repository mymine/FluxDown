// 侧栏计数（对应 crates/downloads/src/model/counts.rs）：一次扫描得出「状态 × 分类」「队列」「设备」
// 全部桶的任务数。逐项按 `filterMatches` 计数是 O(项数 × 任务数)；这里每个任务只算一次状态位与分类命中，
// 再累加进各桶，结果与逐项计数逐一相同。行引擎（`rowEngine.ts`）对任务的增删改走 `add` / `remove`
// 增量维护，不必为一条进度帧重扫全表。

import type { CategoryIndex } from './categories'
import { LOCAL_DEVICE, STATUS_FILTERS, statusMatches } from './filters'
import type { DownloadFilter } from './filters'
import { isSeeding } from './task'
import type { DownloadTaskView, TaskState } from './task'

/** 任务状态命中的状态桶下标（`STATUS_FILTERS` 的下标）。 */
const slotsOf = (state: TaskState, seeding: boolean): readonly number[] =>
  STATUS_FILTERS.flatMap((status, slot) => (statusMatches(status, state, seeding) ? [slot] : []))

const STATE_SLOTS: Record<TaskState, readonly number[]> = {
  pending: slotsOf('pending', false),
  downloading: slotsOf('downloading', false),
  paused: slotsOf('paused', false),
  completed: slotsOf('completed', false),
  failed: slotsOf('failed', false),
}

/** 做种中的任务（必为已完成）额外命中「做种中」桶。 */
const SEEDING_SLOTS: readonly number[] = slotsOf('completed', true)

export class SidebarCounts {
  /** 不限分类时各状态的任务数。 */
  private byStatus: number[] = STATUS_FILTERS.map(() => 0)
  /** 分类 id → 各状态的任务数；只含至少命中过一个任务的分类（减到 0 的分类保留空桶，读数同为 0）。 */
  private byCategory = new Map<string, number[]>()
  /** 本地任务按队列。 */
  private byQueue = new Map<string, number>()
  /** 本机（`LOCAL_DEVICE`）与远程任务的目标设备。 */
  private byDevice = new Map<string, number>()

  constructor(
    views: readonly DownloadTaskView[],
    private readonly categories: CategoryIndex,
  ) {
    for (const view of views) this.add(view)
  }

  /** 计入一个任务。 */
  add(view: DownloadTaskView): void {
    this.tally(view, 1)
  }

  /** 撤销一个先前 `add` 过的任务（须传入当时那份视图）。 */
  remove(view: DownloadTaskView): void {
    this.tally(view, -1)
  }

  /** 深拷贝：桶数量只与状态 / 分类 / 队列 / 设备数有关，与任务数无关。 */
  clone(): SidebarCounts {
    const copy = new SidebarCounts([], this.categories)
    copy.byStatus = this.byStatus.slice()
    for (const [id, buckets] of this.byCategory) copy.byCategory.set(id, buckets.slice())
    copy.byQueue = new Map(this.byQueue)
    copy.byDevice = new Map(this.byDevice)
    return copy
  }

  private tally(view: DownloadTaskView, delta: 1 | -1): void {
    const slots = isSeeding(view) ? SEEDING_SLOTS : STATE_SLOTS[view.state]
    for (const slot of slots) this.byStatus[slot] = (this.byStatus[slot] as number) + delta
    this.categories.forEachMatch(view, (categoryId) => {
      let buckets = this.byCategory.get(categoryId)
      if (buckets === undefined) {
        buckets = STATUS_FILTERS.map(() => 0)
        this.byCategory.set(categoryId, buckets)
      }
      for (const slot of slots) buckets[slot] = (buckets[slot] as number) + delta
    })
    if (view.source === 'local') {
      bump(this.byQueue, view.queueId, delta)
      bump(this.byDevice, LOCAL_DEVICE, delta)
    } else {
      bump(this.byDevice, view.toDevice, delta)
    }
  }

  /** 与 `views.filter((view) => filterMatches(filter, view, categories)).length` 相同。 */
  filter(filter: DownloadFilter): number {
    const slot = STATUS_FILTERS.indexOf(filter.status)
    if (filter.category === null) return this.byStatus[slot] ?? 0
    return this.byCategory.get(filter.category)?.[slot] ?? 0
  }

  queue(queueId: string): number {
    return this.byQueue.get(queueId) ?? 0
  }

  device(deviceId: string): number {
    return this.byDevice.get(deviceId) ?? 0
  }
}

function bump(map: Map<string, number>, key: string, delta: 1 | -1): void {
  map.set(key, (map.get(key) ?? 0) + delta)
}
