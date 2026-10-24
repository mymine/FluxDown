// 单个订阅的条目流状态（对应 GPUI `RssController` + `RssView` 的条目/动作部分）。
//
// 组件必须以 `key={sourceId}` 挂载：切换订阅即整体重置（条目、选择、忙碌集合、动作反馈），
// 等价于 GPUI 的 `select_source` + `reset_actions`，晚到的旧订阅结果因组件卸载而被丢弃。
//
// 条目来源：`daemon.rss.getItems`。订阅的 `rssItemRevisions[sourceId]` 变化
// （`RssChanged` / 引擎 `rssItemsChanged` 都会让 store 递增）、重连恢复、手动重试都会重拉；
// 拉取期间保留旧条目（避免闪烁），并用 `loading` 区分空状态。

import { useCallback, useEffect, useRef, useState } from 'react'
import { useT } from '../../i18n'
import { rpc, shallowEqual, tablesLoading, useDaemon, useServiceEvents } from '../../lib/rpc'
import type { EventFrame, RssItemDto } from '../../lib/rpc'
import { TASK_STATE_UNKNOWN, encodeTaskState } from './format'
import type { LinkedTaskStates } from './format'

const EMPTY_ITEMS: readonly RssItemDto[] = []
const EMPTY_SET: ReadonlySet<string> = new Set()
const EMPTY_TASKS: LinkedTaskStates = {}

export type ItemAction = 'download' | 'ignore'

export interface RssItemsState {
  items: readonly RssItemDto[]
  /** 首次/重拉中（旧条目仍可见）。 */
  loading: boolean
  /** 最近一次拉取失败。 */
  loadError: boolean
  selected: ReadonlySet<string>
  busy: ReadonlySet<string>
  refreshBusy: boolean
  readBusy: boolean
  actionError: string | null
  feedback: string | null
  /** 已建任务条目关联任务的真实状态。 */
  taskStates: LinkedTaskStates
  toggle: (guid: string) => void
  setMany: (guids: readonly string[], checked: boolean) => void
  clearSelection: () => void
  act: (guids: readonly string[], action: ItemAction) => Promise<void>
  refresh: () => Promise<void>
  readAll: () => Promise<void>
  reload: () => void
  dismissFeedback: () => void
  fail: () => void
}

export function useRssItems(sourceId: string, stale: boolean): RssItemsState {
  const t = useT()
  const revision = useDaemon((daemon) => daemon.rssItemRevisions[sourceId] ?? 0, 0)
  const [items, setItems] = useState<readonly RssItemDto[]>(EMPTY_ITEMS)
  const [reloadTick, setReloadTick] = useState(0)
  const [settled, setSettled] = useState<{ key: string; ok: boolean } | null>(null)
  const [selected, setSelected] = useState<ReadonlySet<string>>(EMPTY_SET)
  const [busy, setBusy] = useState<ReadonlySet<string>>(EMPTY_SET)
  const [refreshBusy, setRefreshBusy] = useState(false)
  const [readBusy, setReadBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [feedback, setFeedback] = useState<string | null>(null)

  const itemsRef = useRef(items)
  itemsRef.current = items
  const busyRef = useRef<Set<string>>(new Set())
  const staleRef = useRef(stale)
  staleRef.current = stale

  const key = `${revision}:${reloadTick}`

  useEffect(() => {
    if (stale) return
    let cancelled = false
    rpc.daemon.rss.getItems({ sourceId }).then(
      (list) => {
        if (cancelled) return
        const guids = new Set(list.map((item) => item.guid))
        setItems(list)
        setSelected((prev) => {
          if (prev.size === 0) return prev
          const kept = [...prev].filter((guid) => guids.has(guid))
          return kept.length === prev.size ? prev : new Set(kept)
        })
        setActionError(null)
        setSettled({ key, ok: true })
      },
      () => {
        if (cancelled) return
        setActionError(t('localServiceActionFailed'))
        setSettled({ key, ok: false })
      },
    )
    return () => {
      cancelled = true
    }
  }, [sourceId, key, stale, t])

  const loading = !stale && settled?.key !== key
  const loadError = !stale && settled?.key === key && !settled.ok

  // 已建任务条目 → 关联任务的真实状态（避免每次任务进度都重渲染整表：只取被引用的任务）。
  const linkedIds = useLinkedTaskIds(items)
  const taskStates = useDaemon(
    (daemon) => {
      if (linkedIds.size === 0) return EMPTY_TASKS
      const out: Record<string, number> = {}
      // 任务表载入期间 `tasks` 不完整：「找不到任务」不代表被删，标记为未知。
      if (tablesLoading(daemon)) {
        for (const id of linkedIds) out[id] = TASK_STATE_UNKNOWN
        return out
      }
      for (const task of daemon.tasks) {
        if (linkedIds.has(task.taskId)) out[task.taskId] = encodeTaskState(task.status, task.fileMissing)
      }
      return out
    },
    EMPTY_TASKS,
    shallowEqual,
  )

  // 引擎推来新条目：提示「N 条新条目」（GPUI `rssItemsUpdated`）。
  const onFrame = useCallback(
    (frame: EventFrame) => {
      const outer = frame.event
      if (outer.service !== 'agent' || outer.event.type !== 'daemon') return
      const daemonEvent = outer.event.data
      if (daemonEvent.type !== 'engine' || daemonEvent.data.type !== 'rssItemsChanged') return
      const message = daemonEvent.data
      if (message.sourceId !== sourceId) return
      const known = new Set(itemsRef.current.map((item) => item.guid))
      const added = message.items.filter((item) => !known.has(item.guid)).length
      if (added > 0) setFeedback(t('rssItemsUpdated', { n: added }))
    },
    [sourceId, t],
  )
  useServiceEvents(onFrame)

  const reload = useCallback(() => setReloadTick((tick) => tick + 1), [])

  const toggle = useCallback((guid: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (!next.delete(guid)) next.add(guid)
      return next
    })
  }, [])

  const setMany = useCallback((guids: readonly string[], checked: boolean) => {
    setSelected((prev) => {
      const next = new Set(prev)
      for (const guid of guids) {
        if (checked) next.add(guid)
        else next.delete(guid)
      }
      return next
    })
  }, [])

  const clearSelection = useCallback(() => setSelected(EMPTY_SET), [])

  const act = useCallback(
    async (guids: readonly string[], action: ItemAction) => {
      if (staleRef.current) return
      const known = new Set(itemsRef.current.map((item) => item.guid))
      const todo = guids.filter((guid) => known.has(guid) && !busyRef.current.has(guid))
      if (todo.length === 0) return
      for (const guid of todo) busyRef.current.add(guid)
      setBusy(new Set(busyRef.current))
      setActionError(null)
      let done = 0
      let failed = 0
      for (const guid of todo) {
        try {
          await rpc.daemon.rss.itemAction({ sourceId, guid, action })
          done += 1
          setSelected((prev) => {
            if (!prev.has(guid)) return prev
            const next = new Set(prev)
            next.delete(guid)
            return next
          })
        } catch {
          failed += 1
        } finally {
          busyRef.current.delete(guid)
          setBusy(new Set(busyRef.current))
        }
      }
      if (failed > 0) setActionError(t('localServiceActionFailed'))
      setFeedback(done === 1 && failed === 0 && action === 'download' ? t('rssTaskCreated') : t('rssBatchResult', { done, failed }))
      if (done > 0) reload()
    },
    [sourceId, t, reload],
  )

  const refresh = useCallback(async () => {
    if (staleRef.current) return
    setRefreshBusy(true)
    try {
      await rpc.daemon.rss.refreshSource({ sourceId })
      reload()
    } catch {
      setActionError(t('localServiceActionFailed'))
    } finally {
      setRefreshBusy(false)
    }
  }, [sourceId, t, reload])

  const readAll = useCallback(async () => {
    if (staleRef.current) return
    setReadBusy(true)
    try {
      await rpc.daemon.rss.itemAction({ sourceId, action: 'readAll' })
      reload()
    } catch {
      setActionError(t('localServiceActionFailed'))
    } finally {
      setReadBusy(false)
    }
  }, [sourceId, t, reload])

  const dismissFeedback = useCallback(() => setFeedback(null), [])
  /** 页面级动作（删除订阅等）失败时复用同一条错误横幅。 */
  const fail = useCallback(() => setActionError(t('localServiceActionFailed')), [t])

  return {
    items,
    loading,
    loadError,
    selected,
    busy,
    refreshBusy,
    readBusy,
    actionError,
    feedback,
    taskStates,
    toggle,
    setMany,
    clearSelection,
    act,
    refresh,
    readAll,
    reload,
    dismissFeedback,
    fail,
  }
}

/** 已建任务条目引用的任务 ID 集合（引用稳定：内容不变则复用上一次的 Set）。 */
function useLinkedTaskIds(items: readonly RssItemDto[]): ReadonlySet<string> {
  const previous = useRef<ReadonlySet<string>>(EMPTY_SET)
  const ids = new Set<string>()
  for (const item of items) if (item.status === 1 && item.taskId !== '') ids.add(item.taskId)
  const same = ids.size === previous.current.size && [...ids].every((id) => previous.current.has(id))
  if (!same) previous.current = ids
  return previous.current
}
