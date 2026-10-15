// 下载页共享状态：任务行投影、筛选/搜索、视图偏好（300ms 防抖写回）、选中集、详情面板绑定。
// 页面内所有组件（侧栏 / 标题栏 / 表格 / 状态栏 / 选择条 / 详情）通过 `useDownloads()` 读取。

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../../i18n'
import { useIsMobile } from '../../ui'
import {
  rpc,
  shallowEqual,
  useAgent,
  useAgentSnapshot,
  useConnection,
  useDaemon,
  usePref,
  useTasks,
} from '../../lib/rpc'
import type {
  AgentSessionDto,
  CloudDevice,
  DaemonRuntimeStatsDto,
  GroupDto,
  JsonValue,
  LinkDeviceInfo,
  QueueDto,
  QueuePositionDto,
  RemoteTaskDto,
  TaskRuntimeDto,
} from '../../lib/rpc'
import { CategoryIndex, categoriesFromPreference } from './model/categories'
import { RescanThrottle } from '../../lib/rescanThrottle'
import { currentDeviceId, visibleRemoteTasks } from './model/devices'
import { filterMatches, LOCAL_DEVICE, SELECTION_ALL } from './model/filters'
import type { SidebarSelection } from './model/filters'
import { remoteCan } from './model/batchPlan'
import { isDownloadable } from './model/actions'
import { useLiveSpeeds } from './model/liveSpeeds'
import { isDynamicSortKey } from './model/rowOrder'
import { buildLocalView, buildRemoteView, isLocalKey, sourceSite, STATE_RANK } from './model/task'
import type { DownloadTaskView, RowKey } from './model/task'
import { useStableRowOrder } from './model/useStableRowOrder'
import { queueLabelById } from './dialogs/utils'
import { compareViews, dateBucketOf, DATE_BUCKET_ORDER, matchesQuery, parseViewPrefs, VIEW_PREFS_KEY } from './model/viewPrefs'
import type { ViewPrefs } from './model/viewPrefs'

// ── 类型 ──

/** 表格可见行：任务行或分组头。 */
export type VisibleRow =
  | { type: 'task'; key: RowKey; view: DownloadTaskView }
  | { type: 'group'; key: string; label: string; count: number; collapsed: boolean }

export interface GroupSummary {
  id: string
  name: string
  originUrl: string
  saveDir: string
  total: number
  completed: number
  failed: number
  downloading: number
  progress: number
}

/** 选中集合投影（选择条 / 工具栏）：只统计仍存在的选中任务。 */
export interface SelectionSummary {
  count: number
  any: boolean
  /** 含本地任务。 */
  anyLocal: boolean
  /** 全部为本地已完成任务（可下载文件）。 */
  allDownloadable: boolean
  /** 含下载中 / 排队（可暂停）。 */
  anyActive: boolean
  /** 含暂停 / 失败（可继续）。 */
  anyResumable: boolean
}

export interface ClickModifiers {
  shift: boolean
  /** Ctrl（Win/Linux）或 Cmd（macOS）。 */
  secondary: boolean
}

export interface DownloadsContextValue {
  /** daemon 连接就绪（false = 只读快照）。 */
  connected: boolean
  views: readonly DownloadTaskView[]
  byKey: ReadonlyMap<RowKey, DownloadTaskView>
  categories: CategoryIndex
  queues: readonly QueueDto[]
  queueName: (queueId: string) => string
  groups: readonly GroupDto[]
  groupSummaries: readonly GroupSummary[]
  cloudDevices: readonly CloudDevice[]
  linkedDevices: readonly LinkDeviceInfo[]
  runtimeStats: DaemonRuntimeStatsDto

  sidebarSelection: SidebarSelection
  setSidebarSelection: (selection: SidebarSelection) => void
  query: string
  setQuery: (query: string) => void

  prefs: ViewPrefs
  /** 修改视图偏好；300ms 防抖写回 `desktop.downloads.view`（sync:false）。返回原对象 = 无变化。 */
  updatePrefs: (mutate: (prefs: ViewPrefs) => ViewPrefs) => void

  /** 当前筛选 + 搜索 + 分组 + 排序后的可见行。 */
  rows: readonly VisibleRow[]
  /** 可见任务键（表格顺序，不含折叠分组内的任务）。 */
  visibleKeys: readonly RowKey[]
  selected: ReadonlySet<RowKey>
  selectedViews: readonly DownloadTaskView[]
  summary: SelectionSummary
  /** 显式多选模式：Ctrl/Cmd+点击、行首复选框、右键「选择 / 全选」或全选进入，普通单击 / 清空选择退出（与 GPUI `multi_select` 对齐）。 */
  multiSelect: boolean
  /** 点击行：shift 连选，ctrl/cmd 切换，否则单选。 */
  clickSelect: (key: RowKey, modifiers: ClickModifiers) => void
  /** 只切换该行选中状态（复选框 / 移动端点选）。 */
  toggleSelected: (key: RowKey) => void
  /** 桌面表格行首复选框（与 GPUI `toggle_multi_select` 对齐）：未处于多选模式时以该行开启多选，否则切换该行。 */
  toggleMultiSelect: (key: RowKey) => void
  /** 右键「选择」：保留当前选区并进入显式多选模式。 */
  enterMultiSelect: () => void
  /** 右键 / 长按：未选中则单选它，已选中保持当前选区。 */
  contextSelect: (key: RowKey) => void
  selectAll: () => void
  clearSelection: () => void
  toggleGroupCollapsed: (groupKey: string) => void

  /** 详情面板是否打开：桌面取偏好 `detail_open`，移动端（Sheet）为页面本地状态。 */
  detailOpen: boolean
  /** 详情面板当前承载的本地任务 id。 */
  detailTaskId: string | null
  /** 打开详情面板并切换到该任务（同时把它设为唯一选中项）。 */
  showDetail: (taskId: string) => void
  closeDetail: () => void

  /** 移动端侧栏抽屉。 */
  sidebarOpen: boolean
  setSidebarOpen: (open: boolean) => void
}

const Context = createContext<DownloadsContextValue | null>(null)

export function useDownloads(): DownloadsContextValue {
  const value = useContext(Context)
  if (!value) throw new Error('useDownloads must be used inside <DownloadsProvider>')
  return value
}

const EMPTY_RUNTIME: Readonly<Record<string, TaskRuntimeDto>> = {}
const EMPTY_PRIORITY: readonly string[] = []
const EMPTY_REMOTE: readonly RemoteTaskDto[] = []
const EMPTY_QUEUES: readonly QueueDto[] = []
const EMPTY_QUEUE_POSITIONS: readonly QueuePositionDto[] = []
const EMPTY_GROUPS: readonly GroupDto[] = []
const EMPTY_CLOUD: readonly CloudDevice[] = []
const EMPTY_LINKED: readonly LinkDeviceInfo[] = []
const EMPTY_STATS: DaemonRuntimeStatsDto = {
  activeTasks: 0,
  pendingTasks: 0,
  totalDownloadBps: 0,
  totalUploadBps: 0,
  diskFreeBytes: null,
  saveDir: '',
}
const EMPTY_SELECTED: ReadonlySet<RowKey> = new Set()
const PREFS_DEBOUNCE_MS = 300

const selectRuntime = (daemon: { taskRuntime: Record<string, TaskRuntimeDto> }) => daemon.taskRuntime
const selectPriority = (daemon: { priority: string[] }) => daemon.priority
const selectQueuePositions = (daemon: { queuePositions: QueuePositionDto[] }) => daemon.queuePositions
const selectQueues = (daemon: { queues: QueueDto[] }) => daemon.queues
const selectGroups = (daemon: { groups: GroupDto[] }) => daemon.groups
const selectStats = (daemon: { runtimeStats: DaemonRuntimeStatsDto }) => daemon.runtimeStats
const selectRemote = (snapshot: { remoteTasks: RemoteTaskDto[] }) => snapshot.remoteTasks
const selectDaemonConnected = (snapshot: { daemonConnected: boolean }) => snapshot.daemonConnected
const selectSession = (snapshot: { session: AgentSessionDto | null }) => snapshot.session
const selectCloud = (snapshot: { cloudDevices: CloudDevice[] }) => snapshot.cloudDevices
const selectLinked = (snapshot: { linkedDevices: LinkDeviceInfo[] }) => snapshot.linkedDevices

// ── 视图偏好：同步 + 防抖写回 ──

function useViewPrefsState(): { prefs: ViewPrefs; updatePrefs: DownloadsContextValue['updatePrefs'] } {
  const serverValue = usePref<unknown>(VIEW_PREFS_KEY)
  const snapshot = useAgentSnapshot()
  const tasks = useTasks()
  const [prefs, setPrefs] = useState<ViewPrefs>(() => parseViewPrefs(serverValue))
  const prefsRef = useRef(prefs)
  /** 最近一次应用 / 写出的规范化序列化值：自己写出的值回流不重载。 */
  const applied = useRef(serverValue === undefined ? undefined : JSON.stringify(parseViewPrefs(serverValue)))
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const defaultsDecided = useRef(false)

  const flush = useCallback(() => {
    if (timer.current === null) return
    clearTimeout(timer.current)
    timer.current = null
    const current = prefsRef.current
    applied.current = JSON.stringify(current)
    rpc.agent.preferences
      .patch({ values: { [VIEW_PREFS_KEY]: current as unknown as JsonValue }, sync: false })
      .catch(() => undefined)
  }, [])

  useEffect(() => {
    if (serverValue === undefined) return
    const normalized = JSON.stringify(parseViewPrefs(serverValue))
    if (normalized === applied.current) return
    applied.current = normalized
    const next = parseViewPrefs(serverValue)
    prefsRef.current = next
    setPrefs(next)
  }, [serverValue])

  // 无偏好时首次按「存在组任务」决定默认分组维度（不写回）。
  useEffect(() => {
    if (defaultsDecided.current || !snapshot) return
    defaultsDecided.current = true
    if (serverValue === undefined && tasks.some((task) => task.groupId !== '')) {
      const next: ViewPrefs = { ...prefsRef.current, group_by: 'group' }
      prefsRef.current = next
      setPrefs(next)
    }
  }, [snapshot, serverValue, tasks])

  useEffect(() => flush, [flush])

  const updatePrefs = useCallback<DownloadsContextValue['updatePrefs']>(
    (mutate) => {
      const next = mutate(prefsRef.current)
      if (next === prefsRef.current) return
      prefsRef.current = next
      setPrefs(next)
      if (timer.current !== null) clearTimeout(timer.current)
      timer.current = setTimeout(flush, PREFS_DEBOUNCE_MS)
    },
    [flush],
  )
  return { prefs, updatePrefs }
}

// ── 任务行投影（复用未变化的行对象，行组件才能 memo） ──

interface CachedView {
  task: object
  speed: number | null
  boosted: boolean
  queuePosition: number
  runtime: TaskRuntimeDto | undefined
  connected: boolean
  view: DownloadTaskView
}

// ── Provider ──

export function DownloadsProvider({ children }: { children: ReactNode }) {
  const t = useT()
  const tasks = useTasks()
  const runtimes = useDaemon(selectRuntime, EMPTY_RUNTIME)
  const priority = useDaemon(selectPriority, EMPTY_PRIORITY as string[])
  const queuePositions = useDaemon(selectQueuePositions, EMPTY_QUEUE_POSITIONS as QueuePositionDto[])
  const queues = useDaemon(selectQueues, EMPTY_QUEUES as QueueDto[])
  const groups = useDaemon(selectGroups, EMPTY_GROUPS as GroupDto[])
  const runtimeStats = useDaemon(selectStats, EMPTY_STATS, shallowEqual)
  const remoteTasks = useAgent(selectRemote, EMPTY_REMOTE as RemoteTaskDto[])
  const daemonConnected = useAgent(selectDaemonConnected, false)
  const session = useAgent(selectSession, null)
  const cloudDevices = useAgent(selectCloud, EMPTY_CLOUD as CloudDevice[])
  const linkedDevices = useAgent(selectLinked, EMPTY_LINKED as LinkDeviceInfo[])
  const categoriesPref = usePref<unknown>('custom_categories')
  const phase = useConnection().phase
  const live = useLiveSpeeds()
  const connected = phase === 'ready' && daemonConnected

  // 页面可见 / 连接就绪时重扫已完成文件（daemon 空闲不再定时扫描）；节流镜像 GPUI RescanThrottle。
  const rescanThrottle = useRef(new RescanThrottle())
  const rescanTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    if (!connected) return
    const fire = () => {
      rpc.daemon.task.rescan().catch(() => {})
    }
    const request = () => {
      if (document.visibilityState !== 'visible') return
      const decision = rescanThrottle.current.request(Date.now())
      if (decision.kind === 'now') fire()
      else if (decision.kind === 'after') {
        rescanTimer.current = setTimeout(() => {
          rescanTimer.current = null
          rescanThrottle.current.trailingFired(Date.now())
          fire()
        }, decision.delayMs)
      }
    }
    request()
    document.addEventListener('visibilitychange', request)
    return () => document.removeEventListener('visibilitychange', request)
  }, [connected])
  useEffect(
    () => () => {
      if (rescanTimer.current !== null) clearTimeout(rescanTimer.current)
    },
    [],
  )

  const { prefs, updatePrefs } = useViewPrefsState()
  const [sidebarSelection, setSidebarSelectionState] = useState<SidebarSelection>(SELECTION_ALL)
  const [query, setQueryState] = useState('')
  const [selected, setSelected] = useState<ReadonlySet<RowKey>>(EMPTY_SELECTED)
  const [multiSelectMode, setMultiSelectMode] = useState(false)
  const anchor = useRef<RowKey | null>(null)
  const [detailTaskId, setDetailTaskId] = useState<string | null>(null)
  const mobile = useIsMobile()
  const [mobileDetailOpen, setMobileDetailOpen] = useState(false)
  const detailOpen = mobile ? mobileDetailOpen : prefs.detail_open
  const [sidebarOpen, setSidebarOpen] = useState(false)

  const categories = useMemo(() => new CategoryIndex(categoriesFromPreference(categoriesPref)), [categoriesPref])

  // 本地行：复用缓存
  const cache = useRef(new Map<string, CachedView>())
  const boostedId = priority[0] ?? null
  const positionById = useMemo(
    () => new Map(queuePositions.map((entry) => [entry.taskId, entry.position])),
    [queuePositions],
  )
  const localViews = useMemo(() => {
    const previous = cache.current
    const next = new Map<string, CachedView>()
    const views = tasks.map((task) => {
      const speed = live.download[task.taskId] ?? null
      const boosted = boostedId === task.taskId
      const queuePosition = positionById.get(task.taskId) ?? 0
      const runtime = runtimes[task.taskId]
      const hit = previous.get(task.taskId)
      if (
        hit &&
        hit.task === task &&
        hit.speed === speed &&
        hit.boosted === boosted &&
        hit.queuePosition === queuePosition &&
        hit.runtime === runtime &&
        hit.connected === connected
      ) {
        next.set(task.taskId, hit)
        return hit.view
      }
      const view = buildLocalView(task, speed, boosted, queuePosition, runtime, connected)
      next.set(task.taskId, { task, speed, boosted, queuePosition, runtime, connected, view })
      return view
    })
    cache.current = next
    return views
  }, [tasks, live, boostedId, positionById, runtimes, connected])

  const currentId = useMemo(() => currentDeviceId(session, cloudDevices), [session, cloudDevices])
  const remoteViews = useMemo(
    () => visibleRemoteTasks(remoteTasks, currentId).map(buildRemoteView),
    [remoteTasks, currentId],
  )
  const views = useMemo(() => [...localViews, ...remoteViews], [localViews, remoteViews])
  const byKey = useMemo(() => new Map(views.map((view) => [view.key, view])), [views])

  const queueName = useCallback((queueId: string) => queueLabelById(t, queues, queueId), [t, queues])
  const groupNames = useMemo(() => new Map(groups.map((group) => [group.groupId, group.name])), [groups])

  const groupSummaries = useMemo<GroupSummary[]>(() => {
    const summaries = new Map<string, GroupSummary & { downloaded: number; totalBytes: number }>()
    for (const group of groups) {
      summaries.set(group.groupId, {
        id: group.groupId,
        name: group.name,
        originUrl: group.sourceUrl,
        saveDir: group.saveDir,
        total: 0,
        completed: 0,
        failed: 0,
        downloading: 0,
        progress: 0,
        downloaded: 0,
        totalBytes: 0,
      })
    }
    for (const view of localViews) {
      if (view.groupId === '') continue
      const summary = summaries.get(view.groupId)
      if (!summary) continue
      summary.total += 1
      if (view.state === 'completed') summary.completed += 1
      else if (view.state === 'failed') summary.failed += 1
      else if (view.state === 'downloading') summary.downloading += 1
      summary.downloaded += view.downloadedBytes
      summary.totalBytes += view.sizeBytes
    }
    return [...summaries.values()].map(({ downloaded, totalBytes, ...summary }) => ({
      ...summary,
      progress:
        totalBytes > 0
          ? Math.min(1, downloaded / totalBytes)
          : summary.total > 0
            ? summary.completed / summary.total
            : 0,
    }))
  }, [groups, localViews])

  const normalizedQuery = useMemo(() => query.trim().toLowerCase(), [query])

  // 筛选 + 搜索
  const matching = useMemo(() => {
    const matchesSelection = (view: DownloadTaskView): boolean => {
      switch (sidebarSelection.kind) {
        case 'download':
          return filterMatches(sidebarSelection.filter, view, categories)
        case 'queue':
          return view.source === 'local' && view.queueId === sidebarSelection.queueId
        case 'device': {
          const device = sidebarSelection.deviceId
          if (device === LOCAL_DEVICE) return view.source === 'local'
          return view.source === 'remote' && view.toDevice === device
        }
      }
    }
    return views.filter((view) => matchesSelection(view) && matchesQuery(view, normalizedQuery))
  }, [views, sidebarSelection, categories, normalizedQuery])
  const matchingKeys = useMemo(() => new Set(matching.map((view) => view.key)), [matching])

  // 排序 + 行顺序稳定（保持期内仅内容变化不改变已有行的相对顺序，见 model/rowOrder.ts）
  const sorted = useMemo(() => matching.slice().sort((a, b) => compareViews(prefs, a, b)), [matching, prefs])
  const ordered = useStableRowOrder(
    sorted,
    views,
    [sidebarSelection, normalizedQuery, categories, prefs.sort_key, prefs.sort_dir],
    isDynamicSortKey(prefs.sort_key),
  )

  // 分组
  const { rows, visibleKeys } = useMemo(() => {
    const taskRow = (view: DownloadTaskView): VisibleRow => ({ type: 'task', key: view.key, view })
    if (prefs.group_by === 'none') {
      return { rows: ordered.map(taskRow), visibleKeys: ordered.map((view) => view.key) }
    }

    interface Bucket {
      key: string
      label: string
      order: number
      views: DownloadTaskView[]
    }
    const buckets: Bucket[] = []
    const bucketOf = (view: DownloadTaskView): Omit<Bucket, 'views'> => {
      switch (prefs.group_by) {
        case 'status':
          return { key: `status:${view.state}`, label: stateLabel(t, view.state), order: STATE_RANK[view.state] }
        case 'date': {
          const bucket = dateBucketOf(view.createdAtSecs)
          return { key: `date:${bucket}`, label: t(DATE_LABEL_KEY[bucket]), order: DATE_BUCKET_ORDER[bucket] }
        }
        case 'type': {
          const id = categories.categoryOf(view)
          const index = categories.rules.findIndex((rule) => rule.dto.id === id)
          const rule = categories.rules[index]
          return {
            key: `type:${id}`,
            label: rule ? categoryLabel(t, rule.dto) : t('categoryOther'),
            order: rule ? index : Number.MAX_SAFE_INTEGER,
          }
        }
        case 'queue': {
          if (view.source === 'remote') {
            return { key: 'queue:remote', label: t('remoteTasksGroup'), order: Number.MAX_SAFE_INTEGER }
          }
          const index = queues.findIndex((queue) => queue.queueId === view.queueId)
          return {
            key: `queue:${view.queueId}`,
            label: queueLabelById(t, queues, view.queueId),
            order: index >= 0 ? index : Number.MAX_SAFE_INTEGER - 1,
          }
        }
        case 'site': {
          const site = sourceSite(view)
          const label = site !== '' ? site : view.protocol === 'bt' ? t('viewSiteBt') : '—'
          return { key: `site:${label}`, label, order: 0 }
        }
        case 'group': {
          if (view.groupId === '') {
            return { key: 'group:', label: t('ungroupedTasks'), order: Number.MAX_SAFE_INTEGER }
          }
          return { key: `group:${view.groupId}`, label: groupNames.get(view.groupId) ?? view.groupId, order: 0 }
        }
        default:
          return { key: '', label: '', order: 0 }
      }
    }
    for (const view of ordered) {
      const info = bucketOf(view)
      const existing = buckets.find((bucket) => bucket.key === info.key)
      if (existing) existing.views.push(view)
      else buckets.push({ ...info, views: [view] })
    }
    buckets.sort((a, b) => a.order - b.order || (a.label < b.label ? -1 : a.label > b.label ? 1 : 0))
    const collapsedSet = new Set(prefs.collapsed_groups)
    const out: VisibleRow[] = []
    const keys: RowKey[] = []
    for (const bucket of buckets) {
      const collapsed = collapsedSet.has(bucket.key)
      out.push({ type: 'group', key: bucket.key, label: bucket.label, count: bucket.views.length, collapsed })
      if (!collapsed) {
        for (const view of bucket.views) {
          out.push(taskRow(view))
          keys.push(view.key)
        }
      }
    }
    return { rows: out, visibleKeys: keys }
  }, [ordered, prefs, queues, groupNames, categories, t])

  // 选中集只保留当前筛选 + 搜索下仍在视图内的任务（折叠分组内的仍属于当前视图）。
  useEffect(() => {
    setSelected((current) => {
      if (current.size === 0) return current
      const kept = [...current].filter((key) => matchingKeys.has(key))
      if (kept.length === current.size) return current
      if (anchor.current !== null && !kept.includes(anchor.current)) anchor.current = null
      return kept.length === 0 ? EMPTY_SELECTED : new Set(kept)
    })
  }, [matchingKeys])

  const setSidebarSelection = useCallback((selection: SidebarSelection) => {
    setSidebarSelectionState((current) => (JSON.stringify(current) === JSON.stringify(selection) ? current : selection))
  }, [])

  // 队列被删除后不能继续筛选已不存在的队列。
  useEffect(() => {
    if (sidebarSelection.kind === 'queue' && !queues.some((queue) => queue.queueId === sidebarSelection.queueId)) {
      setSidebarSelectionState(SELECTION_ALL)
    }
  }, [queues, sidebarSelection])

  // 搜索 150ms 防抖由输入框侧完成，这里直接存值。
  const setQuery = useCallback((value: string) => setQueryState(value), [])

  const clickSelect = useCallback(
    (key: RowKey, modifiers: ClickModifiers) => {
      if (modifiers.shift && anchor.current !== null) {
        const from = visibleKeys.indexOf(anchor.current)
        const to = visibleKeys.indexOf(key)
        if (from >= 0 && to >= 0) {
          const [start, end] = from <= to ? [from, to] : [to, from]
          setSelected(new Set(visibleKeys.slice(start, end + 1)))
          anchor.current = key
          return
        }
      }
      setSelected((current) => {
        if (modifiers.secondary) {
          const next = new Set(current)
          if (!next.delete(key)) next.add(key)
          return next
        }
        return new Set([key])
      })
      setMultiSelectMode(modifiers.secondary)
      anchor.current = key
    },
    [visibleKeys],
  )

  const toggleSelected = useCallback((key: RowKey) => {
    setSelected((current) => {
      const next = new Set(current)
      if (!next.delete(key)) next.add(key)
      return next
    })
    anchor.current = key
  }, [])

  const contextSelect = useCallback((key: RowKey) => {
    setSelected((current) => {
      if (current.has(key)) return current
      setMultiSelectMode(false)
      return new Set([key])
    })
    anchor.current = key
  }, [])

  const selectAll = useCallback(() => {
    setSelected(new Set(visibleKeys))
    setMultiSelectMode(true)
    anchor.current = null
  }, [visibleKeys])

  const clearSelection = useCallback(() => {
    setSelected(EMPTY_SELECTED)
    setMultiSelectMode(false)
    anchor.current = null
  }, [])

  const multiSelect = multiSelectMode && selected.size > 0

  const toggleMultiSelect = useCallback(
    (key: RowKey) => {
      setSelected((current) => {
        if (!multiSelect) return new Set([key])
        const next = new Set(current)
        if (!next.delete(key)) next.add(key)
        return next
      })
      setMultiSelectMode(true)
      anchor.current = key
    },
    [multiSelect],
  )

  const enterMultiSelect = useCallback(() => setMultiSelectMode(true), [])

  const toggleGroupCollapsed = useCallback(
    (groupKey: string) =>
      updatePrefs((current) => ({
        ...current,
        collapsed_groups: current.collapsed_groups.includes(groupKey)
          ? current.collapsed_groups.filter((item) => item !== groupKey)
          : [...current.collapsed_groups, groupKey],
      })),
    [updatePrefs],
  )

  const selectedViews = useMemo(
    () => [...selected].map((key) => byKey.get(key)).filter((view): view is DownloadTaskView => view !== undefined),
    [selected, byKey],
  )

  const summary = useMemo<SelectionSummary>(() => {
    let anyLocal = false
    let anyActive = false
    let anyResumable = false
    let allDownloadable = selectedViews.length > 0
    for (const view of selectedViews) {
      anyLocal ||= view.source === 'local'
      if (view.state === 'downloading' || view.state === 'pending') anyActive ||= remoteCan(view, 'pause')
      else if (view.state === 'paused' || view.state === 'failed') anyResumable ||= remoteCan(view, 'resume')
      if (!isDownloadable(view)) allDownloadable = false
    }
    return {
      count: selectedViews.length,
      any: selectedViews.length > 0,
      anyLocal,
      allDownloadable,
      anyActive,
      anyResumable,
    }
  }, [selectedViews])

  // 详情面板打开时跟随「恰好一个本地选中任务」。
  const singleLocal = selected.size === 1 ? [...selected][0] : undefined
  useEffect(() => {
    if (detailOpen && singleLocal !== undefined && isLocalKey(singleLocal)) {
      setDetailTaskId(singleLocal.slice(2))
    }
  }, [detailOpen, singleLocal])

  const showDetail = useCallback(
    (taskId: string) => {
      setDetailTaskId(taskId)
      setSelected(new Set([`l:${taskId}`]))
      anchor.current = `l:${taskId}`
      if (mobile) setMobileDetailOpen(true)
      else updatePrefs((current) => (current.detail_open ? current : { ...current, detail_open: true }))
    },
    [updatePrefs, mobile],
  )

  const closeDetail = useCallback(() => {
    setDetailTaskId(null)
    if (mobile) setMobileDetailOpen(false)
    else updatePrefs((current) => (current.detail_open ? { ...current, detail_open: false } : current))
  }, [updatePrefs, mobile])

  const value = useMemo<DownloadsContextValue>(
    () => ({
      connected,
      views,
      byKey,
      categories,
      queues,
      queueName,
      groups,
      groupSummaries,
      cloudDevices,
      linkedDevices,
      runtimeStats,
      sidebarSelection,
      setSidebarSelection,
      query,
      setQuery,
      prefs,
      updatePrefs,
      rows,
      visibleKeys,
      selected,
      selectedViews,
      summary,
      multiSelect,
      clickSelect,
      toggleSelected,
      toggleMultiSelect,
      enterMultiSelect,
      contextSelect,
      selectAll,
      clearSelection,
      toggleGroupCollapsed,
      detailOpen,
      detailTaskId,
      showDetail,
      closeDetail,
      sidebarOpen,
      setSidebarOpen,
    }),
    [
      connected,
      views,
      byKey,
      categories,
      queues,
      queueName,
      groups,
      groupSummaries,
      cloudDevices,
      linkedDevices,
      runtimeStats,
      sidebarSelection,
      setSidebarSelection,
      query,
      setQuery,
      prefs,
      updatePrefs,
      rows,
      visibleKeys,
      selected,
      selectedViews,
      summary,
      multiSelect,
      clickSelect,
      toggleSelected,
      toggleMultiSelect,
      enterMultiSelect,
      contextSelect,
      selectAll,
      clearSelection,
      toggleGroupCollapsed,
      detailOpen,
      detailTaskId,
      showDetail,
      closeDetail,
      sidebarOpen,
    ],
  )
  return <Context.Provider value={value}>{children}</Context.Provider>
}

// ── 分组标签辅助（i18n 键与 GPUI strings.rs 一致） ──

type Translate = (key: string, params?: Record<string, string | number>) => string

const DATE_LABEL_KEY = {
  today: 'today',
  yesterday: 'yesterday',
  this_week: 'thisWeek',
  this_month: 'thisMonth',
  older: 'older',
} as const

const STATE_LABEL_KEY = {
  pending: 'statusPending',
  downloading: 'statusDownloading',
  paused: 'statusPaused',
  completed: 'statusCompleted',
  failed: 'statusError',
} as const

export const stateLabel = (t: Translate, state: keyof typeof STATE_LABEL_KEY): string => t(STATE_LABEL_KEY[state])

const BUILTIN_LABEL_KEY: Record<string, string> = {
  all: 'tabAll',
  video: 'categoryVideo',
  audio: 'categoryAudio',
  document: 'categoryDocument',
  image: 'categoryImage',
  program: 'categoryProgram',
  archive: 'categoryArchive',
  other: 'categoryOther',
}

/** 分类显示名：内置项走 i18n（categoryVideo…），自定义项用 name。 */
export function categoryLabel(t: Translate, dto: { builtinType: string | null; name: string }): string {
  const key = dto.builtinType ? BUILTIN_LABEL_KEY[dto.builtinType] : undefined
  return key ? t(key) : dto.name
}
