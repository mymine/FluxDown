import { describe, expect, test } from 'bun:test'
import { DYNAMIC_HOLD_MS, holdExpiresAt, POINTER_HOLD_MS } from './rowOrder'
import { mergeStable, resolveRowOrder } from './rowOrder.reference'
import type { OrderResult, OrderState } from './rowOrder.reference'

interface Row {
  key: string
  v: number
}

const rows = (...specs: [string, number][]): Row[] => specs.map(([key, v]) => ({ key, v }))
const keysOf = (result: OrderResult<Row>) => result.rows.map((row) => row.key)

interface Options {
  structural?: boolean
  force?: boolean
  now: number
  lastPointerAt?: number
  menuOpen?: boolean
  dynamicKey?: boolean
}

const resolve = (sorted: Row[], previous: OrderState | null, options: Options) =>
  resolveRowOrder({
    sorted,
    previous,
    structural: false,
    force: false,
    lastPointerAt: Number.NEGATIVE_INFINITY,
    menuOpen: false,
    dynamicKey: false,
    ...options,
  })

describe('稳定合并', () => {
  test('旧行的位置按旧顺序填入，新行落在新排序给出的位置，离开的行消失', () => {
    // 旧顺序 a b c d；新排序 d N a c（b 离开、N 是新行）
    const sorted = rows(['d', 1], ['N', 1], ['a', 1], ['c', 1])
    const merged = mergeStable(['a', 'b', 'c', 'd'], sorted)
    // 新排序里旧行占据的位置是 0、2、3 → 依次填入旧顺序的 a、c、d
    expect(merged.map((row) => row.key)).toEqual(['a', 'N', 'c', 'd'])
  })

  test('内容取新排序里的新对象', () => {
    const merged = mergeStable(['a', 'b'], rows(['b', 9], ['a', 7]))
    expect(merged).toEqual(rows(['a', 7], ['b', 9]))
  })
})

describe('保持期', () => {
  const first = resolve(rows(['a', 1], ['b', 1], ['c', 1]), null, { now: 0 })

  test('首次与结构性变化立即按新排序', () => {
    expect(keysOf(first)).toEqual(['a', 'b', 'c'])
    const structural = resolve(rows(['c', 1], ['b', 1], ['a', 1]), first.state, {
      now: 100,
      lastPointerAt: 90,
      structural: true,
    })
    expect(keysOf(structural)).toEqual(['c', 'b', 'a'])
    expect(structural.stale).toBe(false)
  })

  test('指针活动 1500ms 内仅内容变化不改变相对顺序，并标记过期', () => {
    const held = resolve(rows(['c', 2], ['a', 2], ['b', 2]), first.state, { now: 1000, lastPointerAt: 500 })
    expect(keysOf(held)).toEqual(['a', 'b', 'c'])
    expect(held.rows.every((row) => row.v === 2)).toBe(true)
    expect(held.stale).toBe(true)
  })

  test('保持期内新出现的行按新排序落位，旧行相对顺序不变', () => {
    const held = resolve(rows(['N', 1], ['c', 2], ['a', 2], ['b', 2]), first.state, { now: 1000, lastPointerAt: 900 })
    expect(keysOf(held)).toEqual(['N', 'a', 'b', 'c'])
    expect(held.stale).toBe(true)
  })

  test('保持期内离开筛选的行直接消失', () => {
    const held = resolve(rows(['c', 2], ['a', 2]), first.state, { now: 1000, lastPointerAt: 900 })
    expect(keysOf(held)).toEqual(['a', 'c'])
  })

  test('顺序没变时不标记过期', () => {
    const held = resolve(rows(['a', 2], ['b', 2], ['c', 2]), first.state, { now: 1000, lastPointerAt: 900 })
    expect(held.stale).toBe(false)
  })

  test('保持期结束后重排', () => {
    const sorted = rows(['c', 2], ['a', 2], ['b', 2])
    const later = resolve(sorted, first.state, { now: 500 + POINTER_HOLD_MS, lastPointerAt: 500 })
    expect(keysOf(later)).toEqual(['c', 'a', 'b'])
    expect(later.stale).toBe(false)
  })

  test('过期后的强制重排无视保持期', () => {
    const held = resolve(rows(['c', 2], ['a', 2], ['b', 2]), first.state, { now: 1000, lastPointerAt: 900 })
    const forced = resolve(rows(['c', 3], ['a', 3], ['b', 3]), held.state, { now: 1100, lastPointerAt: 1050, force: true })
    expect(keysOf(forced)).toEqual(['c', 'a', 'b'])
    expect(forced.stale).toBe(false)
  })

  test('菜单打开期间一直保持', () => {
    const held = resolve(rows(['c', 2], ['a', 2], ['b', 2]), first.state, { now: 60_000, menuOpen: true })
    expect(keysOf(held)).toEqual(['a', 'b', 'c'])
    expect(held.stale).toBe(true)
  })
})

describe('动态排序键保持期', () => {
  test('速度/进度键在上次重排后 2000ms 内保持，之后重排', () => {
    const first = resolve(rows(['a', 1], ['b', 1]), null, { now: 0, dynamicKey: true })
    const swapped = rows(['b', 2], ['a', 2])
    const held = resolve(swapped, first.state, { now: DYNAMIC_HOLD_MS - 1, dynamicKey: true })
    expect(keysOf(held)).toEqual(['a', 'b'])
    expect(held.stale).toBe(true)
    const released = resolve(swapped, held.state, { now: DYNAMIC_HOLD_MS, dynamicKey: true })
    expect(keysOf(released)).toEqual(['b', 'a'])
    expect(released.state.lastReorderAt).toBe(DYNAMIC_HOLD_MS)
  })

  test('保持期内顺序没变的内容刷新不顺延「上次重排」时刻', () => {
    const first = resolve(rows(['a', 1], ['b', 1]), null, { now: 0, dynamicKey: true })
    const same = resolve(rows(['a', 2], ['b', 2]), first.state, { now: 1500, dynamicKey: true })
    expect(same.state.lastReorderAt).toBe(0)
    const swapped = resolve(rows(['b', 3], ['a', 3]), same.state, { now: DYNAMIC_HOLD_MS, dynamicKey: true })
    expect(keysOf(swapped)).toEqual(['b', 'a'])
  })

  test('非动态键不受该保持期影响', () => {
    const first = resolve(rows(['a', 1], ['b', 1]), null, { now: 0 })
    const next = resolve(rows(['b', 2], ['a', 2]), first.state, { now: 10 })
    expect(keysOf(next)).toEqual(['b', 'a'])
  })

  test('保持期结束时刻取指针与动态保持的较晚者', () => {
    expect(holdExpiresAt({ lastPointerAt: 1000, dynamicKey: false, lastReorderAt: 0 })).toBe(1000 + POINTER_HOLD_MS)
    expect(holdExpiresAt({ lastPointerAt: 0, dynamicKey: true, lastReorderAt: 3000 })).toBe(3000 + DYNAMIC_HOLD_MS)
  })
})
