// 外壳布局模型（纯函数）：
// 标题栏 / 活动栏 / 状态栏共六个区域（zone），条目可在允许的区域间拖动重排。
// normalize / move 的语义由共享用例 `crates/shell/tests/fixtures/chrome_layout_cases.json` 约束，
// GPUI 与 Web 两端都必须通过；改语义要同时改两端与用例。

/** 区域线上键，规范顺序（归一化时按此顺序消费存储值）。 */
export const CHROME_ZONES = ['titleLeading', 'titleTrailing', 'railTop', 'railBottom', 'statusLeading', 'statusTrailing'] as const
export type ChromeZone = (typeof CHROME_ZONES)[number]

/** 区域族：条目声明自己能放进哪些族的区域。 */
export type ChromeFamily = 'rail' | 'title' | 'status'

export interface ChromeItemSpec {
  id: string
  defaultZone: ChromeZone
  families: readonly ChromeFamily[]
}

/** 完整布局：六个区域一个不缺。 */
export type ChromeLayout = Readonly<Record<ChromeZone, readonly string[]>>

export function zoneFamily(zone: ChromeZone): ChromeFamily {
  if (zone.startsWith('title')) return 'title'
  if (zone.startsWith('rail')) return 'rail'
  return 'status'
}

/** 条目能否放进该区域（按族判定）。 */
export function itemAllowedIn(spec: ChromeItemSpec, zone: ChromeZone): boolean {
  return spec.families.includes(zoneFamily(zone))
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/**
 * 存储值 → 完整布局：
 * 1. 非普通对象 → 默认布局；
 * 2. 按规范区域顺序，数组区域内仅保留「字符串、目录内 id、尚未放置、允许放入该区域」的元素；未知区域键与非数组值忽略；
 * 3. 未被放置的目录条目按目录顺序追加到各自默认区域末尾。
 */
export function normalizeLayout(stored: unknown, catalog: readonly ChromeItemSpec[]): ChromeLayout {
  const specs = new Map(catalog.map((spec) => [spec.id, spec]))
  const placed = new Set<string>()
  const zones: Record<ChromeZone, string[]> = {
    titleLeading: [],
    titleTrailing: [],
    railTop: [],
    railBottom: [],
    statusLeading: [],
    statusTrailing: [],
  }
  if (isPlainObject(stored)) {
    for (const zone of CHROME_ZONES) {
      const items = stored[zone]
      if (!Array.isArray(items)) continue
      for (const id of items) {
        if (typeof id !== 'string' || placed.has(id)) continue
        const spec = specs.get(id)
        if (!spec || !itemAllowedIn(spec, zone)) continue
        placed.add(id)
        zones[zone].push(id)
      }
    }
  }
  for (const spec of catalog) {
    if (!placed.has(spec.id)) zones[spec.defaultZone].push(spec.id)
  }
  return zones
}

export function defaultLayout(catalog: readonly ChromeItemSpec[]): ChromeLayout {
  return normalizeLayout(null, catalog)
}

function sameLayout(left: ChromeLayout, right: ChromeLayout): boolean {
  return CHROME_ZONES.every((zone) => {
    const a = left[zone]
    const b = right[zone]
    return a.length === b.length && a.every((id, index) => id === b[index])
  })
}

/**
 * 把 `id` 移到 `zone`：
 * - `id` 未知或不允许放入 `zone` → 不变；`target === id` → 不变；
 * - `target` 非空且当前在 `zone` 内：按移除前的下标 `ti` 取位，移除 `id` 后插到 `ti`
 *   （同区域向后拖落在 target 之后，向前 / 跨区域落在 target 之前）；
 * - 否则（`null` 或 target 不在该区域）移除后追加到 `zone` 末尾。
 */
export function moveItem(
  layout: ChromeLayout,
  catalog: readonly ChromeItemSpec[],
  id: string,
  zone: ChromeZone,
  target: string | null,
): { layout: ChromeLayout; changed: boolean } {
  const spec = catalog.find((item) => item.id === id)
  if (!spec || !itemAllowedIn(spec, zone) || target === id) return { layout, changed: false }
  const ti = target === null ? -1 : layout[zone].indexOf(target)
  const next = {} as Record<ChromeZone, string[]>
  for (const name of CHROME_ZONES) next[name] = layout[name].filter((item) => item !== id)
  if (ti >= 0) next[zone].splice(ti, 0, id)
  else next[zone].push(id)
  const changed = !sameLayout(layout, next)
  return { layout: changed ? next : layout, changed }
}

/** 序列化为存储值：始终写全六个区域。 */
export function serializeLayout(layout: ChromeLayout): Record<ChromeZone, string[]> {
  return {
    titleLeading: [...layout.titleLeading],
    titleTrailing: [...layout.titleTrailing],
    railTop: [...layout.railTop],
    railBottom: [...layout.railBottom],
    statusLeading: [...layout.statusLeading],
    statusTrailing: [...layout.statusTrailing],
  }
}
