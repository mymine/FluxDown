// FluxMesh对话框的模块级 store：任何位置（下载页菜单、详情面板、新建下载框、FluxMesh页）都可 `openXxx()`，
// `ShareHost` 订阅并渲染分享面板 / 接收确认框。

import { useSyncExternalStore } from 'react'

/** 分享面板请求：`shareId` 打开既有分享；否则按 `taskIds` 复用或创建分享。 */
export interface SharePanelRequest {
  nonce: number
  taskIds: readonly string[]
  shareId: string | null
}

/** 接收确认请求：取件码或分享链接，由确认框 resolve。 */
export interface ShareReceiveRequest {
  nonce: number
  codeOrLink: string
}

interface ShareDialogsState {
  panel: SharePanelRequest | null
  receive: ShareReceiveRequest | null
}

let state: ShareDialogsState = { panel: null, receive: null }
let nonce = 0
const listeners = new Set<() => void>()

function publish(patch: Partial<ShareDialogsState>): void {
  state = { ...state, ...patch }
  for (const listener of listeners) listener()
}

const subscribe = (listener: () => void): (() => void) => {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function useShareDialogs(): ShareDialogsState {
  return useSyncExternalStore(subscribe, () => state)
}

/** 为这批已完成任务打开分享面板（同批任务已有有效分享则复用）。 */
export function openSharePanel(taskIds: readonly string[]): void {
  publish({ panel: { nonce: ++nonce, taskIds: [...taskIds], shareId: null } })
}

/** 打开既有分享的面板（我的分享列表）。 */
export function openShareDetail(shareId: string): void {
  publish({ panel: { nonce: ++nonce, taskIds: [], shareId } })
}

export function closeSharePanel(): void {
  publish({ panel: null })
}

/** 解析取件码 / 分享链接并弹出接收确认框。 */
export function openShareReceive(codeOrLink: string): void {
  publish({ receive: { nonce: ++nonce, codeOrLink } })
}

export function closeShareReceive(): void {
  publish({ receive: null })
}
