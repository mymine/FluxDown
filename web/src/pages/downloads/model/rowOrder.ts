// 行顺序稳定：防止行在光标下跳动（GPUI 同一套语义）。
//
// - 结构性变化（任务增删、筛选/搜索/排序偏好变化、点表头）立即全量重排；
// - 仅内容变化（进度、速度、状态…）时，若处于「保持期」则不改变已有行的相对顺序（稳定合并），
//   新排序与已应用顺序不同即「顺序过期」，由调用方在保持期结束后强制全量重排一次。
// 保持期 = 表格内最近指针活动 < 1500ms，或排序键是动态键（速度 / 进度）且距上次真正重排 < 2000ms；
// 右键菜单 / 下拉菜单打开期间保持期顺延。

import type { ViewSortKey } from './viewPrefs'

export const POINTER_HOLD_MS = 1500
export const DYNAMIC_HOLD_MS = 2000

/** 值随下载实时变化的排序键。 */
export function isDynamicSortKey(key: ViewSortKey): boolean {
  return key === 'speed' || key === 'progress'
}

/** 表格内最近一次指针活动（mousemove / wheel / mousedown / contextmenu）。模块级单例：表格写、状态层读。 */
export const pointerActivity = { lastAt: Number.NEGATIVE_INFINITY }

export function notePointerActivity(now: number = Date.now()): void {
  pointerActivity.lastAt = now
}

/** 有菜单（右键菜单 / 下拉）处于打开状态。 */
export function isMenuOpen(): boolean {
  return typeof document !== 'undefined' && document.querySelector('[role="menu"][data-state="open"]') !== null
}

export interface HoldInput {
  now: number
  lastPointerAt: number
  menuOpen: boolean
  dynamicKey: boolean
  lastReorderAt: number
}

/** 保持期是否仍在生效。 */
export function isHolding(input: HoldInput): boolean {
  if (input.menuOpen) return true
  if (input.now - input.lastPointerAt < POINTER_HOLD_MS) return true
  return input.dynamicKey && input.now - input.lastReorderAt < DYNAMIC_HOLD_MS
}

/** 保持期（不含菜单顺延）的结束时刻；已结束则 ≤ now。 */
export function holdExpiresAt(input: Pick<HoldInput, 'lastPointerAt' | 'dynamicKey' | 'lastReorderAt'>): number {
  const pointer = input.lastPointerAt + POINTER_HOLD_MS
  const dynamic = input.dynamicKey ? input.lastReorderAt + DYNAMIC_HOLD_MS : Number.NEGATIVE_INFINITY
  return Math.max(pointer, dynamic)
}
