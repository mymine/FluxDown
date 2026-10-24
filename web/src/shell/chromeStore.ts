// 外壳布局存储：仅存浏览器 localStorage（设备本地，不发给 agent、不参与同步；GPUI 自有一份）。
// 存储不可用（隐私模式等）时退化为内存默认布局，本次会话内仍可调整。
// 另含编辑态（是否在编辑、当前拖动项、悬停落点）：纯瞬时 UI 状态，不持久化。

import { useSyncExternalStore } from 'react'
import { getChromeCatalog, subscribeChromeCatalog } from './chromeCatalog'
import { defaultLayout, moveItem, normalizeLayout, serializeLayout, setHidden } from './chromeLayout'
import type { ChromeLayout, ChromeZone } from './chromeLayout'

export const CHROME_LAYOUT_KEY = 'fluxdown.chromeLayout'

type Listener = () => void

// ── 布局 ──

const layoutListeners = new Set<Listener>()

/** 持久化的原始布局（`undefined` = 尚未读取）；插件条目晚到 / 重装时用它重新归一化，从不因目录变化而写回。 */
let storedRaw: unknown

function readStoredRaw(): unknown {
  if (storedRaw !== undefined) return storedRaw
  try {
    const raw = localStorage.getItem(CHROME_LAYOUT_KEY)
    storedRaw = raw === null ? null : JSON.parse(raw)
  } catch {
    storedRaw = null
  }
  return storedRaw
}

let layout: ChromeLayout | null = null
let layoutCatalog: unknown = null

export function getChromeLayout(): ChromeLayout {
  const catalog = getChromeCatalog()
  if (layout === null || layoutCatalog !== catalog) {
    layout = normalizeLayout(readStoredRaw(), catalog)
    layoutCatalog = catalog
  }
  return layout
}

function persist(next: ChromeLayout): void {
  layout = next
  layoutCatalog = getChromeCatalog()
  storedRaw = serializeLayout(next)
  try {
    localStorage.setItem(CHROME_LAYOUT_KEY, JSON.stringify(serializeLayout(next)))
  } catch {
    // 存储不可用：本次会话内以内存布局为准。
  }
  for (const listener of layoutListeners) listener()
}

/** 移动一个条目并在成功时立即持久化；返回是否发生了变化。 */
export function moveChromeItem(id: string, zone: ChromeZone, target: string | null): boolean {
  const result = moveItem(getChromeLayout(), getChromeCatalog(), id, zone, target)
  if (result.changed) persist(result.layout)
  return result.changed
}

/** 隐藏 / 显示一个条目并在成功时立即持久化（摆放位置不变；固定 / 未知条目被拒绝）；返回是否发生了变化。 */
export function setChromeItemHidden(id: string, hidden: boolean): boolean {
  const result = setHidden(getChromeLayout(), getChromeCatalog(), id, hidden)
  if (result.changed) persist(result.layout)
  return result.changed
}

/** 重置为默认布局（写入完整默认值，同时清除所有隐藏）。 */
export function resetChromeLayout(): void {
  persist(defaultLayout(getChromeCatalog()))
}

// 其他标签页改了布局：重新读取并通知（仅在内容真的变化时替换快照，避免无谓重渲染）。
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key !== CHROME_LAYOUT_KEY && event.key !== null) return
    const current = getChromeLayout()
    storedRaw = undefined
    const next = normalizeLayout(readStoredRaw(), getChromeCatalog())
    if (JSON.stringify(serializeLayout(next)) === JSON.stringify(serializeLayout(current))) return
    layout = next
    layoutCatalog = getChromeCatalog()
    for (const listener of layoutListeners) listener()
  })
}

// 插件条目变化：布局快照按新目录重新归一化，通知订阅者。
subscribeChromeCatalog(() => {
  for (const listener of layoutListeners) listener()
})

function subscribeLayout(listener: Listener) {
  layoutListeners.add(listener)
  return () => {
    layoutListeners.delete(listener)
  }
}

export function useChromeLayout(): ChromeLayout {
  return useSyncExternalStore(subscribeLayout, getChromeLayout, getChromeLayout)
}

// ── 编辑态 ──

export interface ChromeEditState {
  editing: boolean
  /** 正在拖动的条目 id（dragover 读不到 dataTransfer 内容，只能存这里）。 */
  dragId: string | null
  /** 当前悬停的合法落点：`target` 为 null 表示区域空白处。 */
  hover: { zone: ChromeZone; target: string | null } | null
}

const IDLE: ChromeEditState = { editing: false, dragId: null, hover: null }
const editListeners = new Set<Listener>()
let edit: ChromeEditState = IDLE

function setEdit(next: ChromeEditState): void {
  edit = next
  for (const listener of editListeners) listener()
}

function subscribeEdit(listener: Listener) {
  editListeners.add(listener)
  return () => {
    editListeners.delete(listener)
  }
}

const getEdit = () => edit

export function useChromeEdit(): ChromeEditState {
  return useSyncExternalStore(subscribeEdit, getEdit, getEdit)
}

export function setChromeEditing(editing: boolean): void {
  if (edit.editing === editing) return
  setEdit(editing ? { editing, dragId: null, hover: null } : IDLE)
}

export function beginChromeDrag(id: string): void {
  setEdit({ ...edit, dragId: id, hover: null })
}

export function endChromeDrag(): void {
  if (edit.dragId === null && edit.hover === null) return
  setEdit({ ...edit, dragId: null, hover: null })
}

export function hoverChromeDrop(zone: ChromeZone, target: string | null): void {
  const hover = edit.hover
  if (hover?.zone === zone && hover.target === target) return
  setEdit({ ...edit, hover: { zone, target } })
}

/** 悬停离开某区域（仅当当前悬停仍属于该区域时才清除，避免覆盖已移到别处的悬停）。 */
export function leaveChromeDrop(zone: ChromeZone): void {
  if (edit.hover?.zone !== zone) return
  setEdit({ ...edit, hover: null })
}
