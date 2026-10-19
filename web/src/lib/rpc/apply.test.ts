import { describe, expect, test } from 'bun:test'
import { WEBHOOK_DELIVERY_LIMIT, applyAgentEvent, applyDaemonEvent } from './apply'
import type { AgentSnapshot, DaemonEvent, DaemonSnapshot, TaskDto, TaskRuntimeDto, WebhookDeliveryDto } from './protocol'

test('cloud connection is independent of sync, clears stale self presence and resets on logout', () => {
  const initial = {
    sync: { connected: true },
    cloudDevices: [{ deviceId: 'self', isCurrent: true, isOnline: true }],
    remoteTasks: [],
  } as unknown as AgentSnapshot
  const failed = applyAgentEvent(initial, {
    type: 'cloudConnectionChanged',
    data: { state: 'reconnecting', lastError: 'heartbeat rejected', lastErrorReason: 'cloudUnreachable' },
  })
  expect(failed.sync).toBe(initial.sync)
  expect(failed.cloudDevices[0]?.isOnline).toBe(false)
  expect(failed.cloudConnection?.lastErrorReason).toBe('cloudUnreachable')
  const restored = applyAgentEvent(failed, { type: 'cloudConnectionChanged', data: { state: 'connected' } })
  expect(restored.cloudConnection?.lastError).toBeUndefined()
  expect(restored.cloudDevices[0]?.isOnline).toBe(false)
  const loggedOut = applyAgentEvent(restored, { type: 'sessionChanged', data: null })
  expect(loggedOut.cloudConnection).toEqual({ state: 'disconnected' })
})

function task(taskId: string, status: number): TaskDto {
  return { taskId, status, fileName: 'a.bin', saveDir: '/d', url: 'http://x/a', downloadedBytes: 0, totalBytes: 10, errorMessage: '' } as unknown as TaskDto
}

function runtime(taskId: string, sampleSequence: number, active = 2): TaskRuntimeDto {
  return {
    taskId,
    sampleSequence,
    totalBytes: 10,
    activeTransfers: active,
    segments: [{ index: 0, startByte: 0, endByte: 9, downloadedBytes: 1, active: true }],
  } as unknown as TaskRuntimeDto
}

function daemon(tasks: TaskDto[], taskRuntime: Record<string, TaskRuntimeDto> = {}): DaemonSnapshot {
  return { tasks, taskRuntime, pendingSelections: [], rssItemRevisions: {}, priority: [], plugins: [] } as unknown as DaemonSnapshot
}

describe('apply daemon events', () => {
  test('过期采样被丢弃；分段沿用上次；非活跃任务清零活跃读数', () => {
    let snap = daemon([task('t', 1)], { t: runtime('t', 5) })
    const stale = applyDaemonEvent(snap, { type: 'taskRuntimeChanged', data: runtime('t', 5, 9) })
    expect(stale).toBe(snap)

    const empty = { ...runtime('t', 6, 3), segments: [] }
    snap = applyDaemonEvent(snap, { type: 'taskRuntimeChanged', data: empty })
    expect(snap.taskRuntime.t?.segments.length).toBe(1)
    expect(snap.taskRuntime.t?.activeTransfers).toBe(3)

    snap = applyDaemonEvent(snap, { type: 'taskChanged', data: task('t', 2) })
    expect(snap.taskRuntime.t?.activeTransfers).toBe(0)
    expect(snap.taskRuntime.t?.segments[0]?.active).toBe(false)
  })

  test('tasksChanged：同一事件内插入 + 更新，等价逐条 taskChanged；整批只复制一次数组并清零非活跃运行时', () => {
    const base = daemon([task('a', 1), task('b', 1)], { a: runtime('a', 5), b: runtime('b', 5) })
    const batch = [task('a', 2), task('c', 0), task('d', 1), task('c', 1)]
    const batched = applyDaemonEvent(base, { type: 'tasksChanged', data: batch })

    let sequential = base
    for (const item of batch) sequential = applyDaemonEvent(sequential, { type: 'taskChanged', data: item })
    expect(batched).toEqual(sequential)

    expect(batched.tasks.map((item) => [item.taskId, item.status])).toEqual([
      ['a', 2],
      ['b', 1],
      ['c', 1],
      ['d', 1],
    ])
    // a 变为暂停：活跃读数清零；b 不在批内：运行时保持原引用。
    expect(batched.taskRuntime.a?.activeTransfers).toBe(0)
    expect(batched.taskRuntime.b).toBe(base.taskRuntime.b)
    // 原快照不被就地修改。
    expect(base.tasks.length).toBe(2)
  })

  test('tasksChanged：空批保持引用；只含活跃任务时不复制 taskRuntime', () => {
    const base = daemon([task('a', 1)], { a: runtime('a', 5) })
    expect(applyDaemonEvent(base, { type: 'tasksChanged', data: [] })).toBe(base)
    const next = applyDaemonEvent(base, { type: 'tasksChanged', data: [task('n', 1)] })
    expect(next.tasks.length).toBe(2)
    expect(next.taskRuntime).toBe(base.taskRuntime)
  })

  test('tasksDeleted：整批等价于逐条 taskDeleted；空批或无命中保持引用', () => {
    const base = daemon([task('a', 1), task('b', 1), task('c', 3)], { a: runtime('a', 5), c: runtime('c', 5) })
    const ids = ['a', 'c', 'missing']
    const batched = applyDaemonEvent(base, { type: 'tasksDeleted', data: { taskIds: ids } })

    let sequential = base
    for (const taskId of ids) sequential = applyDaemonEvent(sequential, { type: 'taskDeleted', data: { taskId } })
    expect(batched.tasks).toEqual(sequential.tasks)
    expect(batched.taskRuntime).toEqual(sequential.taskRuntime)
    expect(batched.tasks.map((item) => item.taskId)).toEqual(['b'])
    expect(base.tasks.length).toBe(3)
    expect(base.taskRuntime.a).not.toBeUndefined()

    expect(applyDaemonEvent(base, { type: 'tasksDeleted', data: { taskIds: [] } })).toBe(base)
    expect(applyDaemonEvent(base, { type: 'tasksDeleted', data: { taskIds: ['zzz'] } })).toBe(base)
  })

  test('未知任务的采样被忽略', () => {
    const snap = daemon([])
    expect(applyDaemonEvent(snap, { type: 'taskRuntimeChanged', data: runtime('x', 1) })).toBe(snap)
  })

  test('engine taskProgress：status 4 + deleted 删除任务；否则只覆盖非空字符串字段', () => {
    const base = daemon([task('t', 1)], { t: runtime('t', 1) })
    const msg = {
      type: 'taskProgress', taskId: 't', status: 1, downloadedBytes: 5, totalBytes: 10, speed: 0, fileName: '',
      saveDir: '', uploadSpeed: 0, url: '', errorMessage: '', uploadedBytes: 0, seedingStatus: 0, seedingMessage: '', seedingTimeSecs: 0,
    } as const
    const progressed = applyDaemonEvent(base, { type: 'engine', data: msg })
    expect(progressed.tasks[0]?.fileName).toBe('a.bin')
    expect(progressed.tasks[0]?.downloadedBytes).toBe(5)

    const deleted = applyDaemonEvent(base, { type: 'engine', data: { ...msg, status: 4, errorMessage: 'deleted' } })
    expect(deleted.tasks.length).toBe(0)
    expect(deleted.taskRuntime.t).toBeUndefined()
  })

  test('engine tasksSnapshot：剪除已消失任务；非活跃清零，活跃与已清零条目保持引用', () => {
    const active = runtime('a', 1)
    const cleared = { ...runtime('c', 1, 0), connectedPeers: 0, segments: [] } as unknown as TaskRuntimeDto
    const base = daemon([], { a: active, b: runtime('b', 1), c: cleared, gone: runtime('gone', 1) })
    const next = applyDaemonEvent(base, {
      type: 'engine',
      data: { type: 'tasksSnapshot', tasks: [task('a', 1), task('b', 2), task('c', 2)] },
    } as never)
    expect(next.taskRuntime.a).toBe(active)
    expect(next.taskRuntime.c).toBe(cleared)
    expect(next.taskRuntime.gone).toBeUndefined()
    expect(next.taskRuntime.b?.activeTransfers).toBe(0)
    expect(next.taskRuntime.b?.segments[0]?.active).toBe(false)
  })

  test('selectionPending 按 requestId 去重，selectionResolved 移除', () => {
    const req = (requestId: string) => ({ requestId, taskId: 't' }) as never
    let snap = daemon([])
    snap = applyDaemonEvent(snap, { type: 'selectionPending', data: req('r1') })
    snap = applyDaemonEvent(snap, { type: 'selectionPending', data: req('r1') })
    expect(snap.pendingSelections.length).toBe(1)
    snap = applyDaemonEvent(snap, { type: 'selectionResolved', data: { requestId: 'r1' } })
    expect(snap.pendingSelections.length).toBe(0)
  })

  test('daemonConnectionChanged(false) 清空 taskRuntime', () => {
    const snap = { daemon: daemon([task('t', 1)], { t: runtime('t', 1) }), daemonConnected: true } as unknown as AgentSnapshot
    const next = applyAgentEvent(snap, { type: 'daemonConnectionChanged', data: false })
    expect(next.daemonConnected).toBe(false)
    expect(Object.keys(next.daemon.taskRuntime).length).toBe(0)
  })

  test('webhooksChanged 按 deliveryId 合并、按时间降序、封顶；空增量保留历史，webhooksCleared 才清空', () => {
    const delivery = (deliveryId: string, timestampMs: number, success = true) =>
      ({ deliveryId, timestampMs, success }) as unknown as WebhookDeliveryDto
    let snap = { ...daemon([]), webhookDeliveries: [] } as DaemonSnapshot
    snap = applyDaemonEvent(snap, { type: 'webhooksChanged', data: [delivery('b', 20), delivery('a', 10)] })
    snap = applyDaemonEvent(snap, { type: 'webhooksChanged', data: [delivery('a', 30, false), delivery('c', 15)] })
    expect(snap.webhookDeliveries.map((item) => item.deliveryId)).toEqual(['a', 'b', 'c'])
    expect(snap.webhookDeliveries[0]?.success).toBe(false)

    const kept = applyDaemonEvent(snap, { type: 'webhooksChanged', data: [] })
    expect(kept.webhookDeliveries).toBe(snap.webhookDeliveries)
    const keptEngine = applyDaemonEvent(snap, { type: 'engine', data: { type: 'webhookDeliveriesChanged', deliveries: [] } })
    expect(keptEngine.webhookDeliveries).toBe(snap.webhookDeliveries)
    expect(applyDaemonEvent(snap, { type: 'webhooksCleared' }).webhookDeliveries).toEqual([])

    const flood = Array.from({ length: WEBHOOK_DELIVERY_LIMIT + 5 }, (_, n) => delivery(`d${n}`, n)).reverse()
    const capped = applyDaemonEvent(snap, { type: 'webhooksChanged', data: flood })
    expect(capped.webhookDeliveries.length).toBe(WEBHOOK_DELIVERY_LIMIT)
    expect(capped.webhookDeliveries[0]?.timestampMs).toBe(WEBHOOK_DELIVERY_LIMIT + 4)
  })
})

describe('apply cloud notify', () => {
  const state = {
    reporting: true,
    includeUrl: true,
    includeSaveDir: false,
    catalog: [{ kind: 'email', available: true }],
    overview: { enabled: true, channels: [], maxChannels: 2, accountEmail: 'a@b.c', usage: {} },
    loading: true,
    lastErrorReason: 'cloudUnreachable',
    updatedAtUnixMs: 1,
  } as unknown as NonNullable<AgentSnapshot['cloudNotify']>

  test('cloudNotifyChanged replaces the state; taskNotice never changes the snapshot', () => {
    const initial = { cloudNotify: undefined, daemon: daemon([]) } as unknown as AgentSnapshot
    const next = applyAgentEvent(initial, { type: 'cloudNotifyChanged', data: state })
    expect(next.cloudNotify).toBe(state)
    const notice = applyAgentEvent(next, {
      type: 'daemon',
      data: { type: 'taskNotice', data: { deliveryId: 'd', event: 'task.completed', timestampMs: 1, queueId: 'q', queueName: 'Q' } },
    })
    expect(notice).toBe(next)
  })

  test('plugin view events never change the snapshot', () => {
    const snapshot = { cloudNotify: undefined, daemon: daemon([]) } as unknown as AgentSnapshot
    const events: DaemonEvent[] = [
      { type: 'pluginViewFrame', data: { session: 's', seq: 1, ops: [{ op: 'error', message: 'x' }] } },
      { type: 'pluginViewClosed', data: { session: 's', reason: 'closed' } },
      { type: 'pluginToast', data: { identity: 'a@b', level: 'info', text: 'hi' } },
    ]
    for (const data of events) {
      expect(applyAgentEvent(snapshot, { type: 'daemon', data })).toBe(snapshot)
    }
  })

  test('session end clears overview, loading and error but keeps local preferences and the public catalog', () => {
    const snapshot = { cloudNotify: state, cloudDevices: [], remoteTasks: [] } as unknown as AgentSnapshot
    const next = applyAgentEvent(snapshot, { type: 'sessionChanged', data: null })
    expect(next.cloudNotify).toEqual({
      reporting: true,
      catalog: [{ kind: 'email', available: true }],
      includeUrl: true,
      includeSaveDir: false,
      overview: null,
      loading: false,
      lastErrorReason: null,
      updatedAtUnixMs: null,
      recentDeliveries: [],
      recentNextCursor: null,
    })
  })
})
