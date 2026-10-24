// agent `/rpc` WebSocket JSON-RPC 客户端（单例连接）。
//
// 流程：WebSocket 子协议鉴权 → system.hello → system.snapshot（头，不含任务表）→ 分页 system.taskPage
// 载入任务表 / 队列位置表 → 回放载入期间缓冲的帧 → 按 service.event 增量应用。
// - 载入期间（快照 / 页在途）到达的帧只缓冲不应用；载入完成前不宣布 ready，快照里的 taskTotal /
//   queueTotal 即「加载态」标志；
// - 运行中收到 snapshotReplaced 头 / tablesReloading：应用它进入加载态，就地重新载入；
// - 事件序号断档 / epoch 变化 / 页不一致：重新 snapshot（同一连接内先缓冲后回放）；
// - 关闭码 4009（订阅者落后）：带抖动的短退避后重连并重新 snapshot；
// - 事件帧按时间片成批应用（见 `FRAME_BATCH_MS`）；
// - 关闭原因 `service-quit`：不再重连；
// - 其他断线：指数退避重连，网络恢复/页面回到前台立即重试；
// - 升级前失败（浏览器拿不到 HTTP 状态）：探测 `/api/v1/info` 区分密钥无效 / 需初始化 / 网络问题。

import { applyAgentEvents, installQueue, installTasks, serviceEventReloadsTables, tablesLoading } from './apply'
import { judgeFrame, type EventCursor } from './cursor'
import { RpcError, TRANSPORT_ERROR_CODE, errorMessage, transportError } from './error'
import {
  CAPABILITY_CLIENT_SELECTIONS,
  CLOSE_REASON_SERVICE_QUIT,
  CLOSE_REASON_SERVICE_RESTART,
  EVENT_GAP_CLOSE_CODE,
  METHOD,
  MIN_PROTOCOL_VERSION,
  PROTOCOL_VERSION,
  TABLE_SYNC_BACKLOG_LIMIT,
} from './protocol'
import { normalizeAgentSnapshot, normalizeFrame, normalizeTablePage } from './normalize'
import { claimIncompatibleReload, shouldReloadForVersionChange } from './reload'
import { TableLoad, TableSyncError } from './tableSync'
import type { LoadedTables } from './tableSync'
import type { AgentEvent, ClientHello, DaemonSnapshot, EventFrame, RpcErrorData, ServiceHello, Snapshot, TablePageWire } from './protocol'
import { probeToken } from '../access/setup'
import { rpcStore, setConnection } from './store'

const SUBPROTOCOL = 'fluxdown.rpc.v1'
const TOKEN_PREFIX = 'fluxdown.token.'
const CLIENT_VERSION: string = import.meta.env.VITE_APP_VERSION ?? 'dev'

const DEFAULT_TIMEOUT_MS = 30_000
const PING_INTERVAL_MS = 25_000
const PING_TIMEOUT_MS = 10_000
const BACKOFF_BASE_MS = 500
const BACKOFF_MAX_MS = 15_000
/** 同一次连接内连续 resync 的上限，超过则断开重连。 */
const MAX_RESYNC = 5
/** 快照 / 载入期间事件缓冲上限（协议常量）；溢出后快照 / 页到达即重新同步。 */
const MAX_BUFFERED = TABLE_SYNC_BACKLOG_LIMIT
/** 事件帧成批应用的时间片：同一时间片内到达的帧一次性应用（大集合整批只复制一次）。 */
const FRAME_BATCH_MS = 16
/** 4009（事件落后）后重连退避：起点 / 封顶；`GAP_STREAK_WINDOW_MS` 内再次落后视为连续。 */
const GAP_BACKOFF_BASE_MS = 250
const GAP_BACKOFF_MAX_MS = 3_000
const GAP_STREAK_WINDOW_MS = 30_000

export interface CallOptions {
  /** 请求超时（ms），默认 30s。安装类长任务请调大。 */
  timeoutMs?: number
}

interface Pending {
  method: string
  resolve: (value: unknown) => void
  reject: (error: RpcError) => void
  timer: ReturnType<typeof setTimeout>
}

type EventListener = (frame: EventFrame) => void

function base64Url(text: string): string {
  const bytes = new TextEncoder().encode(text)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '')
}

function wsUrl(): string {
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${scheme}//${location.host}/rpc`
}

class RpcConnection {
  private ws: WebSocket | null = null
  private generation = 0
  private token = ''
  private wanted = false
  private helloDone = false
  private everReady = false
  /** 最近一次成功握手的 serviceVersion；重启后版本变化即刷新页面。 */
  private lastServiceVersion: string | null = null
  private attempt = 0
  private nextId = 1
  private readonly pending = new Map<number, Pending>()
  private cursor: EventCursor | null = null
  private syncing = false
  private buffered: EventFrame[] = []
  /** 快照在途期间缓冲溢出：溢出的帧已丢，快照到达后必须重新拉取。 */
  private bufferOverflowed = false
  /** 已通过游标判定、待成批应用的帧。 */
  private queued: EventFrame[] = []
  private batchTimer: ReturnType<typeof setTimeout> | null = null
  private gapStreak = 0
  private lastGapAt = 0
  private resyncs = 0
  private retryTimer: ReturnType<typeof setTimeout> | null = null
  private pingTimer: ReturnType<typeof setInterval> | null = null
  private readonly listeners = new Set<EventListener>()
  private envHooked = false

  /** 以访问密钥启动（幂等：相同密钥且已在运行则忽略）。 */
  start(token: string): void {
    if (this.wanted && this.token === token) return
    this.teardown()
    this.token = token
    this.wanted = true
    this.everReady = false
    this.attempt = 0
    this.hookEnvironment()
    this.open()
  }

  /** 就地替换访问密钥（服务端轮换后）：不断开、不重拉快照，下次重连才用新值。 */
  setToken(token: string): void {
    if (this.token !== '' && token !== '') this.token = token
  }

  /** 当前连接使用的密钥（unauthorized 时用来与存储值比较）。 */
  currentToken(): string {
    return this.token
  }

  /** 停止并清空状态（登出）。 */
  stop(): void {
    this.wanted = false
    this.token = ''
    this.lastServiceVersion = null
    this.teardown()
    rpcStore.reset()
  }

  /** 手动立即重试（用户点「重试」）；服务已退出（stopped）时也允许重新拉起。 */
  retryNow(): void {
    if (this.token === '') return
    if (this.ws?.readyState === WebSocket.OPEN && this.helloDone && !this.syncing && this.cursor !== null) return
    this.teardown()
    this.wanted = true
    this.attempt = 0
    this.open()
  }

  /** 环境事件（网络恢复/回到前台）触发的重试：只在仍处于自动重连态时生效。 */
  private poke(): void {
    if (!this.wanted || this.retryTimer === null) return
    this.clearRetry()
    this.open()
  }

  subscribeEvents(listener: EventListener): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  call<R>(method: string, params?: object, options?: CallOptions): Promise<R> {
    return this.request<R>(method, params, options?.timeoutMs ?? DEFAULT_TIMEOUT_MS, false)
  }

  // ---- 连接生命周期 -------------------------------------------------------

  private hookEnvironment(): void {
    if (this.envHooked) return
    this.envHooked = true
    window.addEventListener('online', () => this.poke())
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible') this.poke()
    })
  }

  private open(): void {
    const generation = ++this.generation
    setConnection({
      phase: this.everReady || this.attempt > 0 ? 'reconnecting' : 'connecting',
      nextRetryAt: null,
    })
    let ws: WebSocket
    try {
      ws = new WebSocket(wsUrl(), [SUBPROTOCOL, TOKEN_PREFIX + base64Url(this.token)])
    } catch (error) {
      this.scheduleRetry(errorMessage(error))
      return
    }
    this.ws = ws
    let opened = false
    ws.onopen = () => {
      if (generation !== this.generation) return
      if (ws.protocol !== SUBPROTOCOL) {
        ws.close(4000, 'subprotocol-mismatch')
        return
      }
      opened = true
      void this.handshake(generation).catch((error: unknown) => this.onHandshakeFailure(generation, error))
    }
    ws.onmessage = (event) => {
      if (generation !== this.generation) return
      if (typeof event.data === 'string') this.onMessage(event.data)
    }
    ws.onclose = (event) => {
      if (generation !== this.generation) return
      void this.onClose(generation, event, opened)
    }
    ws.onerror = () => {
      // 错误后必有 close，统一在 onClose 处理。
    }
  }

  private async handshake(generation: number): Promise<void> {
    const hello: ClientHello = {
      clientName: 'fluxdown-web',
      clientVersion: CLIENT_VERSION,
      minProtocolVersion: MIN_PROTOCOL_VERSION,
      maxProtocolVersion: PROTOCOL_VERSION,
      requestedRole: 'agent',
      capabilities: [CAPABILITY_CLIENT_SELECTIONS],
    }
    // 握手前的 call 必须绕过 helloDone 检查。
    const service = await this.request<ServiceHello>('system.hello', hello, DEFAULT_TIMEOUT_MS, true)
    if (generation !== this.generation) return
    this.helloDone = true
    const serviceVersion = service.serviceVersion ?? ''
    if (shouldReloadForVersionChange(this.lastServiceVersion, serviceVersion)) {
      location.reload()
      return
    }
    this.lastServiceVersion = serviceVersion
    rpcStore.update((state) => ({ ...state, hello: service }), true)
    this.resyncs = 0
    await this.sync(generation)
  }

  private onHandshakeFailure(generation: number, error: unknown): void {
    if (generation !== this.generation) return
    if (error instanceof RpcError) {
      if (error.is('protocolIncompatible')) {
        // Docker / NAS 升级跨协议版本：新 SPA 在新服务里，刷新一次取新资源（限频，避免死循环）。
        if (claimIncompatibleReload()) {
          location.reload()
          return
        }
        this.fatal('incompatible', error.message)
        return
      }
      if (error.is('unauthorized')) {
        this.fatal('unauthorized', error.message)
        return
      }
    }
    // 请求已失败，不等待 WebSocket 关闭握手才退出当前连接。
    this.teardown()
    this.scheduleRetry(errorMessage(error))
  }

  /**
   * 拉取快照、分页载入任务表并回放缓冲事件。快照 / 载入在途期间到达的帧只缓冲、不应用（`syncing`）；
   * 缓冲超过上限时溢出部分已被丢弃，这份快照之后的状态缺口无法补齐：丢掉它重新拉取
   * （重试次数受 `MAX_RESYNC` 限制）。载入完成前不宣布 `ready`。
   */
  private async sync(generation: number): Promise<void> {
    for (;;) {
      this.syncing = true
      this.buffered = []
      this.bufferOverflowed = false
      this.discardPending()
      this.cursor = null
      rpcStore.update((state) => (state.tableLoad === null ? state : { ...state, tableLoad: null }))
      setConnection({ phase: 'syncing' })
      const snapshot = await this.request<Snapshot>('system.snapshot', undefined, DEFAULT_TIMEOUT_MS, true)
      if (generation !== this.generation) return
      if (snapshot.body.role !== 'agent') throw new Error('unexpected snapshot role')
      if (this.bufferOverflowed) {
        if (this.exhaustedResyncs()) return
        continue
      }
      const agent = normalizeAgentSnapshot(snapshot.body.snapshot)
      this.cursor = { epoch: snapshot.epoch, sequence: snapshot.sequence }
      rpcStore.update((state) => ({ ...state, snapshot: agent }), true)

      const outcome = await this.settle(generation)
      if (outcome === 'stale') return
      if (outcome === 'resync') {
        if (this.exhaustedResyncs()) return
        continue
      }
      this.everReady = true
      this.attempt = 0
      setConnection({ phase: 'ready', attempt: 0, nextRetryAt: null, lastError: null, restarting: false })
      this.startPing()
      return
    }
  }

  /** 连续重新同步次数超限则断开重连（返回 true）。 */
  private exhaustedResyncs(): boolean {
    this.resyncs += 1
    if (this.resyncs <= MAX_RESYNC) return false
    this.ws?.close(4000, 'resync-loop')
    return true
  }

  /**
   * 基准帧（快照或宣告重载的事件）已应用到投影之后的收尾：按投影里的待载入标记分页载入表并装入，
   * 再以基准序号为游标回放缓冲帧；回放中遇到又一个 `reloadsTables` 帧就应用它并重新载入
   * （契约 §2）。整个过程 `syncing` 保持为真，帧只缓冲；返回 `done` 时同步退出缓冲态。
   *
   * - `done`：投影完整、缓冲已清空；
   * - `resync`：缓冲溢出 / 页不一致 / 帧断档，调用方应整体重新同步；
   * - `stale`：连接已被替换或关闭，什么都不要再做。
   */
  private async settle(generation: number): Promise<'done' | 'resync' | 'stale'> {
    for (;;) {
      const cursor = this.cursor
      const daemon = rpcStore.peek().snapshot?.daemon
      if (cursor === null || daemon === undefined) return 'resync'
      if (tablesLoading(daemon)) {
        const loaded = await this.loadTables(generation, cursor.epoch, daemon)
        if (loaded === 'stale' || loaded === 'resync') return loaded
        this.installTables(loaded)
      }

      // 回放整个缓冲：逐帧判定游标，应用只做一次批量。
      const backlog = this.buffered
      this.buffered = []
      let reloadAt = -1
      for (let at = 0; at < backlog.length; at += 1) {
        const frame = backlog[at] as EventFrame
        const verdict = judgeFrame(cursor, frame)
        if (verdict === 'skip') continue
        if (verdict === 'resync') return 'resync'
        cursor.sequence = frame.sequence
        this.queued.push(frame)
        if (serviceEventReloadsTables(frame.event)) {
          reloadAt = at
          break
        }
      }
      if (!this.flushPending()) return 'resync'
      if (reloadAt >= 0) {
        // 重载帧已应用（投影回到加载态）；其后的帧留作下一轮的缓冲。
        this.buffered = backlog.slice(reloadAt + 1)
        continue
      }
      this.syncing = false
      return 'done'
    }
  }

  /** 逐页拉取待载入的表（先任务、后队列位置）；载入期间到达的帧由 `onMessage` 缓冲。 */
  private async loadTables(
    generation: number,
    epoch: string,
    daemon: DaemonSnapshot,
  ): Promise<LoadedTables | 'resync' | 'stale'> {
    const load = TableLoad.forSnapshot(epoch, daemon)
    if (load === null) return { tasks: null, queue: null }
    rpcStore.update((state) => ({ ...state, tableLoad: { loadedTasks: 0 } }))
    for (;;) {
      const params = load.nextRequest()
      if (params === null) return load.parts()
      const wire = await this.request<TablePageWire>(METHOD.SYSTEM_TASK_PAGE, params, DEFAULT_TIMEOUT_MS, false)
      if (generation !== this.generation) return 'stale'
      if (this.bufferOverflowed) return 'resync'
      try {
        load.accept(normalizeTablePage(wire))
      } catch (error) {
        if (error instanceof TableSyncError) {
          console.warn('table paging failed; resyncing', error.failure)
          return 'resync'
        }
        throw error
      }
      rpcStore.update((state) => ({ ...state, tableLoad: { loadedTasks: load.loadedTasks } }))
    }
  }

  /**
   * 把载入的表装入投影并退出加载态。不立即发布：紧随其后回放缓冲帧，订阅者只应看到回放后的完整状态
   * （初次同步由随后的 `ready`、重载由下一帧的合并发布一并带出）。
   */
  private installTables(tables: LoadedTables): void {
    rpcStore.update((state) => {
      if (state.snapshot === null) return state
      let daemon = state.snapshot.daemon
      if (tables.tasks !== null) daemon = installTasks(daemon, tables.tasks)
      if (tables.queue !== null) daemon = installQueue(daemon, tables.queue)
      return { ...state, snapshot: { ...state.snapshot, daemon }, tableLoad: null }
    })
  }

  private onMessage(text: string): void {
    let message: unknown
    try {
      message = JSON.parse(text)
    } catch {
      return
    }
    if (typeof message !== 'object' || message === null) return
    const record = message as Record<string, unknown>
    if ('id' in record && ('result' in record || 'error' in record)) {
      // 应答之前到达的事件必须先生效：调用方拿到结果后立即读快照，不能比应答旧。
      this.flushPending()
      this.onResponse(record)
      return
    }
    if (record.method === 'service.event' && typeof record.params === 'object' && record.params !== null) {
      // 入口归一化：帧里的 `TaskDto` 补齐线上省略的默认值字段，之后 reducer / UI 只见完整任务。
      // 解不出来的帧不能丢（游标会与投影脱节）：按「应用失败」处理，整体重新同步。
      let frame: EventFrame
      try {
        frame = normalizeFrame(record.params as EventFrame)
      } catch (error) {
        console.error('failed to decode service event; resyncing', error)
        if (this.syncing) {
          this.buffered = []
          this.bufferOverflowed = true
        } else {
          void this.resync(this.generation)
        }
        return
      }
      if (this.syncing) {
        if (this.bufferOverflowed) return
        if (this.buffered.length >= MAX_BUFFERED) {
          // 丢弃缓冲并记下溢出：快照 / 页到达后（`sync` / `loadTables`）据此重新同步。这里不能调用
          // `resync`——它在同步期间会直接返回，缺口就被静默吞掉了。
          this.buffered = []
          this.bufferOverflowed = true
          return
        }
        this.buffered.push(frame)
        return
      }
      if (this.admit(frame, this.generation)) this.scheduleFlush()
    }
  }

  /**
   * 按游标判定一帧并推进游标；接受的帧进入待应用队列（由 `flushPending` 成批应用）。
   * 返回 false 表示已触发重新同步 / 重新载入，调用方不必再安排应用。
   */
  private admit(frame: EventFrame, generation: number): boolean {
    const cursor = this.cursor
    if (!cursor) return false
    const verdict = judgeFrame(cursor, frame)
    if (verdict === 'skip') return true
    if (verdict === 'resync') {
      void this.resync(generation)
      return false
    }
    cursor.sequence = frame.sequence
    this.queued.push(frame)
    if (serviceEventReloadsTables(frame.event)) {
      this.beginReload(generation)
      return false
    }
    return true
  }

  /**
   * 运行中收到要求重载表的帧（`snapshotReplaced` 头 / `tablesReloading`）：先把它（及此前已接受的帧）
   * 应用掉，投影才会进入加载态；之后到达的帧一律缓冲，直到页载完并回放。
   */
  private beginReload(generation: number): void {
    if (!this.flushPending()) return
    this.syncing = true
    this.buffered = []
    this.bufferOverflowed = false
    void this.reloadTables(generation)
  }

  private async reloadTables(generation: number): Promise<void> {
    try {
      const outcome = await this.settle(generation)
      if (outcome === 'stale' || outcome === 'done') return
      this.syncing = false
      await this.resync(generation)
    } catch (error) {
      this.onHandshakeFailure(generation, error)
    }
  }

  private scheduleFlush(): void {
    if (this.batchTimer !== null) return
    this.batchTimer = setTimeout(() => {
      this.batchTimer = null
      this.flushPending()
    }, FRAME_BATCH_MS)
  }

  /** 丢弃尚未应用的帧（重新同步 / 断线：快照会整体取代它们）。 */
  private discardPending(): void {
    if (this.batchTimer !== null) clearTimeout(this.batchTimer)
    this.batchTimer = null
    this.queued = []
  }

  /**
   * 把已接受的帧一次性应用到快照并通知监听器。游标在入队时已前移，应用抛异常意味着快照与游标
   * 脱节，只能重新同步；返回 false 表示已触发重新同步。监听器各自隔离：一个监听器出错不影响
   * 其他监听器，也不影响快照。
   */
  private flushPending(): boolean {
    if (this.batchTimer !== null) clearTimeout(this.batchTimer)
    this.batchTimer = null
    const frames = this.queued
    if (frames.length === 0) return true
    this.queued = []
    const events: AgentEvent[] = []
    for (const frame of frames) {
      if (frame.event.service === 'agent') events.push(frame.event.event)
    }
    if (events.length > 0) {
      try {
        rpcStore.update((state) => (state.snapshot ? { ...state, snapshot: applyAgentEvents(state.snapshot, events) } : state))
      } catch (error) {
        console.error('failed to apply service events; resyncing', error)
        void this.resync(this.generation)
        return false
      }
    }
    for (const frame of frames) {
      for (const listener of this.listeners) {
        try {
          listener(frame)
        } catch (error) {
          console.error('service event listener failed', error)
        }
      }
    }
    return true
  }

  /** 4009 后的重连延迟：30s 内连续落后按 250ms 起翻倍到 3s 封顶，再取 50%~100% 抖动。 */
  private gapRetryDelay(): number {
    const now = Date.now()
    this.gapStreak = now - this.lastGapAt < GAP_STREAK_WINDOW_MS ? this.gapStreak + 1 : 0
    this.lastGapAt = now
    const ceiling = Math.min(GAP_BACKOFF_MAX_MS, GAP_BACKOFF_BASE_MS * 2 ** this.gapStreak)
    return Math.round(ceiling * (0.5 + Math.random() * 0.5))
  }

  private async resync(generation: number): Promise<void> {
    if (generation !== this.generation || this.syncing) return
    this.resyncs += 1
    if (this.resyncs > MAX_RESYNC) {
      this.ws?.close(4000, 'resync-loop')
      return
    }
    try {
      await this.sync(generation)
    } catch (error) {
      this.onHandshakeFailure(generation, error)
    }
  }

  private async onClose(generation: number, event: CloseEvent, opened: boolean): Promise<void> {
    this.ws = null
    // 先隔离该连接的异步握手/快照 continuation，再拒绝在途请求。
    generation = ++this.generation
    this.stopPing()
    this.helloDone = false
    this.syncing = false
    this.cursor = null
    this.buffered = []
    this.discardPending()
    this.rejectAll(transportError('connection closed', 'unavailable'))
    if (!this.wanted) return

    if (event.reason === CLOSE_REASON_SERVICE_RESTART) {
      // server 模式自更新重启：保持重连（含退避），握手发现 serviceVersion 变化后刷新页面。
      setConnection({ restarting: true })
      this.scheduleRetry(event.reason)
      return
    }
    if (event.reason === CLOSE_REASON_SERVICE_QUIT) {
      this.fatal('stopped', event.reason)
      return
    }
    if (event.code === EVENT_GAP_CLOSE_CODE) {
      // 事件落后（4009）：服务端重连后仍要重拉整份快照，立即重连会让落后的订阅者反复重拉；
      // 带抖动的短退避，连续落后时逐步拉长。
      this.scheduleRetry(event.reason || 'event-gap', this.gapRetryDelay())
      return
    }
    if (!opened) {
      const probe = await probeToken(this.token)
      if (generation !== this.generation || !this.wanted) return
      if (probe === 'unauthorized') return this.fatal('unauthorized', 'unauthorized')
      if (probe === 'setupRequired') return this.fatal('setupRequired', 'setup required')
      if (probe === 'unknown' && !this.everReady) return this.fatal('unauthorized', 'rejected')
    }
    this.scheduleRetry(event.reason || `closed (${event.code})`)
  }

  private fatal(phase: 'unauthorized' | 'setupRequired' | 'stopped' | 'incompatible', message: string): void {
    this.teardown()
    this.wanted = false
    setConnection({ phase, nextRetryAt: null, lastError: message, restarting: false })
  }

  private scheduleRetry(reason: string, delayMs?: number): void {
    this.clearRetry()
    const exponent = Math.min(this.attempt, 10)
    const base = Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** exponent)
    const delay = delayMs ?? Math.round(base * (0.8 + Math.random() * 0.4))
    this.attempt += 1
    setConnection({
      phase: 'reconnecting',
      attempt: this.attempt,
      nextRetryAt: Date.now() + delay,
      lastError: reason,
    })
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null
      if (this.wanted) this.open()
    }, delay)
  }

  private clearRetry(): void {
    if (this.retryTimer !== null) clearTimeout(this.retryTimer)
    this.retryTimer = null
  }

  private teardown(): void {
    this.clearRetry()
    this.stopPing()
    this.generation += 1
    const ws = this.ws
    this.ws = null
    if (ws) {
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null
      ws.close(1000, 'client-stop')
    }
    this.helloDone = false
    this.syncing = false
    this.cursor = null
    this.buffered = []
    this.discardPending()
    this.rejectAll(transportError('connection closed', 'unavailable'))
  }

  // ---- keepalive ---------------------------------------------------------

  private startPing(): void {
    this.stopPing()
    this.pingTimer = setInterval(() => {
      const generation = this.generation
      this.request('system.ping', undefined, PING_TIMEOUT_MS, false).catch(() => {
        if (generation !== this.generation) return
        // 服务端同通道串行处理：有长耗时请求在途时 ping 排队属正常，不据此断线。
        for (const pending of this.pending.values()) {
          if (pending.method !== 'system.ping') return
        }
        this.ws?.close(4000, 'ping-timeout')
      })
    }, PING_INTERVAL_MS)
  }

  private stopPing(): void {
    if (this.pingTimer !== null) clearInterval(this.pingTimer)
    this.pingTimer = null
  }

  // ---- 请求/响应 ---------------------------------------------------------

  private request<R>(method: string, params: object | undefined, timeoutMs: number, handshake: boolean): Promise<R> {
    const ws = this.ws
    if (!ws || ws.readyState !== WebSocket.OPEN || (!handshake && !this.helloDone)) {
      return Promise.reject(transportError('not connected', 'unavailable', method))
    }
    const id = this.nextId++
    const frame = { jsonrpc: '2.0', id, method, ...(params === undefined ? {} : { params }) }
    return new Promise<R>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(transportError(`request timed out: ${method}`, 'timeout', method))
      }, timeoutMs)
      this.pending.set(id, { method, resolve: resolve as (value: unknown) => void, reject, timer })
      try {
        ws.send(JSON.stringify(frame))
      } catch (error) {
        clearTimeout(timer)
        this.pending.delete(id)
        reject(transportError(errorMessage(error), 'unavailable', method))
      }
    })
  }

  private onResponse(record: Record<string, unknown>): void {
    const id = record.id
    if (typeof id !== 'number') return
    const pending = this.pending.get(id)
    if (!pending) return
    this.pending.delete(id)
    clearTimeout(pending.timer)
    if ('error' in record && typeof record.error === 'object' && record.error !== null) {
      const error = record.error as { code?: number; message?: string; data?: RpcErrorData }
      pending.reject(
        new RpcError({
          code: error.code ?? TRANSPORT_ERROR_CODE,
          message: error.message ?? 'rpc error',
          data: error.data,
          method: pending.method,
        }),
      )
      return
    }
    pending.resolve(record.result)
  }

  private rejectAll(error: RpcError): void {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer)
      pending.reject(error)
      this.pending.delete(id)
    }
  }
}

const connection = new RpcConnection()

export const startConnection = (token: string): void => connection.start(token)
export const setConnectionToken = (token: string): void => connection.setToken(token)
export const connectionToken = (): string => connection.currentToken()
export const stopConnection = (): void => connection.stop()
export const retryConnection = (): void => connection.retryNow()
export const subscribeServiceEvents = (listener: EventListener): (() => void) => connection.subscribeEvents(listener)

/** 通用 RPC 调用（方法包装器 `rpc.*` 的底层入口）。 */
export function call<R = unknown>(method: string, params?: object, options?: CallOptions): Promise<R> {
  return connection.call<R>(method, params, options)
}
