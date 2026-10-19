// 事件应用：`native/protocol/src/event.rs` 的 `apply_agent_event` / `apply_daemon_event` /
// `apply_engine_message` 逐条移植。全部为不可变更新（返回新对象，未变化的分支保持引用不变），
// 以便 React 选择器按引用比较。

import type {
  AgentEvent,
  AgentSnapshot,
  DaemonEvent,
  DaemonSnapshot,
  TaskDto,
  TaskRuntimeDto,
  WebhookDeliveryDto,
  WsServerMsg,
} from './protocol'

/** 状态 1（下载中）/ 5（准备中）以外的任务视为不活跃。 */
function isActiveStatus(status: number): boolean {
  return status === 1 || status === 5
}

/** 投递日志保留上限（对应 Rust `WEBHOOK_DELIVERY_LIMIT`）。 */
export const WEBHOOK_DELIVERY_LIMIT = 1000

/**
 * 按 `deliveryId` 合并投递记录增量（对应 Rust `merge_webhook_deliveries`）：同 id 以增量为准，
 * 结果按 `timestampMs` 降序并截到上限。增量为空时返回原数组引用——清空由 `webhooksCleared` 表达。
 */
export function mergeWebhookDeliveries(
  current: WebhookDeliveryDto[],
  delta: readonly WebhookDeliveryDto[],
): WebhookDeliveryDto[] {
  if (delta.length === 0) return current
  const incoming = new Set(delta.map((delivery) => delivery.deliveryId))
  const merged = current.filter((delivery) => !incoming.has(delivery.deliveryId)).concat(delta)
  merged.sort((a, b) => b.timestampMs - a.timestampMs)
  return merged.slice(0, WEBHOOK_DELIVERY_LIMIT)
}

function withoutKey<V>(record: Readonly<Record<string, V>>, key: string): Record<string, V> {
  if (!(key in record)) return record as Record<string, V>
  const next = { ...record }
  delete next[key]
  return next
}

/** 读数已清零时为 true，可复用原引用。 */
function isRuntimeCleared(runtime: TaskRuntimeDto): boolean {
  return (
    runtime.activeTransfers === 0 &&
    runtime.connectedPeers === 0 &&
    runtime.segments.every((segment) => !segment.active)
  )
}

/** 清零传输活跃读数（对应 Rust `clear_active_runtime`）。 */
function clearedRuntime(runtime: TaskRuntimeDto): TaskRuntimeDto {
  if (isRuntimeCleared(runtime)) return runtime
  return {
    ...runtime,
    activeTransfers: 0,
    connectedPeers: 0,
    segments: runtime.segments.map((segment) => ({ ...segment, active: false })),
  }
}

function clearActiveRuntime(snapshot: DaemonSnapshot, taskId: string): DaemonSnapshot {
  const runtime = snapshot.taskRuntime[taskId]
  if (!runtime) return snapshot
  const cleared = clearedRuntime(runtime)
  if (cleared === runtime) return snapshot
  return { ...snapshot, taskRuntime: { ...snapshot.taskRuntime, [taskId]: cleared } }
}

/**
 * 一批 `taskChanged`（对应 Rust `DaemonEvent::TasksChanged`）：语义等同按序逐条应用，
 * 但整批只复制一次任务数组（建任务块通常上百条，逐条复制是 O(N·B)）；
 * 任务索引只建一次，`taskRuntime` 至多复制一次。
 */
function applyTasksChanged(snapshot: DaemonSnapshot, changed: readonly TaskDto[]): DaemonSnapshot {
  if (changed.length === 0) return snapshot
  const tasks = snapshot.tasks.slice()
  const indexById = new Map<string, number>()
  for (let index = 0; index < tasks.length; index += 1) indexById.set((tasks[index] as TaskDto).taskId, index)
  let taskRuntime = snapshot.taskRuntime
  for (const task of changed) {
    const index = indexById.get(task.taskId)
    if (index === undefined) {
      indexById.set(task.taskId, tasks.length)
      tasks.push(task)
    } else {
      tasks[index] = task
    }
    if (isActiveStatus(task.status)) continue
    const runtime = taskRuntime[task.taskId]
    if (!runtime) continue
    const cleared = clearedRuntime(runtime)
    if (cleared === runtime) continue
    if (taskRuntime === snapshot.taskRuntime) taskRuntime = { ...taskRuntime }
    taskRuntime[task.taskId] = cleared
  }
  return taskRuntime === snapshot.taskRuntime ? { ...snapshot, tasks } : { ...snapshot, tasks, taskRuntime }
}

/**
 * 一批 `taskDeleted`（对应 Rust `DaemonEvent::TasksDeleted`）：语义等同按序逐条应用，
 * 但整批一次过滤任务数组、`taskRuntime` 至多复制一次；没有任何命中时保持原引用。
 */
function applyTasksDeleted(snapshot: DaemonSnapshot, taskIds: readonly string[]): DaemonSnapshot {
  if (taskIds.length === 0) return snapshot
  const deleted = new Set(taskIds)
  const tasks = snapshot.tasks.filter((task) => !deleted.has(task.taskId))
  let taskRuntime = snapshot.taskRuntime
  for (const taskId of deleted) {
    if (!(taskId in taskRuntime)) continue
    if (taskRuntime === snapshot.taskRuntime) taskRuntime = { ...taskRuntime }
    delete taskRuntime[taskId]
  }
  if (tasks.length === snapshot.tasks.length && taskRuntime === snapshot.taskRuntime) return snapshot
  return { ...snapshot, tasks, taskRuntime }
}

function updateTask(
  snapshot: DaemonSnapshot,
  taskId: string,
  update: (task: TaskDto) => TaskDto,
): DaemonSnapshot {
  const index = snapshot.tasks.findIndex((task) => task.taskId === taskId)
  if (index < 0) return snapshot
  const tasks = snapshot.tasks.slice()
  tasks[index] = update(snapshot.tasks[index] as TaskDto)
  return { ...snapshot, tasks }
}

/** 采样序号不落后且任务存在时返回任务状态（对应 Rust `accepted_runtime_status`）。 */
export function acceptedRuntimeStatus(snapshot: DaemonSnapshot, runtime: TaskRuntimeDto): number | null {
  const previous = snapshot.taskRuntime[runtime.taskId]
  if (previous && previous.sampleSequence !== 0 && runtime.sampleSequence <= previous.sampleSequence) {
    return null
  }
  const task = snapshot.tasks.find((item) => item.taskId === runtime.taskId)
  return task ? task.status : null
}

function applyEngineMessage(snapshot: DaemonSnapshot, message: WsServerMsg): DaemonSnapshot {
  switch (message.type) {
    case 'tasksSnapshot': {
      // 单遍 O(N)：新 taskRuntime 只分配一次，不活跃任务就地写入清零值。
      const taskRuntime: Record<string, TaskRuntimeDto> = {}
      const live = new Map<string, boolean>()
      for (const task of message.tasks) live.set(task.taskId, isActiveStatus(task.status))
      for (const [id, runtime] of Object.entries(snapshot.taskRuntime)) {
        const active = live.get(id)
        if (active === undefined) continue
        taskRuntime[id] = active ? runtime : clearedRuntime(runtime)
      }
      return { ...snapshot, tasks: message.tasks, taskRuntime }
    }
    case 'taskProgress': {
      if (message.status === 4 && message.errorMessage === 'deleted') {
        return {
          ...snapshot,
          taskRuntime: withoutKey(snapshot.taskRuntime, message.taskId),
          tasks: snapshot.tasks.filter((task) => task.taskId !== message.taskId),
        }
      }
      let next = snapshot
      if (!isActiveStatus(message.status)) next = clearActiveRuntime(next, message.taskId)
      return updateTask(next, message.taskId, (task) => ({
        ...task,
        status: message.status,
        downloadedBytes: message.downloadedBytes,
        totalBytes: message.totalBytes,
        fileName: message.fileName !== '' ? message.fileName : task.fileName,
        saveDir: message.saveDir !== '' ? message.saveDir : task.saveDir,
        url: message.url !== '' ? message.url : task.url,
        errorMessage: message.errorMessage,
        uploadedBytes: message.uploadedBytes,
        seedingStatus: message.seedingStatus,
        seedingMessage: message.seedingMessage,
        seedingTimeSecs: message.seedingTimeSecs,
      }))
    }
    case 'segmentProgress': {
      const task = snapshot.tasks.find((item) => item.taskId === message.taskId)
      if (!task) return snapshot
      const inactive = !isActiveStatus(task.status)
      const existing = snapshot.taskRuntime[message.taskId]
      // 真实传输采样存在时不得被持久几何覆盖（对应 Rust 注释）。
      if (existing && existing.sampleSequence !== 0) return snapshot
      const base: TaskRuntimeDto = existing ?? {
        taskId: message.taskId,
        sampleSequence: 0,
        totalBytes: 0,
        segments: [],
        activeTransfers: inactive ? 0 : null,
      }
      const runtime: TaskRuntimeDto = {
        ...base,
        totalBytes: message.totalBytes,
        segments: message.segments.map((segment) => ({
          index: segment.index,
          startByte: segment.startByte,
          endByte: segment.endByte,
          downloadedBytes: segment.downloadedBytes,
          active: inactive ? false : null,
        })),
      }
      return { ...snapshot, taskRuntime: { ...snapshot.taskRuntime, [message.taskId]: runtime } }
    }
    case 'taskMetaProbed':
      return updateTask(snapshot, message.taskId, (task) => ({
        ...task,
        fileName: message.fileName !== '' ? message.fileName : task.fileName,
        totalBytes: message.totalBytes,
      }))
    case 'taskQueueChanged':
      return updateTask(snapshot, message.taskId, (task) => ({ ...task, queueId: message.queueId }))
    case 'taskRouteChanged':
      return updateTask(snapshot, message.taskId, (task) => ({ ...task, autoRoute: message.route }))
    case 'queuesChanged':
      return { ...snapshot, queues: message.queues }
    case 'queuePositionsChanged':
      return { ...snapshot, queuePositions: message.positions }
    case 'groupsChanged':
      return { ...snapshot, groups: message.groups }
    case 'rssSourcesChanged':
      return { ...snapshot, rssSources: message.sources }
    case 'rssItemsChanged':
      return {
        ...snapshot,
        rssItemRevisions: {
          ...snapshot.rssItemRevisions,
          [message.sourceId]: (snapshot.rssItemRevisions[message.sourceId] ?? 0) + 1,
        },
      }
    case 'webhookDeliveriesChanged':
      return { ...snapshot, webhookDeliveries: mergeWebhookDeliveries(snapshot.webhookDeliveries, message.deliveries) }
    case 'fileMissingChanged': {
      let next = snapshot
      for (const update of message.updates) {
        next = updateTask(next, update.taskId, (task) => ({ ...task, fileMissing: update.missing }))
      }
      return next
    }
    case 'priorityTaskChanged':
      return { ...snapshot, priority: message.priorityTaskId !== '' ? [message.priorityTaskId] : [] }
    case 'pluginAutoDisabled': {
      const index = snapshot.plugins.findIndex((plugin) => plugin.identity === message.identity)
      if (index < 0) return snapshot
      const plugins = snapshot.plugins.slice()
      plugins[index] = { ...(snapshot.plugins[index] as (typeof plugins)[number]), enabled: false, disabledReason: message.reason }
      return { ...snapshot, plugins }
    }
    default:
      return snapshot
  }
}

export function applyDaemonEvent(snapshot: DaemonSnapshot, event: DaemonEvent): DaemonSnapshot {
  switch (event.type) {
    case 'taskRuntimeChanged': {
      const status = acceptedRuntimeStatus(snapshot, event.data)
      if (status === null) return snapshot
      const inactive = !isActiveStatus(status)
      const previous = snapshot.taskRuntime[event.data.taskId]
      let runtime: TaskRuntimeDto = event.data
      if (runtime.segments.length === 0 && previous) runtime = { ...runtime, segments: previous.segments }
      if (inactive) {
        runtime = {
          ...runtime,
          activeTransfers: 0,
          segments: runtime.segments.map((segment) => ({ ...segment, active: false })),
        }
      }
      return { ...snapshot, taskRuntime: { ...snapshot.taskRuntime, [runtime.taskId]: runtime } }
    }
    case 'taskActivityAdded':
    case 'taskNotice':
    case 'pluginViewFrame':
    case 'pluginViewClosed':
    case 'pluginToast':
      return snapshot
    case 'snapshotReplaced':
      return event.data
    case 'engine':
      return applyEngineMessage(snapshot, event.data)
    case 'taskChanged': {
      const task = event.data
      const exists = snapshot.tasks.some((item) => item.taskId === task.taskId)
      let next: DaemonSnapshot = exists
        ? updateTask(snapshot, task.taskId, () => task)
        : { ...snapshot, tasks: [...snapshot.tasks, task] }
      if (!isActiveStatus(task.status)) next = clearActiveRuntime(next, task.taskId)
      return next
    }
    case 'tasksChanged':
      return applyTasksChanged(snapshot, event.data)
    case 'tasksDeleted':
      return applyTasksDeleted(snapshot, event.data.taskIds)
    case 'taskDeleted':
      return {
        ...snapshot,
        tasks: snapshot.tasks.filter((task) => task.taskId !== event.data.taskId),
        taskRuntime: withoutKey(snapshot.taskRuntime, event.data.taskId),
      }
    case 'queuesChanged':
      return { ...snapshot, queues: event.data }
    case 'groupsChanged':
      return { ...snapshot, groups: event.data }
    case 'configChanged':
      return { ...snapshot, config: event.data }
    case 'rssChanged':
      return {
        ...snapshot,
        rssItemRevisions: { ...snapshot.rssItemRevisions, [event.data.sourceId]: event.data.itemRevision },
      }
    case 'pluginsChanged':
      return { ...snapshot, plugins: event.data }
    case 'componentsChanged':
      return { ...snapshot, components: event.data }
    case 'webhooksChanged':
      return { ...snapshot, webhookDeliveries: mergeWebhookDeliveries(snapshot.webhookDeliveries, event.data) }
    case 'webhooksCleared':
      return snapshot.webhookDeliveries.length === 0 ? snapshot : { ...snapshot, webhookDeliveries: [] }
    case 'runtimeStatsChanged':
      return { ...snapshot, runtimeStats: event.data }
    case 'selectionPending':
      return {
        ...snapshot,
        pendingSelections: [
          ...snapshot.pendingSelections.filter((item) => item.requestId !== event.data.requestId),
          event.data,
        ],
      }
    case 'selectionResolved':
      return {
        ...snapshot,
        pendingSelections: snapshot.pendingSelections.filter((item) => item.requestId !== event.data.requestId),
      }
  }
}

export function applyAgentEvent(snapshot: AgentSnapshot, event: AgentEvent): AgentSnapshot {
  switch (event.type) {
    case 'daemon': {
      const daemon = applyDaemonEvent(snapshot.daemon, event.data)
      return daemon === snapshot.daemon ? snapshot : { ...snapshot, daemon }
    }
    case 'daemonSnapshotReplaced':
      return { ...snapshot, daemon: event.data }
    case 'daemonConnectionChanged':
      return {
        ...snapshot,
        daemonConnected: event.data,
        daemon: event.data ? snapshot.daemon : { ...snapshot.daemon, taskRuntime: {} },
      }
    case 'sessionChanged':
      // 会话结束后账号维度投影随之失效（对应 Rust apply_agent_event）。
      return event.data === null
        ? {
            ...snapshot,
            session: null,
            cloudDevices: [],
            remoteTasks: [],
            cloudConnection: { state: 'disconnected' },
            cloudNotify: snapshot.cloudNotify
              ? {
                  ...snapshot.cloudNotify,
                  overview: null,
                  loading: false,
                  lastErrorReason: null,
                  updatedAtUnixMs: null,
                  recentDeliveries: [],
                  recentNextCursor: null,
                }
              : snapshot.cloudNotify,
          }
        : { ...snapshot, session: event.data }
    case 'syncChanged':
      return { ...snapshot, sync: event.data }
    case 'updateChanged':
      return { ...snapshot, update: event.data }
    case 'cloudConnectionChanged':
      return {
        ...snapshot,
        cloudConnection: event.data,
        cloudDevices: event.data.state === 'connected'
          ? snapshot.cloudDevices
          : snapshot.cloudDevices.map((device) => device.isCurrent ? { ...device, isOnline: false } : device),
      }
    case 'preferencesChanged':
      return { ...snapshot, preferences: event.data }
    case 'gatewayChanged':
      return { ...snapshot, gateway: event.data }
    case 'cloudDevicesChanged':
      return { ...snapshot, cloudDevices: event.data }
    case 'linkedDevicesChanged':
      return { ...snapshot, linkedDevices: event.data }
    case 'linkPairingRequestsChanged':
      return { ...snapshot, linkPairingRequests: event.data }
    case 'linkDiscoveredChanged':
      return { ...snapshot, linkDiscovered: event.data }
    case 'remoteTasksChanged':
      return { ...snapshot, remoteTasks: event.data }
    case 'pendingCapturesChanged':
      return { ...snapshot, pendingCaptures: event.data }
    case 'shellChanged':
      return { ...snapshot, shell: event.data }
    case 'powerChanged':
      return { ...snapshot, power: event.data }
    case 'cloudNotifyChanged':
      return { ...snapshot, cloudNotify: event.data }
    case 'captureTasksStarted':
    case 'sessionRevoked':
      return snapshot
  }
}
