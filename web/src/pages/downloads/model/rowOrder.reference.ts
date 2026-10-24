// 仅供测试使用：旧的整表「行顺序稳定」实现（对每帧排好序的全表做稳定合并），即行引擎 `rowEngine.ts` 的规格。
// 生产路径走增量的 `RowEngine`；`rowOrder.test.ts` 钉住规格本身，`rowEngine.test.ts` 让引擎与它逐帧对拍。

import { isHolding } from './rowOrder'
import type { HoldInput } from './rowOrder'

export interface OrderState {
  /** 上一次应用的行 key 顺序。 */
  keys: readonly string[]
  /** 上一次真正改变顺序的时刻（ms）。 */
  lastReorderAt: number
}

/**
 * 稳定合并：`sorted` 里属于旧行的位置按旧顺序依次填入旧行（内容取 `sorted` 里的新对象），
 * 新出现的行落在 `sorted` 给出的位置；离开的旧行直接消失。
 */
export function mergeStable<T extends { key: string }>(previousKeys: readonly string[], sorted: readonly T[]): T[] {
  const byKey = new Map<string, T>()
  for (const row of sorted) byKey.set(row.key, row)
  const previousSet = new Set(previousKeys)
  const queue: T[] = []
  for (const key of previousKeys) {
    const row = byKey.get(key)
    if (row) queue.push(row)
  }
  let cursor = 0
  return sorted.map((row) => (previousSet.has(row.key) ? (queue[cursor++] as T) : row))
}

function sameKeys(left: readonly string[], right: readonly { key: string }[]): boolean {
  return left.length === right.length && left.every((key, index) => key === right[index]?.key)
}

export interface OrderInput<T extends { key: string }> extends Omit<HoldInput, 'lastReorderAt'> {
  /** 按当前排序键排好的新结果。 */
  sorted: readonly T[]
  previous: OrderState | null
  /** 结构性变化：立即全量重排。 */
  structural: boolean
  /** 保持期结束后的强制重排。 */
  force: boolean
}

export interface OrderResult<T> {
  rows: T[]
  state: OrderState
  /** 已应用顺序与新排序不同（需要在保持期结束后重排）。 */
  stale: boolean
}

export function resolveRowOrder<T extends { key: string }>(input: OrderInput<T>): OrderResult<T> {
  const { sorted, previous } = input
  const holding =
    previous !== null &&
    !input.structural &&
    !input.force &&
    isHolding({ ...input, lastReorderAt: previous.lastReorderAt })
  const rows = previous !== null && holding ? mergeStable(previous.keys, sorted) : sorted.slice()
  const keys = rows.map((row) => row.key)
  // 只有顺序真正变了才刷新「上次重排」时刻，否则动态键下每次内容刷新都会无限顺延保持期。
  const reordered = previous === null || !sameKeys(previous.keys, rows)
  return {
    rows,
    state: { keys, lastReorderAt: reordered ? input.now : (previous?.lastReorderAt ?? input.now) },
    stale: holding && !sameKeys(keys, sorted),
  }
}
