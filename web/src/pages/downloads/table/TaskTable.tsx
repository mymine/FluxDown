// 桌面任务表（移植 crates/downloads/src/components/task_table.rs）：
// 列 / 宽度 / 最小宽度 / 排序与 GPUI 一致，分组头，悬停行操作，复选框选择，右键菜单，虚拟化。

import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, Download, Pause, Play, RotateCw } from 'lucide-react'
import { memo, useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { MouseEvent } from 'react'
import { useT } from '../../../i18n'
import { cn } from '../../../lib/cn'
import { Checkbox, ContextMenuArea, Icon, Tooltip } from '../../../ui'
import type { MenuEntry } from '../../../ui'
import { downloadViewsFiles, isDownloadable, pauseViews, resumeViews } from '../model/actions'
import { remoteCan } from '../model/batchPlan'
import { notePointerActivity } from '../model/rowOrder'
import { formatBytes, formatDateTime, MAX_ETA_SECS, PROTOCOL_LABEL, sourceSite } from '../model/task'
import type { DownloadTaskView } from '../model/task'
import {
  columnShown,
  COLUMN_LABEL_KEY,
  COLUMN_SORT_KEY,
  FILE_NAME_MAX_WIDTH,
  MAX_COLUMN_WIDTH,
  MIN_WIDTH,
  nextHeaderSort,
  NUMERIC_COLUMNS,
  resolveColumns,
  toColumnPrefs,
} from '../model/viewPrefs'
import type { ColumnKind, ResolvedColumn, ViewDensity } from '../model/viewPrefs'
import { useDownloads } from '../state'
import type { VisibleRow } from '../state'
import { useConflictTaskIds } from '../dialogs/fileConflict'
import { FileCell, KindGlyph, ProgressCell, StatusCell } from './cells'
import { formatEta } from './text'
import type { Translate } from './text'
import { buildGroupMenu, buildTaskMenu } from './menus'
import { SelectionHeaderBar } from '../selection'
import { TaskEmpty } from './TaskEmpty'

/** 固定左侧选择列宽（含左侧留白）。 */
const SELECTION_COLUMN_WIDTH = 36
const TRAILING_GUTTER = 8
const HEADER_HEIGHT = 28
const CELL_PADDING_X = 8
const PROGRESS_LABEL_WIDTH = 32
const PROGRESS_GAP = 8
/** 分组头槽位：28 高内容 + 上方 8px 留白。 */
const GROUP_SLOT_HEIGHT = HEADER_HEIGHT + 8

interface LayoutColumn extends ResolvedColumn {
  /** 文件名列吸收剩余宽度（未固定）。 */
  fluid: boolean
}

function cellStyle(column: LayoutColumn) {
  return column.fluid
    ? { flex: '1 1 0%', minWidth: MIN_WIDTH.file_name }
    : { flex: '0 0 auto', width: column.width }
}

function Meta({ children, numeric, tertiary }: { children: string; numeric: boolean; tertiary?: boolean }) {
  return (
    <div
      className={cn(
        'min-w-0 truncate text-xs',
        numeric && 'tabular text-right',
        tertiary ? 'text-text-tertiary' : 'text-muted-foreground',
      )}
    >
      {children}
    </div>
  )
}

interface RowProps {
  t: Translate
  view: DownloadTaskView
  columns: readonly LayoutColumn[]
  density: ViewDensity
  conflict: boolean
  selected: boolean
  anySelected: boolean
  queueName: (queueId: string) => string
  onClick: (event: MouseEvent, view: DownloadTaskView) => void
  onDoubleClick: (view: DownloadTaskView) => void
  onToggle: (view: DownloadTaskView) => void
  getMenu: (view: DownloadTaskView) => MenuEntry[]
  onContext: (view: DownloadTaskView) => void
  rowActionLabels: { pause: string; resume: string; download: string; select: string }
}

function renderCell(props: RowProps, column: LayoutColumn) {
  const { t, view, density, queueName } = props
  const downloading = view.state === 'downloading'
  switch (column.kind) {
    case 'file_name':
      return <FileCell t={t} view={view} density={density} conflict={props.conflict} />
    case 'progress': {
      const barWidth = Math.max(0, column.width - 2 * CELL_PADDING_X - PROGRESS_LABEL_WIDTH - PROGRESS_GAP)
      return <ProgressCell view={view} barWidth={barWidth} />
    }
    case 'size':
      return view.sizeBytes > 0 ? (
        <Meta numeric>{formatBytes(view.sizeBytes)}</Meta>
      ) : (
        <Meta numeric tertiary>
          —
        </Meta>
      )
    case 'status':
      return <StatusCell t={t} view={view} density={density} />
    case 'speed':
      return downloading && view.speed !== null && view.speed > 0 ? (
        <Meta numeric>{`${formatBytes(view.speed)}/s`}</Meta>
      ) : (
        <Meta numeric tertiary>
          —
        </Meta>
      )
    case 'eta':
      return downloading && view.etaSeconds !== null && view.etaSeconds <= MAX_ETA_SECS ? (
        <Meta numeric>{formatEta(t, view.etaSeconds)}</Meta>
      ) : (
        <Meta numeric tertiary>
          —
        </Meta>
      )
    case 'created':
      return view.createdAtSecs > 0 ? (
        <Meta numeric>{formatDateTime(view.createdAtSecs)}</Meta>
      ) : (
        <Meta numeric tertiary>
          —
        </Meta>
      )
    case 'protocol':
      return <Meta numeric={false}>{PROTOCOL_LABEL[view.protocol]}</Meta>
    case 'source':
      return <Meta numeric={false}>{sourceSite(view)}</Meta>
    case 'queue':
      return <Meta numeric={false}>{view.source === 'local' ? queueName(view.queueId) : ''}</Meta>
  }
}

/** 行悬停操作：下载中 / 排队 → 暂停；暂停 → 继续；失败 → 重试；完成 → 下载文件（仅本地）。 */
function RowActions({ view, labels, selected }: { view: DownloadTaskView; labels: RowProps['rowActionLabels']; selected: boolean }) {
  const buttons: { key: string; label: string; icon: typeof Pause; run: () => void }[] = []
  switch (view.state) {
    case 'downloading':
    case 'pending':
      if (remoteCan(view, 'pause')) buttons.push({ key: 'pause', label: labels.pause, icon: Pause, run: () => void pauseViews([view]) })
      break
    case 'paused':
      if (remoteCan(view, 'resume')) buttons.push({ key: 'resume', label: labels.resume, icon: Play, run: () => void resumeViews([view]) })
      break
    case 'failed':
      if (remoteCan(view, 'resume')) buttons.push({ key: 'retry', label: labels.resume, icon: RotateCw, run: () => void resumeViews([view]) })
      break
    case 'completed':
      if (isDownloadable(view)) {
        buttons.push({ key: 'download', label: labels.download, icon: Download, run: () => downloadViewsFiles([view]) })
      }
      break
  }
  if (buttons.length === 0) return null
  return (
    <div
      className={cn(
        'absolute inset-y-0 right-1 rounded-r-[var(--fx-components-task-row-radius)] hidden items-center gap-0.5 pl-1 pr-1 group-hover/row:flex',
        selected ? 'bg-nav-selected' : 'bg-row-hover',
      )}
    >
      {buttons.map((button) => (
        <Tooltip key={button.key} content={button.label}>
          <button
            type="button"
            aria-label={button.label}
            onClick={(event) => {
              event.stopPropagation()
              button.run()
            }}
            onDoubleClick={(event) => event.stopPropagation()}
            className="flex size-6 items-center justify-center rounded-sm text-muted-foreground hover:bg-nav-selected hover:text-foreground"
          >
            <Icon icon={button.icon} size="md" />
          </button>
        </Tooltip>
      ))}
    </div>
  )
}

const TaskRow = memo(function TaskRow(props: RowProps) {
  const { view, columns, selected, anySelected, onClick, onDoubleClick, onToggle, getMenu, onContext, density } = props
  const twoLine = density !== 'compact'
  return (
    <ContextMenuArea entries={() => getMenu(view)} title={view.name}>
      <div
        role="row"
        aria-selected={selected}
        onClick={(event) => onClick(event, view)}
        onDoubleClick={() => onDoubleClick(view)}
        onContextMenu={() => onContext(view)}
        className={cn(
          'group/row relative flex cursor-default select-none items-center',
          density === 'relaxed' ? 'h-task-row-relaxed' : twoLine ? 'h-task-row' : 'h-task-row-compact',
        )}
      >
        <div className={cn('pointer-events-none absolute inset-y-0 left-1 right-1 rounded-[var(--fx-components-task-row-radius)]', selected ? 'bg-accent' : 'group-hover/row:bg-row-hover')} />
        <div className="relative flex shrink-0 items-center justify-center self-stretch" style={{ width: SELECTION_COLUMN_WIDTH }}>
          {anySelected ? (
            <div className="flex p-1" onClick={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()}>
              <Checkbox checked={selected} onCheckedChange={() => onToggle(view)} aria-label={view.name} />
            </div>
          ) : (
            // 与 GPUI `render_selection_cell` 对齐：只在悬停该格本身时把图标换成复选框，点击以该行进入多选。
            <Tooltip content={props.rowActionLabels.select}>
              <div
                role="button"
                aria-label={props.rowActionLabels.select}
                className="group/sel flex size-full cursor-pointer items-center justify-center"
                onClick={(event) => {
                  event.stopPropagation()
                  onToggle(view)
                }}
                onDoubleClick={(event) => event.stopPropagation()}
              >
                <span className="flex group-hover/sel:hidden">
                  <KindGlyph view={view} tile={density === 'relaxed'} />
                </span>
                <span aria-hidden className="hidden p-1 group-hover/sel:flex">
                  <Checkbox checked={false} className="pointer-events-none" />
                </span>
              </div>
            </Tooltip>
          )}
        </div>
        {columns.map((column) => (
          <div
            key={column.kind}
            className={cn('relative min-w-0 shrink-0', twoLine ? 'py-1' : '')}
            style={{ ...cellStyle(column), paddingInline: CELL_PADDING_X }}
          >
            {renderCell(props, column)}
          </div>
        ))}
        <div style={{ width: TRAILING_GUTTER }} className="shrink-0" />
        <RowActions view={view} labels={props.rowActionLabels} selected={selected} />
      </div>
    </ContextMenuArea>
  )
})

function GroupHeaderRow({
  row,
  progress,
  onToggle,
  entries,
  title,
}: {
  row: Extract<VisibleRow, { type: 'group' }>
  progress: string | null
  onToggle: () => void
  entries: readonly MenuEntry[]
  title: string
}) {
  const header = (
    <div
      role="row"
      onClick={onToggle}
      className="flex h-full cursor-pointer select-none items-end"
      style={{ paddingLeft: 8 }}
    >
      <div className="flex items-center gap-1 text-xs" style={{ height: HEADER_HEIGHT }}>
        <Icon icon={row.collapsed ? ChevronRight : ChevronDown} size="sm" className="text-text-tertiary" />
        <span className="min-w-0 truncate font-medium text-foreground">{row.label}</span>
        <span className="tabular shrink-0 text-text-tertiary">{row.count}</span>
        {progress ? <span className="tabular shrink-0 text-text-tertiary">{progress}</span> : null}
      </div>
    </div>
  )
  return entries.length > 0 ? (
    <ContextMenuArea entries={entries} title={title} className="h-full">
      {header}
    </ContextMenuArea>
  ) : (
    header
  )
}

export function TaskTable() {
  const t = useT()
  const ctx = useDownloads()
  const conflictTaskIds = useConflictTaskIds()
  const { rows, prefs, selected, visibleKeys, views, groupSummaries } = ctx
  const scrollRef = useRef<HTMLDivElement>(null)
  // 拖拽调宽中的临时宽度（松手后写回偏好）。
  const [resizing, setResizing] = useState<{ kind: ColumnKind; width: number } | null>(null)
  const [dragKind, setDragKind] = useState<ColumnKind | null>(null)

  const resolved = useMemo(() => resolveColumns(prefs.columns), [prefs.columns])
  const columns = useMemo<LayoutColumn[]>(() => {
    const shown = resolved.filter((column) => columnShown(column.kind, column.visible, prefs.density))
    return shown.map((column) => {
      let width = column.width
      let fluid = false
      if (column.kind === 'file_name') {
        if (prefs.file_name_width !== null) {
          width = Math.min(FILE_NAME_MAX_WIDTH, Math.max(MIN_WIDTH.file_name, prefs.file_name_width))
        } else {
          fluid = true
        }
      }
      if (resizing && resizing.kind === column.kind) {
        width = resizing.width
        fluid = false
      }
      return { ...column, width, fluid }
    })
  }, [resolved, prefs.file_name_width, prefs.density, resizing])
  const totalMinWidth = useMemo(
    () =>
      SELECTION_COLUMN_WIDTH +
      TRAILING_GUTTER +
      columns.reduce((sum, column) => sum + (column.fluid ? MIN_WIDTH.file_name : column.width), 0),
    [columns],
  )

  const rowActionLabels = useMemo(
    () => ({ pause: t('pause'), resume: t('resume'), download: t('webDownloadFile'), select: t('selectTaskTooltip') }),
    [t],
  )

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => (rows[index]?.type === 'group' ? GROUP_SLOT_HEIGHT : prefs.density === 'relaxed' ? 66 : prefs.density === 'comfortable' ? 44 : 30),
    overscan: 10,
    scrollMargin: HEADER_HEIGHT,
    getItemKey: (index) => rows[index]?.key ?? index,
  })

  // 密度切换会改变所有任务行高，清掉屏外行的旧测量，避免滚动时保留旧间距。
  useLayoutEffect(() => {
    virtualizer.measure()
  }, [prefs.density, virtualizer])

  // 回调经 ref 读取最新上下文，自身身份恒定，TaskRow 的 memo 才能在进度帧中命中。
  const ctxRef = useRef(ctx)
  ctxRef.current = ctx
  const tRef = useRef(t)
  tRef.current = t
  const onClick = useCallback((event: MouseEvent, view: DownloadTaskView) => {
    ctxRef.current.clickSelect(view.key, { shift: event.shiftKey, secondary: event.ctrlKey || event.metaKey })
  }, [])
  const onDoubleClick = useCallback((view: DownloadTaskView) => {
    if (isDownloadable(view)) downloadViewsFiles([view])
    else if (view.source === 'local') ctxRef.current.showDetail(view.taskId)
  }, [])
  const onToggle = useCallback((view: DownloadTaskView) => ctxRef.current.toggleMultiSelect(view.key), [])
  const onContext = useCallback((view: DownloadTaskView) => ctxRef.current.contextSelect(view.key), [])
  // 菜单在打开时才构建，此时读到的选区即右键选择后的最新选区。
  const getMenu = useCallback((view: DownloadTaskView) => {
    const current = ctxRef.current
    return buildTaskMenu({
      t: tRef.current,
      views: current.selected.has(view.key) ? current.selectedViews : [view],
      queues: current.queues,
      queueName: current.queueName,
      showDetail: current.showDetail,
      selection: { multiSelect: current.multiSelect, enterMultiSelect: current.enterMultiSelect, selectAll: current.selectAll },
    })
  }, [])

  const allChecked = visibleKeys.length > 0 && visibleKeys.every((key) => selected.has(key))
  const someChecked = !allChecked && visibleKeys.some((key) => selected.has(key))

  const toggleSort = (kind: ColumnKind) => {
    const key = COLUMN_SORT_KEY[kind]
    if (!key) return
    ctx.updatePrefs((current) => nextHeaderSort(current, key))
  }

  const startResize = (event: React.PointerEvent, column: LayoutColumn) => {
    event.preventDefault()
    event.stopPropagation()
    const startX = event.clientX
    const startWidth = event.currentTarget.parentElement?.getBoundingClientRect().width ?? column.width
    const max = column.kind === 'file_name' ? FILE_NAME_MAX_WIDTH : MAX_COLUMN_WIDTH
    const clamp = (width: number) => Math.round(Math.min(max, Math.max(MIN_WIDTH[column.kind], width)))
    let latest = clamp(startWidth)
    const move = (moveEvent: PointerEvent) => {
      latest = clamp(startWidth + moveEvent.clientX - startX)
      setResizing({ kind: column.kind, width: latest })
    }
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      setResizing(null)
      ctx.updatePrefs((current) => {
        if (column.kind === 'file_name') return { ...current, file_name_width: latest }
        const next = resolveColumns(current.columns).map((item) =>
          item.kind === column.kind ? { ...item, width: latest } : item,
        )
        return { ...current, columns: toColumnPrefs(next) }
      })
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  const moveColumn = (from: ColumnKind, to: ColumnKind) => {
    if (from === to) return
    ctx.updatePrefs((current) => {
      const list = resolveColumns(current.columns)
      const fromIndex = list.findIndex((column) => column.kind === from)
      const toIndex = list.findIndex((column) => column.kind === to)
      if (fromIndex < 0 || toIndex < 0) return current
      const [moved] = list.splice(fromIndex, 1)
      if (moved) list.splice(toIndex, 0, moved)
      return { ...current, columns: toColumnPrefs(list) }
    })
  }

  if (rows.length === 0) return <TaskEmpty />

  return (
    <div
      ref={scrollRef}
      onMouseMove={() => notePointerActivity()}
      onMouseDown={() => notePointerActivity()}
      onWheel={() => notePointerActivity()}
      onContextMenu={() => notePointerActivity()}
      className="relative h-full min-h-0 overflow-auto bg-surface"
    >
      <div style={{ minWidth: totalMinWidth }}>
        <div
          role="row"
          className="sticky top-0 z-10 flex items-center bg-surface"
          style={{ height: HEADER_HEIGHT }}
        >
          <div
            className="group/all flex shrink-0 items-center justify-center"
            style={{ width: SELECTION_COLUMN_WIDTH }}
          >
            {/* 非多选模式（普通单击选中）表头全选框按未选处理；表头选择条只在显式多选模式出现，单项操作走右键菜单。 */}
            <div className={cn(ctx.multiSelect ? 'flex' : 'invisible group-hover/all:visible')}>
              <Checkbox
                checked={!ctx.multiSelect ? false : allChecked ? true : someChecked ? 'indeterminate' : false}
                onCheckedChange={() => (ctx.multiSelect && allChecked ? ctx.clearSelection() : ctx.selectAll())}
                aria-label={t('selectedCount', { n: visibleKeys.length })}
              />
            </div>
          </div>
          {ctx.multiSelect ? (
            <SelectionHeaderBar />
          ) : (
            <>
              {columns.map((column) => {
                const sortKey = COLUMN_SORT_KEY[column.kind]
                const active = sortKey !== undefined && sortKey === prefs.sort_key
                const numeric = NUMERIC_COLUMNS.has(column.kind)
                const arrow = active ? (
                  <Icon icon={prefs.sort_dir === 'asc' ? ArrowUp : ArrowDown} size="sm" />
                ) : null
                return (
                  <div
                    key={column.kind}
                    draggable
                    onDragStart={(event) => {
                      setDragKind(column.kind)
                      event.dataTransfer.effectAllowed = 'move'
                    }}
                    onDragOver={(event) => dragKind && event.preventDefault()}
                    onDrop={() => {
                      if (dragKind) moveColumn(dragKind, column.kind)
                      setDragKind(null)
                    }}
                    onDragEnd={() => setDragKind(null)}
                    onClick={() => toggleSort(column.kind)}
                    className={cn(
                      'group/th relative flex min-w-0 shrink-0 items-center gap-0.5 text-xs font-medium text-text-tertiary',
                      numeric && 'justify-end',
                      sortKey && 'cursor-pointer',
                    )}
                    style={{ ...cellStyle(column), paddingInline: CELL_PADDING_X }}
                  >
                    {numeric ? arrow : null}
                    <span className="min-w-0 truncate">{t(COLUMN_LABEL_KEY[column.kind])}</span>
                    {numeric ? null : arrow}
                    <div
                      role="separator"
                      onPointerDown={(event) => startResize(event, column)}
                      onClick={(event) => event.stopPropagation()}
                      className="absolute inset-y-0 right-0 flex w-2 cursor-col-resize items-center justify-center"
                    >
                      <div className="h-3.5 w-px bg-hairline group-hover/th:h-full group-hover/th:bg-border" />
                    </div>
                  </div>
                )
              })}
              <div style={{ width: TRAILING_GUTTER }} className="shrink-0" />
            </>
          )}
        </div>
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index]
            if (!row) return null
            return (
              <div
                key={item.key}
                data-index={item.index}
                ref={virtualizer.measureElement}
                className="absolute left-0 top-0 w-full"
                style={{
                  transform: `translateY(${item.start - HEADER_HEIGHT}px)`,
                  height: row.type === 'group' ? GROUP_SLOT_HEIGHT : undefined,
                }}
              >
                {row.type === 'group' ? (
                  <GroupHeaderRow
                    row={row}
                    progress={groupProgress(row.key, views)}
                    onToggle={() => ctx.toggleGroupCollapsed(row.key)}
                    entries={
                      row.key.startsWith('group:')
                        ? buildGroupMenu(t, row.key.slice(6), groupSummaries.find((group) => group.id === row.key.slice(6)), views)
                        : []
                    }
                    title={row.label}
                  />
                ) : (
                  <TaskRow
                    t={t}
                    view={row.view}
                    columns={columns}
                    density={prefs.density}
                    conflict={row.view.source === 'local' && conflictTaskIds.has(row.view.taskId)}
                    selected={selected.has(row.key)}
                    anySelected={ctx.multiSelect}
                    queueName={ctx.queueName}
                    onClick={onClick}
                    onDoubleClick={onDoubleClick}
                    onToggle={onToggle}
                    getMenu={getMenu}
                    onContext={onContext}
                    rowActionLabels={rowActionLabels}
                  />
                )}
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}

/** 任务组头的 `完成/总数`。 */
function groupProgress(groupKey: string, views: readonly DownloadTaskView[]): string | null {
  if (!groupKey.startsWith('group:') || groupKey === 'group:') return null
  const groupId = groupKey.slice(6)
  let completed = 0
  let total = 0
  for (const view of views) {
    if (view.source !== 'local' || view.groupId !== groupId) continue
    total += 1
    if (view.state === 'completed') completed += 1
  }
  return `${completed}/${total}`
}
