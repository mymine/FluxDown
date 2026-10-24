// 任务实时速度：TaskDto 没有 speed，只有引擎 `taskProgress` 推送里有（移植 controller.rs 的 live_speeds）。
// 页面挂载期间订阅一次服务事件；按帧合并发布，避免高频进度打爆渲染。

import { useEffect, useSyncExternalStore } from 'react'
import { subscribeServiceEvents } from '../../../lib/rpc'
import type { DaemonEvent, EventFrame } from '../../../lib/rpc'

export interface LiveSpeeds {
  /** taskId → 下载速率 B/s（仅下载中任务；其余为 0）。 */
  download: Readonly<Record<string, number>>
  /** taskId → BT 上传速率 B/s。 */
  upload: Readonly<Record<string, number>>
}

const EMPTY: LiveSpeeds = { download: {}, upload: {} }

let working: { download: Record<string, number>; upload: Record<string, number> } = { download: {}, upload: {} }
let published: LiveSpeeds = EMPTY
let dirty = false
let scheduled = false
let refCount = 0
let unsubscribe: (() => void) | null = null
const listeners = new Set<() => void>()

function flush() {
  scheduled = false
  if (!dirty) return
  dirty = false
  published = { download: { ...working.download }, upload: { ...working.upload } }
  for (const listener of listeners) listener()
}

function schedule() {
  dirty = true
  if (scheduled) return
  scheduled = true
  if (typeof requestAnimationFrame === 'function') requestAnimationFrame(flush)
  else setTimeout(flush, 16)
}

function clearAll() {
  if (Object.keys(working.download).length === 0 && Object.keys(working.upload).length === 0) return
  working = { download: {}, upload: {} }
  schedule()
}

function drop(taskId: string) {
  if (!(taskId in working.download) && !(taskId in working.upload)) return
  delete working.download[taskId]
  delete working.upload[taskId]
  schedule()
}

/** 本地做种中（已完成 + 正在做种）：保留上传速率采样。 */
const isSeedingDto = (task: { status: number; seedingStatus: number }) => task.status === 3 && task.seedingStatus === 1

function onDaemonEvent(event: DaemonEvent) {
  switch (event.type) {
    case 'snapshotReplaced':
      clearAll()
      break
    case 'taskChanged':
      if (event.data.status !== 1 && event.data.status !== 5 && !isSeedingDto(event.data)) drop(event.data.taskId)
      break
    case 'tasksChanged':
      for (const task of event.data) if (task.status !== 1 && task.status !== 5 && !isSeedingDto(task)) drop(task.taskId)
      break
    case 'taskDeleted':
      drop(event.data.taskId)
      break
    case 'tasksDeleted':
      for (const taskId of event.data.taskIds) drop(taskId)
      break
    case 'engine': {
      const message = event.data
      if (message.type === 'taskProgress') {
        if (message.status === 4 && message.errorMessage === 'deleted') {
          drop(message.taskId)
        } else {
          working.download[message.taskId] = message.status === 1 ? message.speed : 0
          working.upload[message.taskId] = message.status === 1 || isSeedingDto(message) ? message.uploadSpeed : 0
          schedule()
        }
      } else if (message.type === 'tasksSnapshot') {
        const ids = new Set(message.tasks.map((task) => task.taskId))
        for (const id of Object.keys(working.download)) if (!ids.has(id)) drop(id)
      }
      break
    }
    default:
      break
  }
}

function onFrame(frame: EventFrame) {
  const service = frame.event
  if (service.service === 'daemon') {
    onDaemonEvent(service.event)
    return
  }
  const event = service.event
  if (event.type === 'daemon') onDaemonEvent(event.data)
  else if (event.type === 'daemonSnapshotReplaced') clearAll()
  else if (event.type === 'daemonConnectionChanged' && !event.data) clearAll()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

const getSnapshot = () => published

/** 订阅实时速度；组件卸载时清空缓存并退订。 */
export function useLiveSpeeds(): LiveSpeeds {
  useEffect(() => {
    refCount += 1
    if (refCount === 1) unsubscribe = subscribeServiceEvents(onFrame)
    return () => {
      refCount -= 1
      if (refCount === 0) {
        unsubscribe?.()
        unsubscribe = null
        working = { download: {}, upload: {} }
        published = EMPTY
        dirty = false
      }
    }
  }, [])
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
