// 协议 v17 分页载入：`system.snapshot` 只带头，任务表 / 队列位置表经 `system.taskPage` 翻页，
// 载入期间到达的帧缓冲、装入后回放。模拟 transport 的写法同 client.test.ts。

import { afterEach, beforeEach, describe, expect, jest, spyOn, test } from 'bun:test'
import { startConnection, stopConnection, subscribeServiceEvents } from './client'
import { rpc } from './methods'
import { rpcStore } from './store'

type Request = { id: number; method: string; params?: Record<string, unknown> }

class MockWebSocket {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSING = 2
  static readonly CLOSED = 3
  static instances: MockWebSocket[] = []
  readyState = MockWebSocket.CONNECTING
  protocol = 'fluxdown.rpc.v1'
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: ((event: { code: number; reason: string }) => void) | null = null
  onerror: (() => void) | null = null
  sent: Request[] = []

  constructor(readonly url: string, readonly protocols: string[]) {
    MockWebSocket.instances.push(this)
  }

  open(): void {
    this.readyState = MockWebSocket.OPEN
    this.onopen?.()
  }

  send(text: string): void {
    this.sent.push(JSON.parse(text) as Request)
  }

  close(): void {
    this.readyState = MockWebSocket.CLOSING
  }

  closed(code = 1006, reason = ''): void {
    this.readyState = MockWebSocket.CLOSED
    this.onclose?.({ code, reason })
  }

  reply(method: string, result: unknown): void {
    const request = this.sent.findLast((item) => item.method === method)
    if (!request) throw new Error(`missing request: ${method}`)
    this.onmessage?.({ data: JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) })
  }

  fail(method: string, code: string): void {
    const request = this.sent.findLast((item) => item.method === method)
    if (!request) throw new Error(`missing request: ${method}`)
    this.onmessage?.({ data: JSON.stringify({ id: request.id, error: { code: -32000, message: code, data: { code } } }) })
  }

  push(text: string): void {
    this.onmessage?.({ data: text })
  }

  count(method: string): number {
    return this.sent.filter((item) => item.method === method).length
  }

  pages(): Array<Record<string, unknown> | undefined> {
    return this.sent.filter((item) => item.method === 'system.taskPage').map((item) => item.params)
  }
}

const globals = ['WebSocket', 'window', 'document', 'location'] as const
const descriptors = new Map(globals.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]))
const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve()
}

beforeEach(() => {
  jest.useFakeTimers()
  MockWebSocket.instances = []
  Object.defineProperties(globalThis, {
    WebSocket: { configurable: true, writable: true, value: MockWebSocket },
    window: { configurable: true, value: new EventTarget() },
    document: { configurable: true, value: new EventTarget() },
    location: { configurable: true, value: { protocol: 'http:', host: 'localhost:17800' } },
  })
  spyOn(console, 'warn').mockImplementation(() => undefined)
  spyOn(console, 'error').mockImplementation(() => undefined)
})

afterEach(async () => {
  stopConnection()
  await flush()
  jest.restoreAllMocks()
  jest.useRealTimers()
  for (const key of globals) {
    const descriptor = descriptors.get(key)
    if (descriptor) Object.defineProperty(globalThis, key, descriptor)
    else Reflect.deleteProperty(globalThis, key)
  }
})

// ---- 夹具 -----------------------------------------------------------------------------------

/** 线上任务：只带必带字段（默认值字段一律省略）。 */
function wireTask(taskId: string, patch: Record<string, unknown> = {}): Record<string, unknown> {
  return { taskId, url: `http://x/${taskId}`, fileName: `${taskId}.bin`, saveDir: '/d', status: 1, downloadedBytes: 0, totalBytes: 100, createdAt: '1', ...patch }
}

function snapshotBody(daemon: Record<string, unknown>): unknown {
  return {
    epoch: 'e',
    sequence: 5,
    body: {
      role: 'agent',
      snapshot: { daemonConnected: true, daemon: { tasks: [], taskRuntime: {}, queuePositions: [], ...daemon } },
    },
  }
}

/** 协议 v17 的快照头：不含任务表，带待载入标记。 */
const header = (daemon: Record<string, unknown> = {}) => snapshotBody({ taskTotal: 4, ...daemon })

const daemonFrame = (sequence: number, event: unknown, previous?: number) =>
  JSON.stringify({
    method: 'service.event',
    params: { epoch: 'e', sequence, ...(previous === undefined ? {} : { previous }), event: { service: 'agent', event: { type: 'daemon', data: event } } },
  })

const agentFrame = (sequence: number, event: unknown) =>
  JSON.stringify({ method: 'service.event', params: { epoch: 'e', sequence, event: { service: 'agent', event } } })

const taskPage = (tasks: unknown[], nextCursor?: string, epoch = 'e') => ({
  epoch,
  sequence: 5,
  section: 'tasks',
  total: 4,
  tasks,
  ...(nextCursor === undefined ? {} : { nextCursor }),
})

const queuePage = (queue: unknown[], nextCursor?: string) => ({
  epoch: 'e',
  sequence: 5,
  section: 'queue',
  total: queue.length,
  queue,
  ...(nextCursor === undefined ? {} : { nextCursor }),
})

const progress = (taskId: string, downloadedBytes: number) => ({
  type: 'engine',
  data: {
    type: 'taskProgress',
    taskId,
    status: 1,
    downloadedBytes,
    totalBytes: 100,
    speed: 0,
    fileName: '',
    saveDir: '',
    url: '',
    errorMessage: '',
    uploadedBytes: 0,
    seedingStatus: 0,
    seedingMessage: '',
    seedingTimeSecs: 0,
  },
})

function start(): MockWebSocket {
  startConnection('test-token')
  return MockWebSocket.instances[MockWebSocket.instances.length - 1]!
}

async function hello(ws: MockWebSocket): Promise<void> {
  ws.open()
  ws.reply('system.hello', { protocolVersion: 17 })
  await flush()
}

const daemonOf = () => rpcStore.peek().snapshot!.daemon as unknown as Record<string, unknown> & {
  tasks: Array<{
    taskId: string
    status: number
    downloadedBytes: number
    seedRatioLimitMilli: number
    seedTimeLimitMinutes: number
    sourceBytes: unknown
  }>
  queuePositions: Array<{ taskId: string; position: number; order: number }>
}
const phase = () => rpcStore.peek().connection.phase
const ids = () => daemonOf().tasks.map((task) => task.taskId)

// ---- 测试 -----------------------------------------------------------------------------------

describe('分页载入', () => {
  test('载入完成前为加载态且不宣布就绪；逐页翻到末页后任务按默认值补齐、队列位置名次 = 下标 + 1', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header({ queueTotal: 2 }))
    await flush()

    // 头已落地：加载态，半份（空）列表不是完整列表，连接还没就绪。
    expect(phase()).toBe('syncing')
    expect(daemonOf().taskTotal).toBe(4)
    expect(daemonOf().queueTotal).toBe(2)
    expect(daemonOf().tasks).toEqual([])
    expect(ws.pages()).toEqual([{ section: 'tasks' }])

    ws.reply('system.taskPage', taskPage([wireTask('a'), wireTask('b')], 'b'))
    await flush()
    expect(rpcStore.peek().tableLoad).toEqual({ loadedTasks: 2 })
    expect(daemonOf().tasks).toEqual([]) // 页只在全部到齐后一次装入
    expect(phase()).toBe('syncing')
    expect(ws.pages()[1]).toEqual({ section: 'tasks', cursor: 'b' })

    ws.reply('system.taskPage', taskPage([wireTask('c'), wireTask('d')]))
    await flush()
    expect(ws.pages()[2]).toEqual({ section: 'queue' })
    expect(phase()).toBe('syncing')
    expect(daemonOf().taskTotal).toBe(4)

    ws.reply('system.taskPage', queuePage([{ taskId: 'c', order: 20 }, { taskId: 'a', order: 10 }], '20'))
    await flush()
    expect(ws.pages()[3]).toEqual({ section: 'queue', cursor: '20' })
    ws.reply('system.taskPage', queuePage([{ taskId: 'b', order: 20 }]))
    await flush()

    expect(phase()).toBe('ready')
    const daemon = daemonOf()
    expect('taskTotal' in daemon).toBe(false)
    expect('queueTotal' in daemon).toBe(false)
    expect(rpcStore.peek().tableLoad).toBeNull()
    expect(ids()).toEqual(['a', 'b', 'c', 'd'])
    // 线上省略的默认值字段在入口补齐。
    expect(daemon.tasks[0]?.seedRatioLimitMilli).toBe(-2)
    expect(daemon.tasks[0]?.sourceBytes).toEqual({ cdnBytes: 0, proxyBytes: 0, nicBytes: 0 })
    expect(daemon.queuePositions).toEqual([
      { taskId: 'a', order: 10, position: 1 },
      { taskId: 'b', order: 20, position: 2 },
      { taskId: 'c', order: 20, position: 3 },
    ])
  })

  test('无待载入标记（空表 / 旧内联形态）：不请求页，直接就绪', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', snapshotBody({ tasks: [wireTask('a')] }))
    await flush()
    expect(phase()).toBe('ready')
    expect(ws.pages()).toEqual([])
    expect(ids()).toEqual(['a'])
    expect(daemonOf().tasks[0]?.seedRatioLimitMilli).toBe(-2)
  })

  test('分页拉取期间到达的 TaskChanged / TaskDeleted / Progress 不丢不重；载入期间帧不直接应用', async () => {
    const ws = start()
    await hello(ws)
    const seen: number[] = []
    const off = subscribeServiceEvents((frame) => seen.push(frame.sequence))
    // 快照应答之前就到达的帧也要缓冲，不得在基准之前应用。
    ws.push(daemonFrame(6, { type: 'taskChanged', data: wireTask('b', { status: 2 }) }))
    ws.reply('system.snapshot', header())
    await flush()

    ws.reply('system.taskPage', taskPage([wireTask('a'), wireTask('b')], 'b'))
    await flush()
    // 基准之前已覆盖的旧帧、翻页期间的新帧。
    ws.push(daemonFrame(4, { type: 'taskChanged', data: wireTask('a', { status: 3 }) }))
    ws.push(daemonFrame(7, { type: 'taskDeleted', data: { taskId: 'a' } }))
    ws.push(daemonFrame(8, progress('c', 77)))
    ws.push(daemonFrame(9, { type: 'taskChanged', data: wireTask('d') }))
    ws.push(daemonFrame(10, { type: 'taskChanged', data: wireTask('b', { status: 2, downloadedBytes: 40 }) }))
    jest.advanceTimersByTime(200)
    // 缓冲不应用、不通知监听器：载入期间任务表保持空（加载态）。
    expect(daemonOf().tasks).toEqual([])
    expect(seen).toEqual([])

    ws.reply('system.taskPage', taskPage([wireTask('c', { downloadedBytes: 10 }), wireTask('e')]))
    await flush()

    expect(phase()).toBe('ready')
    const tasks = daemonOf().tasks
    // 每个任务恰好一次（不重复）；被删的 a 不在；翻页后新增的 d 靠事件补齐。
    expect(ids()).toEqual(['b', 'c', 'e', 'd'])
    expect(new Set(ids()).size).toBe(ids().length)
    expect(tasks.find((task) => task.taskId === 'b')).toMatchObject({ status: 2, downloadedBytes: 40 }) // 取最后一次触及它的帧
    expect(tasks.find((task) => task.taskId === 'c')?.downloadedBytes).toBe(77) // 页读到的旧值被进度帧覆盖
    expect(tasks.find((task) => task.taskId === 'e')?.downloadedBytes).toBe(0) // 没被触及：页内容
    // 监听器按序收到回放的帧；基准之前的旧帧被 judgeFrame 丢弃。
    expect(seen).toEqual([6, 7, 8, 9, 10])
    off()
  })

  test('载入期间帧只回放一次：回放后的新帧照常按游标应用，不重复', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    ws.push(daemonFrame(6, { type: 'taskChanged', data: wireTask('x') }))
    ws.reply('system.taskPage', taskPage([wireTask('a')]))
    await flush()
    expect(ids()).toEqual(['a', 'x'])
    // 重复投递的旧帧被游标丢弃；后续帧正常应用。
    ws.push(daemonFrame(6, { type: 'taskChanged', data: wireTask('x') }))
    ws.push(daemonFrame(7, { type: 'taskChanged', data: wireTask('y') }))
    jest.advanceTimersByTime(16)
    expect(ids()).toEqual(['a', 'x', 'y'])
  })

  test('epoch 变化：放弃本轮，重新拉取快照', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    ws.reply('system.taskPage', taskPage([wireTask('a')], 'a', 'restarted'))
    await flush()
    expect(ws.count('system.snapshot')).toBe(2)
    expect(phase()).toBe('syncing')
    expect(daemonOf().tasks).toEqual([])

    // 第二轮：完整快照，就绪。
    ws.reply('system.snapshot', snapshotBody({ tasks: [wireTask('z')] }))
    await flush()
    expect(phase()).toBe('ready')
    expect(ids()).toEqual(['z'])
  })

  test('游标不前进：按失败处理并重新拉取快照', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    ws.reply('system.taskPage', taskPage([wireTask('a')], 'a'))
    await flush()
    expect(ws.pages()[1]).toEqual({ section: 'tasks', cursor: 'a' })
    ws.reply('system.taskPage', taskPage([wireTask('a')], 'a'))
    await flush()
    expect(ws.count('system.snapshot')).toBe(2)
    expect(phase()).toBe('syncing')
  })

  test('页应答错误：走重连路径，不宣布就绪', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    ws.fail('system.taskPage', 'unavailable')
    await flush()
    expect(phase()).toBe('reconnecting')
    expect(ws.readyState).toBe(MockWebSocket.CLOSING)
  })

  test('缓冲帧超过 65536：放弃本轮，页到达后整体重新同步', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    for (let sequence = 6; sequence <= 6 + 65_536; sequence += 1) {
      ws.push(daemonFrame(sequence, { type: 'taskDeleted', data: { taskId: 'nobody' } }))
    }
    ws.reply('system.taskPage', taskPage([wireTask('a')]))
    await flush()
    expect(ws.count('system.snapshot')).toBe(2)
    expect(phase()).toBe('syncing')
    expect(daemonOf().tasks).toEqual([])
  })

  test('回放中遇到序号断档：整体重新同步', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    ws.push(daemonFrame(9, { type: 'taskChanged', data: wireTask('x') }, 8))
    ws.reply('system.taskPage', taskPage([wireTask('a')]))
    await flush()
    expect(ws.count('system.snapshot')).toBe(2)
    expect(phase()).toBe('syncing')
  })

  test('回放中遇到 tablesReloading：应用它并就地重新载入，其后的帧在新一轮装入后回放', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    ws.push(daemonFrame(6, { type: 'taskChanged', data: wireTask('x') }))
    ws.push(daemonFrame(7, { type: 'tablesReloading', data: { tasks: 3 } }))
    ws.push(daemonFrame(8, { type: 'taskChanged', data: wireTask('y', { status: 3 }) }))
    ws.reply('system.taskPage', taskPage([wireTask('a'), wireTask('b')]))
    await flush()

    // 第一轮装入并回放到重载帧：投影回到加载态，重新从头翻页；仍未就绪。
    expect(phase()).toBe('syncing')
    expect(daemonOf().taskTotal).toBe(3)
    expect(daemonOf().tasks).toEqual([])
    expect(ws.pages()).toEqual([{ section: 'tasks' }, { section: 'tasks' }])
    expect(ws.count('system.snapshot')).toBe(1)

    ws.reply('system.taskPage', taskPage([wireTask('a'), wireTask('b'), wireTask('x')]))
    await flush()
    expect(phase()).toBe('ready')
    expect(ids()).toEqual(['a', 'b', 'x', 'y'])
    expect(daemonOf().tasks[3]?.status).toBe(3)
  })

  test('4009 / 断线时丢弃进行中的载入：晚到的页不生效，重连后重新握手拿快照', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', header())
    await flush()
    ws.closed(4009, 'event-gap')
    await flush()
    expect(phase()).toBe('reconnecting')
    // 保留的快照仍是加载态（不是「完整的空列表」）。
    expect(daemonOf().taskTotal).toBe(4)
    expect(daemonOf().tasks).toEqual([])

    ws.reply('system.taskPage', taskPage([wireTask('a')]))
    await flush()
    expect(daemonOf().tasks).toEqual([])
    expect(phase()).toBe('reconnecting')

    jest.advanceTimersByTime(3_001)
    const next = MockWebSocket.instances[1]!
    await hello(next)
    next.reply('system.snapshot', header())
    await flush()
    expect(next.pages()).toEqual([{ section: 'tasks' }])
    next.reply('system.taskPage', taskPage([wireTask('q')]))
    await flush()
    expect(phase()).toBe('ready')
    expect(ids()).toEqual(['q'])
  })
})

describe('运行中重载', () => {
  async function readyWithTasks(): Promise<MockWebSocket> {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', snapshotBody({ tasks: [wireTask('old1'), wireTask('old2')], queuePositions: [{ taskId: 'old1', order: 1, position: 1 }] }))
    await flush()
    expect(phase()).toBe('ready')
    return ws
  }

  test('tablesReloading：应用它进入加载态，缓冲其后的帧，分页装入后回放；连接阶段保持 ready', async () => {
    const ws = await readyWithTasks()
    ws.push(daemonFrame(6, { type: 'tablesReloading', data: { tasks: 3 } }))
    // 重载帧同步应用：任务表清空、加载态可见。
    expect(daemonOf().taskTotal).toBe(3)
    expect(daemonOf().tasks).toEqual([])
    expect(phase()).toBe('ready')
    expect(ws.pages()).toEqual([{ section: 'tasks' }])

    // 载入期间的帧只缓冲。
    ws.push(daemonFrame(7, { type: 'taskChanged', data: wireTask('n1') }))
    ws.push(daemonFrame(8, { type: 'taskDeleted', data: { taskId: 'c2' } }))
    jest.advanceTimersByTime(100)
    expect(daemonOf().tasks).toEqual([])

    ws.reply('system.taskPage', taskPage([wireTask('c1'), wireTask('c2'), wireTask('c3')]))
    await flush()
    expect('taskTotal' in daemonOf()).toBe(false)
    expect(ids()).toEqual(['c1', 'c3', 'n1'])
    // 队列位置没被标记：不动。
    expect(daemonOf().queuePositions).toEqual([{ taskId: 'old1', order: 1, position: 1 }])

    // 回到常规增量路径。
    ws.push(daemonFrame(9, { type: 'taskChanged', data: wireTask('after') }))
    jest.advanceTimersByTime(16)
    expect(ids()).toEqual(['c1', 'c3', 'n1', 'after'])
  })

  test('daemonSnapshotReplaced 头：整体替换投影并分页重载任务与队列位置', async () => {
    const ws = await readyWithTasks()
    ws.push(
      agentFrame(6, {
        type: 'daemonSnapshotReplaced',
        data: { tasks: [], taskRuntime: {}, queuePositions: [], taskTotal: 2, queueTotal: 2 },
      }),
    )
    expect(daemonOf().tasks).toEqual([])
    expect(daemonOf().queuePositions).toEqual([])
    expect(daemonOf().taskTotal).toBe(2)
    expect(daemonOf().queueTotal).toBe(2)
    ws.reply('system.taskPage', taskPage([wireTask('n1'), wireTask('n2')]))
    await flush()
    expect(ws.pages()[1]).toEqual({ section: 'queue' })
    expect(daemonOf().queueTotal).toBe(2) // 队列还没到齐：仍是加载态
    ws.reply('system.taskPage', queuePage([{ taskId: 'n2', order: 7 }, { taskId: 'n1', order: 3 }]))
    await flush()
    expect(ids()).toEqual(['n1', 'n2'])
    expect(daemonOf().queuePositions).toEqual([
      { taskId: 'n1', order: 3, position: 1 },
      { taskId: 'n2', order: 7, position: 2 },
    ])
    expect('queueTotal' in daemonOf()).toBe(false)
    expect(phase()).toBe('ready')
  })

  test('重载期间 epoch 变化：重新拉取快照', async () => {
    const ws = await readyWithTasks()
    ws.push(daemonFrame(6, { type: 'tablesReloading', data: { tasks: 1 } }))
    ws.reply('system.taskPage', taskPage([wireTask('a')], undefined, 'other'))
    await flush()
    expect(ws.count('system.snapshot')).toBe(2)
    expect(phase()).toBe('syncing')
    ws.reply('system.snapshot', snapshotBody({ tasks: [wireTask('fresh')] }))
    await flush()
    expect(phase()).toBe('ready')
    expect(ids()).toEqual(['fresh'])
  })

  test('无待载入标记的整表替换（≤2000 行内联）照常应用，不进入加载态', async () => {
    const ws = await readyWithTasks()
    ws.push(daemonFrame(6, { type: 'engine', data: { type: 'tasksSnapshot', tasks: [wireTask('only')] } }))
    jest.advanceTimersByTime(16)
    expect(ws.pages()).toEqual([])
    expect(ids()).toEqual(['only'])
    expect(daemonOf().tasks[0]?.seedRatioLimitMilli).toBe(-2)
  })
})

describe('入口归一化', () => {
  test('daemon.task.get 补齐默认值', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', snapshotBody({}))
    await flush()
    const pending = rpc.daemon.task.get({ taskId: 'a' })
    ws.reply('daemon.task.get', wireTask('a'))
    const task = await pending
    expect(task.taskId).toBe('a')
    expect(task.seedRatioLimitMilli).toBe(-2)
    expect(task.sourceBytes).toEqual({ cdnBytes: 0, proxyBytes: 0, nicBytes: 0 })
    expect(task.errorMessage).toBe('')
  })

  test('TaskChanged / TasksChanged 帧里的任务补齐默认值', async () => {
    const ws = start()
    await hello(ws)
    ws.reply('system.snapshot', snapshotBody({}))
    await flush()
    ws.push(daemonFrame(6, { type: 'taskChanged', data: wireTask('a') }))
    ws.push(daemonFrame(7, { type: 'tasksChanged', data: [wireTask('b'), wireTask('c', { seedTimeLimitMinutes: 30 })] }))
    jest.advanceTimersByTime(16)
    expect(daemonOf().tasks.map((task) => task.seedRatioLimitMilli)).toEqual([-2, -2, -2])
    expect(daemonOf().tasks[2]?.seedTimeLimitMinutes).toBe(30)
  })
})
