// 协议 v17 的分页载入 reducer 规则（对应 native/protocol/src/event.rs 的
// `TablesReloading` / `SnapshotReplaced` 头 / `install_tasks` / `install_queue` / 清除 total 标志）。

import { describe, expect, test } from 'bun:test'
import {
  agentEventReloadsTables,
  applyAgentEvent,
  applyDaemonEvent,
  applyDaemonEvents,
  daemonEventReloadsTables,
  installQueue,
  installTasks,
  serviceEventReloadsTables,
  tablesLoading,
} from './apply'
import type { AgentSnapshot, DaemonSnapshot, QueuePositionDto, TaskDto, TaskRuntimeDto } from './protocol'

function task(taskId: string, status = 1): TaskDto {
  return { taskId, status, fileName: `${taskId}.bin`, saveDir: '/d', url: 'http://x/a', downloadedBytes: 0, totalBytes: 10 } as unknown as TaskDto
}

function runtime(taskId: string, active = 2): TaskRuntimeDto {
  return {
    taskId,
    sampleSequence: 1,
    totalBytes: 10,
    activeTransfers: active,
    connectedPeers: 1,
    segments: [{ index: 0, startByte: 0, endByte: 9, downloadedBytes: 1, active: true }],
  } as unknown as TaskRuntimeDto
}

function position(taskId: string, order: number, rank: number): QueuePositionDto {
  return { taskId, order, position: rank }
}

function daemon(patch: Partial<DaemonSnapshot> = {}): DaemonSnapshot {
  return {
    tasks: [],
    queuePositions: [],
    taskRuntime: {},
    pendingSelections: [],
    rssItemRevisions: {},
    priority: [],
    plugins: [],
    ...patch,
  } as unknown as DaemonSnapshot
}

describe('tablesReloading', () => {
  test('tasks：清空任务表并置 taskTotal；队列位置与运行时不动', () => {
    const base = daemon({
      tasks: [task('a'), task('b')],
      queuePositions: [position('a', 10, 1)],
      taskRuntime: { a: runtime('a') },
    })
    const next = applyDaemonEvent(base, { type: 'tablesReloading', data: { tasks: 7 } })
    expect(next.tasks).toEqual([])
    expect(next.taskTotal).toBe(7)
    expect(next.queueTotal).toBeUndefined()
    expect(next.queuePositions).toBe(base.queuePositions)
    expect(next.taskRuntime).toBe(base.taskRuntime)
    expect(tablesLoading(next)).toBe(true)
    // 原快照不被就地修改。
    expect(base.tasks.length).toBe(2)
    expect(base.taskTotal).toBeUndefined()
  })

  test('queue：清空队列位置并置 queueTotal；任务表不动', () => {
    const base = daemon({ tasks: [task('a')], queuePositions: [position('a', 10, 1)] })
    const next = applyDaemonEvent(base, { type: 'tablesReloading', data: { queue: 3 } })
    expect(next.queuePositions).toEqual([])
    expect(next.queueTotal).toBe(3)
    expect(next.tasks).toBe(base.tasks)
    expect(next.taskTotal).toBeUndefined()
  })

  test('两者同时携带；0 行也算进入加载态', () => {
    const next = applyDaemonEvent(daemon({ tasks: [task('a')] }), { type: 'tablesReloading', data: { tasks: 0, queue: 0 } })
    expect(next.taskTotal).toBe(0)
    expect(next.queueTotal).toBe(0)
    expect(tablesLoading(next)).toBe(true)
  })

  test('批内先 queueOrderChanged 再 tablesReloading(queue)：累计增量作废，不写回空表', () => {
    const base = daemon({ queuePositions: [position('a', 10, 1)] })
    const next = applyDaemonEvents(base, [
      { type: 'engine', data: { type: 'queueOrderChanged', removed: [], added: [{ taskId: 'z', order: 5 }] } },
      { type: 'tablesReloading', data: { queue: 1 } },
    ])
    expect(next.queuePositions).toEqual([])
    expect(next.queueTotal).toBe(1)
  })

  test('agent 转发同样生效（嵌套在 daemon 事件里）', () => {
    const agent = { daemon: daemon({ tasks: [task('a')] }) } as unknown as AgentSnapshot
    const next = applyAgentEvent(agent, { type: 'daemon', data: { type: 'tablesReloading', data: { tasks: 9 } } })
    expect(next.daemon.tasks).toEqual([])
    expect(next.daemon.taskTotal).toBe(9)
  })
})

describe('替换头', () => {
  const header = daemon({ taskTotal: 5, queueTotal: 2, queuePositions: [] })

  test('snapshotReplaced：整体替换投影，含加载态标志', () => {
    const base = daemon({ tasks: [task('a')], queuePositions: [position('a', 1, 1)] })
    const next = applyDaemonEvent(base, { type: 'snapshotReplaced', data: header })
    expect(next.tasks).toEqual([])
    expect(next.taskTotal).toBe(5)
    expect(next.queueTotal).toBe(2)
    expect(tablesLoading(next)).toBe(true)
  })

  test('daemonSnapshotReplaced（agent）：整体替换 daemon 投影', () => {
    const agent = { daemon: daemon({ tasks: [task('a')] }), daemonConnected: true } as unknown as AgentSnapshot
    const next = applyAgentEvent(agent, { type: 'daemonSnapshotReplaced', data: header })
    expect(next.daemon.taskTotal).toBe(5)
    expect(next.daemon.tasks).toEqual([])
  })

  test('替换为无标记的完整快照：退出加载态', () => {
    const loading = applyDaemonEvent(daemon(), { type: 'snapshotReplaced', data: header })
    const done = applyDaemonEvent(loading, { type: 'snapshotReplaced', data: daemon({ tasks: [task('a')] }) })
    expect(tablesLoading(done)).toBe(false)
    expect(done.tasks.length).toBe(1)
  })
})

describe('整表替换清除加载标志', () => {
  test('engine tasksSnapshot（≤2000 行的内联形态）：清 taskTotal，runtime 只留存在的任务、非活跃清零', () => {
    const base = daemon({
      taskTotal: 3,
      taskRuntime: { a: runtime('a'), b: runtime('b'), gone: runtime('gone') },
    })
    const next = applyDaemonEvent(base, {
      type: 'engine',
      data: { type: 'tasksSnapshot', tasks: [task('a', 1), task('b', 2)] },
    })
    expect(next.taskTotal).toBeUndefined()
    expect('taskTotal' in next).toBe(false)
    expect(Object.keys(next.taskRuntime).sort()).toEqual(['a', 'b'])
    expect(next.taskRuntime.a).toBe(base.taskRuntime.a)
    expect(next.taskRuntime.b?.activeTransfers).toBe(0)
    expect(next.taskRuntime.b?.segments.every((segment) => segment.active === false)).toBe(true)
  })

  test('engine tasksSnapshot / installTasks：已完成任务的运行时被剪除，暂停与失败保留', () => {
    const base = daemon({
      tasks: [],
      taskTotal: 4,
      taskRuntime: { run: runtime('run'), done: runtime('done'), paused: runtime('paused'), failed: runtime('failed') },
    })
    const next = installTasks(base, [task('run', 1), task('done', 3), task('paused', 2), task('failed', 4)])
    expect(Object.keys(next.taskRuntime).sort()).toEqual(['failed', 'paused', 'run'])
    const replaced = applyDaemonEvent(next, {
      type: 'engine',
      data: { type: 'tasksSnapshot', tasks: [task('run', 1), task('paused', 3)] },
    } as never)
    expect(Object.keys(replaced.taskRuntime)).toEqual(['run'])
  })

  test('engine queuePositionsChanged（内联形态）：清 queueTotal', () => {
    const base = daemon({ queueTotal: 4 })
    const next = applyDaemonEvent(base, {
      type: 'engine',
      data: { type: 'queuePositionsChanged', positions: [position('a', 1, 1)] },
    })
    expect(next.queueTotal).toBeUndefined()
    expect(next.queuePositions.length).toBe(1)
    // 只清自己那一张表的标志。
    const both = daemon({ taskTotal: 1, queueTotal: 1 })
    const queueOnly = applyDaemonEvent(both, { type: 'engine', data: { type: 'queuePositionsChanged', positions: [] } })
    expect(queueOnly.taskTotal).toBe(1)
    expect(queueOnly.queueTotal).toBeUndefined()
  })
})

describe('install_tasks / install_queue', () => {
  test('installTasks：装入任务、清 taskTotal、runtime 只保留存在的任务、非活跃读数归零', () => {
    const base = daemon({
      taskTotal: 3,
      queueTotal: 1,
      taskRuntime: { run: runtime('run'), paused: runtime('paused'), gone: runtime('gone') },
    })
    const next = installTasks(base, [task('run', 1), task('paused', 2), task('prep', 5)])
    expect(next.tasks.map((item) => item.taskId)).toEqual(['run', 'paused', 'prep'])
    expect(next.taskTotal).toBeUndefined()
    expect(next.queueTotal).toBe(1) // 队列表还没载入：仍是加载态
    expect(tablesLoading(next)).toBe(true)
    expect(Object.keys(next.taskRuntime).sort()).toEqual(['paused', 'run'])
    expect(next.taskRuntime.run).toBe(base.taskRuntime.run)
    expect(next.taskRuntime.paused?.activeTransfers).toBe(0)
    expect(next.taskRuntime.paused?.connectedPeers).toBe(0)
    // 装入后按下标索引可被后续事件命中（索引已重建）。
    const changed = applyDaemonEvent(next, { type: 'taskChanged', data: { ...task('paused', 1), downloadedBytes: 4 } })
    expect(changed.tasks.length).toBe(3)
    expect(changed.tasks[1]?.downloadedBytes).toBe(4)
  })

  test('installQueue：按 (order, taskId) 排序，名次 = 下标 + 1，清 queueTotal', () => {
    const base = daemon({ queueTotal: 4, queuePositions: [] })
    const next = installQueue(base, [
      { taskId: 'c', order: 30 },
      { taskId: 'b', order: 10 },
      { taskId: 'a', order: 10 },
      { taskId: 'd', order: 20 },
    ])
    expect(next.queuePositions).toEqual([
      { taskId: 'a', order: 10, position: 1 },
      { taskId: 'b', order: 10, position: 2 },
      { taskId: 'd', order: 20, position: 3 },
      { taskId: 'c', order: 30, position: 4 },
    ])
    expect(next.queueTotal).toBeUndefined()
    expect(tablesLoading(next)).toBe(false)
  })

  test('两张表都装入后退出加载态；装入后 queueOrderChanged 增量正常叠加', () => {
    let current = daemon({ taskTotal: 1, queueTotal: 1 })
    current = installTasks(current, [task('a', 0)])
    current = installQueue(current, [{ taskId: 'a', order: 5 }])
    expect(tablesLoading(current)).toBe(false)
    const next = applyDaemonEvent(current, {
      type: 'engine',
      data: { type: 'queueOrderChanged', removed: [], added: [{ taskId: 'b', order: 1 }] },
    })
    expect(next.queuePositions.map((entry) => [entry.taskId, entry.position])).toEqual([
      ['b', 1],
      ['a', 2],
    ])
  })
})

describe('reloadsTables 判定', () => {
  const header = daemon({ taskTotal: 1 })
  const full = daemon({ tasks: [task('a')] })

  test('带标志的替换头 / tablesReloading 触发；无标志的替换与普通事件不触发', () => {
    expect(daemonEventReloadsTables({ type: 'snapshotReplaced', data: header })).toBe(true)
    expect(daemonEventReloadsTables({ type: 'snapshotReplaced', data: full })).toBe(false)
    expect(daemonEventReloadsTables({ type: 'tablesReloading', data: { tasks: 3 } })).toBe(true)
    expect(daemonEventReloadsTables({ type: 'tablesReloading', data: { queue: 0 } })).toBe(true)
    expect(daemonEventReloadsTables({ type: 'tablesReloading', data: {} })).toBe(false)
    expect(daemonEventReloadsTables({ type: 'taskChanged', data: task('a') })).toBe(false)
    expect(daemonEventReloadsTables({ type: 'engine', data: { type: 'tasksSnapshot', tasks: [] } })).toBe(false)
  })

  test('agent 事件：daemonSnapshotReplaced 头、转发的 daemon 事件', () => {
    expect(agentEventReloadsTables({ type: 'daemonSnapshotReplaced', data: header })).toBe(true)
    expect(agentEventReloadsTables({ type: 'daemonSnapshotReplaced', data: full })).toBe(false)
    expect(agentEventReloadsTables({ type: 'daemon', data: { type: 'tablesReloading', data: { tasks: 1 } } })).toBe(true)
    expect(agentEventReloadsTables({ type: 'daemonConnectionChanged', data: true })).toBe(false)
    expect(serviceEventReloadsTables({ service: 'daemon', event: { type: 'tablesReloading', data: { queue: 1 } } })).toBe(true)
    expect(
      serviceEventReloadsTables({ service: 'agent', event: { type: 'daemon', data: { type: 'taskChanged', data: task('a') } } }),
    ).toBe(false)
  })
})
