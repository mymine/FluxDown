// 可移植视图会话 store（模块级，React 经 useSyncExternalStore 读取）。
// - 每个 `PluginViewHost` 持有一个 ViewSession：mount → `view.open`，unmount → `view.close`；
// - `pluginViewFrame` 按 seq 应用，断档（seq != last+1）或应用失败改取 `view.snapshot`；
// - `pluginViewClosed` 标记会话结束（显示「已结束」+ 重新打开）；
// - 连接重新就绪（重连、快照整体替换）后对全部会话重取 snapshot，会话已不存在则重新 open；
// - 每 30 s 对所有存活会话发 keepalive（daemon 90 s 无续期即关闭）。

import { useSyncExternalStore } from 'react'
import { RpcError, errorMessage, rpc, rpcStore, subscribeServiceEvents } from '../../../lib/rpc'
import type { DaemonEvent, EventFrame, ViewEventDto, ViewNode } from '../../../lib/rpc'
import { VIEW_ERROR_PROP, applyViewOps } from '../../../lib/rpc/protocol/pluginView'
import { toast } from '../../../ui'
import type { ToastKind } from '../../../ui'

export const KEEPALIVE_INTERVAL_MS = 30_000

export type SessionPhase = 'loading' | 'ready' | 'closed' | 'failed'

export interface ViewSessionState {
  phase: SessionPhase
  tree: ViewNode | null
  /** `failed` 时的错误文案。 */
  error: string | null
  /** 会话结束原因（`closed` / `expired` / `pluginUnloaded`）。 */
  reason: string | null
}

export interface ViewTarget {
  identity: string
  view: string
  taskId?: string
  locale: string
}

const INITIAL: ViewSessionState = { phase: 'loading', tree: null, error: null, reason: null }

/** 从事件帧取出 daemon 事件（agent 转发的与 daemon 直出的两种形状）。 */
export function daemonEventOf(frame: EventFrame): DaemonEvent | null {
  const body = frame.event
  if (body.service === 'daemon') return body.event
  return body.event.type === 'daemon' ? body.event.data : null
}

const live = new Map<string, ViewSession>()

export class ViewSession {
  private state: ViewSessionState = INITIAL
  private id: string | null = null
  private seq = 0
  private disposed = false
  /** 每次 open / reopen 递增，用来丢弃过期的异步结果。 */
  private epoch = 0
  private resyncing = false
  private readonly listeners = new Set<() => void>()

  constructor(readonly target: ViewTarget) {}

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  readonly getState = (): ViewSessionState => this.state

  get sessionId(): string | null {
    return this.id
  }

  private set(next: Partial<ViewSessionState>): void {
    this.state = { ...this.state, ...next }
    for (const listener of this.listeners) listener()
  }

  private register(id: string): void {
    this.id = id
    live.set(id, this)
    ensureRunning()
  }

  private unregister(): void {
    if (this.id !== null && live.get(this.id) === this) live.delete(this.id)
    this.id = null
    stopIfIdle()
  }

  /** 打开（或重新打开）会话。 */
  async open(): Promise<void> {
    const epoch = ++this.epoch
    this.unregister()
    this.set({ phase: 'loading', error: null, reason: null })
    try {
      const opened = await rpc.daemon.plugin.viewOpen({
        identity: this.target.identity,
        view: this.target.view,
        taskId: this.target.taskId,
        locale: this.target.locale,
      })
      if (this.disposed || epoch !== this.epoch) {
        void rpc.daemon.plugin.viewClose({ session: opened.session }).catch(() => undefined)
        return
      }
      this.seq = opened.seq
      this.register(opened.session)
      this.set({ phase: 'ready', tree: opened.tree })
    } catch (error) {
      if (this.disposed || epoch !== this.epoch) return
      this.set({ phase: 'failed', error: errorMessage(error) })
    }
  }

  /** 卸载：关闭 daemon 会话。 */
  dispose(): void {
    this.disposed = true
    this.epoch += 1
    const id = this.id
    this.unregister()
    this.listeners.clear()
    if (id !== null) void rpc.daemon.plugin.viewClose({ session: id }).catch(() => undefined)
  }

  /** 应用一帧；seq 非 last+1 或应用失败时改取 snapshot。 */
  applyFrame(seq: number, ops: Parameters<typeof applyViewOps>[1]): void {
    const tree = this.state.tree
    if (this.state.phase !== 'ready' || tree === null) return
    if (seq <= this.seq) return
    if (seq !== this.seq + 1) {
      void this.resync()
      return
    }
    try {
      const next = applyViewOps(tree, ops)
      this.seq = seq
      this.set({ tree: next })
    } catch {
      void this.resync()
    }
  }

  markClosed(reason: string): void {
    this.unregister()
    this.set({ phase: 'closed', reason })
  }

  /** 重取 snapshot；会话已不存在则重新 open。 */
  async resync(): Promise<void> {
    const id = this.id
    if (id === null || this.resyncing || this.disposed) return
    this.resyncing = true
    const epoch = this.epoch
    try {
      const snapshot = await rpc.daemon.plugin.viewSnapshot({ session: id })
      if (this.disposed || epoch !== this.epoch || this.id !== id) return
      this.seq = snapshot.seq
      this.set({ phase: 'ready', tree: snapshot.tree })
    } catch (error) {
      if (this.disposed || epoch !== this.epoch || this.id !== id) return
      // 传输层失败（断线 / 超时）等连接就绪后再取；会话确实不存在才重新 open。
      if (error instanceof RpcError && (error.appCode === 'unavailable' || error.appCode === 'timeout')) return
      this.resyncing = false
      await this.open()
    } finally {
      this.resyncing = false
    }
  }

  /** 投递视图事件（入队即返回；结果经帧送回）。 */
  send(node: string, event: ViewEventDto): void {
    const id = this.id
    if (id === null || this.state.phase !== 'ready') return
    void rpc.daemon.plugin.viewEvent({ session: id, node, event }).catch((error: unknown) => {
      toast.error(error)
    })
  }
}

// ── 全局：事件订阅 / 重连重取 / keepalive ──

let unsubscribeEvents: (() => void) | null = null
let unsubscribeStore: (() => void) | null = null
let keepaliveTimer: ReturnType<typeof setInterval> | null = null
let wasReady = rpcStore.peek().connection.phase === 'ready'

function onFrame(frame: EventFrame): void {
  // agent 重连 daemon 后整体替换快照：断线期间可能漏帧。
  if (frame.event.service === 'agent' && frame.event.event.type === 'daemonSnapshotReplaced') {
    resyncAll()
    return
  }
  const event = daemonEventOf(frame)
  if (event === null) return
  if (event.type === 'pluginViewFrame') {
    live.get(event.data.session)?.applyFrame(event.data.seq, event.data.ops)
  } else if (event.type === 'pluginViewClosed') {
    live.get(event.data.session)?.markClosed(event.data.reason)
  }
}

function resyncAll(): void {
  for (const session of [...live.values()]) void session.resync()
}

function onStore(): void {
  const ready = rpcStore.peek().connection.phase === 'ready'
  // 重新进入就绪（重连 / 重新同步）：断线期间可能漏帧，全部重取。
  if (ready && !wasReady) resyncAll()
  wasReady = ready
}

function keepalive(): void {
  const sessions = [...live.keys()]
  if (sessions.length === 0) return
  void rpc.daemon.plugin.viewKeepalive({ sessions }).catch(() => undefined)
}

function ensureRunning(): void {
  unsubscribeEvents ??= subscribeServiceEvents(onFrame)
  unsubscribeStore ??= rpcStore.subscribe(onStore)
  keepaliveTimer ??= setInterval(keepalive, KEEPALIVE_INTERVAL_MS)
}

function stopIfIdle(): void {
  if (live.size > 0) return
  unsubscribeEvents?.()
  unsubscribeEvents = null
  unsubscribeStore?.()
  unsubscribeStore = null
  if (keepaliveTimer !== null) clearInterval(keepaliveTimer)
  keepaliveTimer = null
}

export function useViewSession(session: ViewSession): ViewSessionState {
  return useSyncExternalStore(session.subscribe, session.getState, session.getState)
}

/** 根节点上的渲染错误（`p.__error`）。 */
export function viewError(tree: ViewNode | null): string | null {
  const value = tree?.p[VIEW_ERROR_PROP]
  return typeof value === 'string' && value !== '' ? value : null
}

// ── toast ──

const TOAST_KINDS: readonly ToastKind[] = ['info', 'success', 'warning', 'error']

export function toastKind(level: string): ToastKind {
  return (TOAST_KINDS as readonly string[]).includes(level) ? (level as ToastKind) : 'info'
}

/** 订阅 `pluginToast` 事件并转为全局 toast（挂在外壳上，返回取消函数）。 */
export function startPluginToasts(): () => void {
  return subscribeServiceEvents((frame) => {
    const event = daemonEventOf(frame)
    if (event?.type !== 'pluginToast') return
    const { level, text } = event.data
    const kind = toastKind(level)
    if (kind === 'error') toast.error(text)
    else toast[kind](text)
  })
}
