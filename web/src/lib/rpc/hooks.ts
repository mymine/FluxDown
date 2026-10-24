// 快照订阅 hooks。选择器返回「引用稳定」的值（快照子树/原始值）即可；
// 返回新构造的对象/数组时请传 `shallowEqual`，否则每次快照变化都会触发重渲染。

import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { subscribeServiceEvents } from './client'
import { tablesLoading } from './apply'
import type { ConnectionState, RpcState } from './store'
import { rpcStore } from './store'
import type { AgentSnapshot, DaemonSnapshot, EventFrame, TaskDto, TaskRuntimeDto } from './protocol'

export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false
    return a.every((item, index) => Object.is(item, b[index]))
  }
  const left = a as Record<string, unknown>
  const right = b as Record<string, unknown>
  const keys = Object.keys(left)
  if (keys.length !== Object.keys(right).length) return false
  return keys.every((key) => Object.hasOwn(right, key) && Object.is(left[key], right[key]))
}

/** 订阅整个 RpcState 的某个派生值。 */
export function useRpcSelector<T>(selector: (state: RpcState) => T, equal: (a: T, b: T) => boolean = Object.is): T {
  const read = useMemo(() => {
    let lastState: RpcState | undefined
    let lastResult: T
    return () => {
      const state = rpcStore.getPublished()
      if (lastState === state) return lastResult
      const result = selector(state)
      if (lastState !== undefined && equal(lastResult, result)) {
        lastState = state
        return lastResult
      }
      lastState = state
      lastResult = result
      return result
    }
  }, [selector, equal])
  return useSyncExternalStore(rpcStore.subscribe, read, read)
}

/** 连接状态（阶段、重试次数、下次重连时间）。 */
export function useConnection(): ConnectionState {
  return useRpcSelector(selectConnection)
}
const selectConnection = (state: RpcState) => state.connection

/** 快照未就绪（首次同步前）时为 null。 */
export function useAgentSnapshot(): AgentSnapshot | null {
  return useRpcSelector(selectSnapshot)
}
const selectSnapshot = (state: RpcState) => state.snapshot

/** 对 AgentSnapshot 做选择；快照未就绪返回 `fallback`。 */
export function useAgent<T>(
  selector: (snapshot: AgentSnapshot) => T,
  fallback: T,
  equal: (a: T, b: T) => boolean = Object.is,
): T {
  const select = useMemo(
    () => (state: RpcState) => (state.snapshot ? selector(state.snapshot) : fallback),
    [selector, fallback],
  )
  return useRpcSelector(select, equal)
}

/** 对 DaemonSnapshot 做选择。 */
export function useDaemon<T>(
  selector: (daemon: DaemonSnapshot) => T,
  fallback: T,
  equal: (a: T, b: T) => boolean = Object.is,
): T {
  const select = useMemo(
    () => (state: RpcState) => (state.snapshot ? selector(state.snapshot.daemon) : fallback),
    [selector, fallback],
  )
  return useRpcSelector(select, equal)
}

const EMPTY_TASKS: readonly TaskDto[] = []
const selectTasks = (daemon: DaemonSnapshot) => daemon.tasks

export function useTasks(): readonly TaskDto[] {
  return useDaemon(selectTasks, EMPTY_TASKS)
}

/** 单个任务（按 id）。 */
export function useTask(taskId: string | null): TaskDto | undefined {
  return useDaemon(
    (daemon) => (taskId === null ? undefined : daemon.tasks.find((task) => task.taskId === taskId)),
    undefined,
  )
}

/** 任务表载入状态：`loading` 期间 `tasks` 不是完整列表，UI 只能显示加载态。 */
export interface TaskTableStatus {
  /** `daemon.taskTotal` 或 `queueTotal` 存在（任一表尚在分页载入）。 */
  loading: boolean
  /** 服务端任务总数：载入期间取快照头里的 `taskTotal`，载入完毕取实际行数。 */
  total: number
  /** 已载入的任务行数；载入完毕等于 `total`。 */
  loaded: number
}

const TABLE_IDLE: TaskTableStatus = { loading: false, total: 0, loaded: 0 }

function selectTaskTableStatus(state: RpcState): TaskTableStatus {
  const daemon = state.snapshot?.daemon
  if (daemon === undefined) return TABLE_IDLE
  if (!tablesLoading(daemon)) return { loading: false, total: daemon.tasks.length, loaded: daemon.tasks.length }
  return { loading: true, total: daemon.taskTotal ?? daemon.tasks.length, loaded: state.tableLoad?.loadedTasks ?? 0 }
}

export function useTaskTableStatus(): TaskTableStatus {
  return useRpcSelector(selectTaskTableStatus, shallowEqual)
}

/** 任务表是否仍在载入（半份列表不得当作完整列表：计数、空态、批量范围、「任务已消失」推断都要跳过）。 */
export function useTasksLoading(): boolean {
  return useDaemon(tablesLoading, false)
}

/** 单个任务的运行时（速度/分段）。 */
export function useTaskRuntime(taskId: string | null): TaskRuntimeDto | undefined {
  return useDaemon((daemon) => (taskId === null ? undefined : daemon.taskRuntime[taskId]), undefined)
}

/** daemon 配置值（字符串 KV，`daemon.config.patch` 写入）。 */
export function useConfigValue(key: string): string | undefined {
  return useDaemon((daemon) => daemon.config.values[key], undefined)
}

const EMPTY_CONFIG: Readonly<Record<string, string>> = {}
export function useConfigValues(): Readonly<Record<string, string>> {
  return useDaemon((daemon) => daemon.config.values, EMPTY_CONFIG)
}

const EMPTY_PREFS: Readonly<Record<string, unknown>> = {}

/** agent 偏好全集（`agent.preferences.patch` 写入，键名空间见 crates/settings）。 */
export function usePreferences(): Readonly<Record<string, unknown>> {
  return useAgent((snapshot) => snapshot.preferences.values as Record<string, unknown>, EMPTY_PREFS)
}

export function usePref<T = unknown>(key: string): T | undefined {
  return useAgent((snapshot) => (snapshot.preferences.values as Record<string, unknown>)[key] as T | undefined, undefined)
}

/** 布尔偏好；未设置返回 `fallback`（对应 GPUI `pref_bool(key, default)`）。 */
export function usePrefBool(key: string, fallback: boolean): boolean {
  const value = usePref<unknown>(key)
  return typeof value === 'boolean' ? value : fallback
}

/** 订阅一次性/瞬态服务事件（不在快照里的：captureTasksStarted、engine 通知等）。 */
export function useServiceEvents(handler: (frame: EventFrame) => void): void {
  useEffect(() => subscribeServiceEvents(handler), [handler])
}
