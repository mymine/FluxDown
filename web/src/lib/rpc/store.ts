// 快照存储：连接状态 + AgentSnapshot。React 侧经 useSyncExternalStore 订阅（见 hooks.ts）。
//
// 事件高频（任务进度）：应用到「工作态」是同步的，对订阅者的发布合并到每帧一次，
// 连接阶段变化与整份快照替换则立即发布。

import type { AgentSnapshot, ServiceHello } from './protocol'

export type ConnectionPhase =
  /** 尚未启动（未登录）。 */
  | 'idle'
  /** 首次建立连接 / 握手中。 */
  | 'connecting'
  /** 已握手，正在拉取快照。 */
  | 'syncing'
  /** 快照就绪，事件增量应用中。 */
  | 'ready'
  /** 断线后退避重连（`snapshot` 保留上一次内容，只读）。 */
  | 'reconnecting'
  /** 访问密钥无效 / 被服务端更换：需回登录页。 */
  | 'unauthorized'
  /** 服务端尚未设置访问密钥：需走首次运行向导。 */
  | 'setupRequired'
  /** 服务已退出（关闭原因 service-quit），不再重连。 */
  | 'stopped'
  /** 协议版本不兼容，不再重连。 */
  | 'incompatible'

export interface ConnectionState {
  phase: ConnectionPhase
  /** 连续失败次数（成功同步后清零）。 */
  attempt: number
  /** 下次重连的时间戳（ms，仅 reconnecting）。 */
  nextRetryAt: number | null
  lastError: string | null
  /** 服务因自更新重启（关闭原因 service-restart）：保持重连，新版本就绪后刷新页面。 */
  restarting: boolean
}

export interface RpcState {
  connection: ConnectionState
  snapshot: AgentSnapshot | null
  hello: ServiceHello | null
  /** 分页载入进度（仅载入期间非 null）：已载入的任务行数；总数见 `snapshot.daemon.taskTotal`。 */
  tableLoad: { loadedTasks: number } | null
}

const INITIAL: RpcState = {
  connection: { phase: 'idle', attempt: 0, nextRetryAt: null, lastError: null, restarting: false },
  snapshot: null,
  hello: null,
  tableLoad: null,
}

type Listener = () => void

class RpcStore {
  private working: RpcState = INITIAL
  private published: RpcState = INITIAL
  private readonly listeners = new Set<Listener>()
  private scheduled = false

  /** React 快照（合并发布后的稳定引用）。 */
  readonly getPublished = (): RpcState => this.published

  /** 最新工作态（非 React 代码、事件应用使用）。 */
  peek(): RpcState {
    return this.working
  }

  readonly subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  /** 应用变更；`immediate` 立即发布，否则合并到下一帧。 */
  update(change: (state: RpcState) => RpcState, immediate = false): void {
    const next = change(this.working)
    if (next === this.working) return
    this.working = next
    if (immediate) {
      this.flush()
    } else if (!this.scheduled) {
      this.scheduled = true
      const run = () => {
        this.scheduled = false
        this.flush()
      }
      if (typeof requestAnimationFrame === 'function') requestAnimationFrame(run)
      else setTimeout(run, 16)
    }
  }

  private flush(): void {
    if (this.published === this.working) return
    this.published = this.working
    for (const listener of this.listeners) listener()
  }

  /** 回到未启动状态（登出）。 */
  reset(): void {
    this.working = INITIAL
    this.flush()
  }
}

export const rpcStore = new RpcStore()

export function setConnection(patch: Partial<ConnectionState>): void {
  rpcStore.update(
    (state) => ({ ...state, connection: { ...state.connection, ...patch } }),
    true,
  )
}
