// 移动端任务列表：卡片行（名称 / 进度 / 状态行）。
// 点击 = 有选中时切换选中，否则打开详情；左侧图标区点按 = 切换选中；⋯ / 长按 = 动作菜单（底部 Sheet）。

import { useVirtualizer } from '@tanstack/react-virtual'
import { ChevronDown, ChevronRight, Ellipsis } from 'lucide-react'
import { memo, useCallback, useRef } from 'react'
import { useT } from '../../../i18n'
import { cn } from '../../../lib/cn'
import { ActionMenu, Checkbox, ContextMenuArea, Icon } from '../../../ui'
import type { MenuEntry } from '../../../ui'
import { notePointerActivity } from '../model/rowOrder'
import { formatBytes, percentLabel, sourceSite } from '../model/task'
import type { DownloadTaskView } from '../model/task'
import { useDownloads } from '../state'
import { useConflictTaskIds } from '../dialogs/fileConflict'
import { ConflictBadge, KindGlyph } from './cells'
import { kindLabel, STATUS_TEXT, statusDetail, statusLabel } from './text'
import type { Translate } from './text'
import { usePluginTaskCommands } from '../../plugins/usePluginContributions'
import { buildGroupMenu, buildTaskMenu } from './menus'
import { SegmentProgress } from './SegmentProgress'
import { TaskEmpty } from './TaskEmpty'

interface CardProps {
  t: Translate
  view: DownloadTaskView
  conflict: boolean
  selected: boolean
  anySelected: boolean
  onTap: (view: DownloadTaskView) => void
  onToggle: (view: DownloadTaskView) => void
  getMenu: (view: DownloadTaskView) => MenuEntry[]
  onContext: (view: DownloadTaskView) => void
}

const TaskCard = memo(function TaskCard({ t, view, conflict, selected, anySelected, onTap, onToggle, getMenu, onContext }: CardProps) {
  const detail = statusDetail(t, view)
  const site = sourceSite(view)
  const meta = [kindLabel(t, view.kind), site, view.sizeBytes > 0 ? formatBytes(view.sizeBytes) : '']
    .filter((part) => part !== '')
    .join(' · ')
  return (
    <ContextMenuArea entries={() => getMenu(view)} title={view.name} className="h-full">
      <div
        role="row"
        aria-selected={selected}
        onContextMenu={() => onContext(view)}
        className={cn('relative flex min-h-touch select-none items-stretch active:bg-row-hover', selected && 'bg-accent')}
      >
        <button
          type="button"
          aria-label={view.name}
          onClick={(event) => {
            event.stopPropagation()
            onToggle(view)
          }}
          className="flex w-11 shrink-0 items-center justify-center"
        >
          {anySelected || selected ? (
            <Checkbox checked={selected} aria-label={view.name} className="pointer-events-none" />
          ) : (
            <KindGlyph view={view} />
          )}
        </button>
        <div className="min-w-0 flex-1 cursor-pointer py-2" onClick={() => onTap(view)}>
          <div className={cn('truncate text-sm', view.metadataPending ? 'text-muted-foreground' : 'text-foreground')}>
            {view.metadataPending ? t('statusPreparing') : view.name}
          </div>
          {view.state !== 'completed' ? (
            <div className="mt-1.5 flex items-center gap-2">
              <SegmentProgress
                runtime={view.runtime}
                progress={view.progress}
                state={view.state}
                className="min-w-0 flex-1"
              />
              <span className="tabular w-8 shrink-0 text-right text-xs text-muted-foreground">
                {percentLabel(view.progress)}
              </span>
            </div>
          ) : null}
          <div className="tabular mt-1 flex min-w-0 items-center gap-1.5 text-xs">
            <span className={cn('shrink-0', STATUS_TEXT[view.state])}>{statusLabel(t, view)}</span>
            {conflict ? <ConflictBadge t={t} /> : null}
            {view.state === 'completed' ? (
              <span className="truncate text-text-tertiary">{meta}</span>
            ) : detail ? (
              <span className="truncate text-text-tertiary">{detail}</span>
            ) : null}
          </div>
        </div>
        <ActionMenu
          title={view.name}
          entries={getMenu(view)}
          trigger={
            <button
              type="button"
              aria-label={view.name}
              onClick={() => onContext(view)}
              className="flex w-11 shrink-0 items-center justify-center text-muted-foreground active:text-foreground"
            >
              <Icon icon={Ellipsis} />
            </button>
          }
        />
      </div>
    </ContextMenuArea>
  )
})

export function TaskCards() {
  const t = useT()
  const ctx = useDownloads()
  const conflictTaskIds = useConflictTaskIds()
  const pluginCommands = usePluginTaskCommands()
  const { rows, selected, selectedViews, queues, views, groupSummaries } = ctx
  const scrollRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => (rows[index]?.type === 'group' ? 36 : 84),
    overscan: 8,
    getItemKey: (index) => rows[index]?.key ?? index,
  })

  const onTap = useCallback(
    (view: DownloadTaskView) => {
      if (selected.size > 0 || view.source !== 'local') ctx.toggleSelected(view.key)
      else ctx.showDetail(view.taskId)
    },
    [ctx, selected.size],
  )
  const getMenu = useCallback(
    (view: DownloadTaskView) =>
      buildTaskMenu({
        t,
        views: selected.has(view.key) ? selectedViews : [view],
        queues,
        queueName: ctx.queueName,
        showDetail: ctx.showDetail,
        pluginCommands,
      }),
    [t, selected, selectedViews, queues, ctx.queueName, ctx.showDetail, pluginCommands],
  )

  if (rows.length === 0) return <TaskEmpty />

  return (
    <div
      ref={scrollRef}
      onTouchStart={() => notePointerActivity()}
      onTouchMove={() => notePointerActivity()}
      onWheel={() => notePointerActivity()}
      onContextMenu={() => notePointerActivity()}
      className="h-full min-h-0 overflow-y-auto overflow-x-hidden bg-surface pb-24"
    >
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index]
          if (!row) return null
          const menu =
            row.type === 'group' && row.key.startsWith('group:')
              ? buildGroupMenu(t, row.key.slice(6), groupSummaries.find((group) => group.id === row.key.slice(6)), views)
              : []
          return (
            <div
              key={item.key}
              data-index={item.index}
              ref={virtualizer.measureElement}
              className="absolute left-0 top-0 w-full"
              style={{ transform: `translateY(${item.start}px)` }}
            >
              {row.type === 'group' ? (
                <ContextMenuArea entries={menu} title={row.label} disabled={menu.length === 0}>
                  <button
                    type="button"
                    onClick={() => ctx.toggleGroupCollapsed(row.key)}
                    className="flex min-h-touch w-full items-end gap-1 px-3 pb-1 text-left text-xs"
                  >
                    <Icon icon={row.collapsed ? ChevronRight : ChevronDown} size="sm" className="mb-0.5 text-text-tertiary" />
                    <span className="min-w-0 truncate font-medium text-foreground">{row.label}</span>
                    <span className="tabular shrink-0 text-text-tertiary">{row.count}</span>
                  </button>
                </ContextMenuArea>
              ) : (
                <TaskCard
                  t={t}
                  view={row.view}
                  conflict={row.view.source === 'local' && conflictTaskIds.has(row.view.taskId)}
                  selected={selected.has(row.key)}
                  anySelected={selected.size > 0}
                  onTap={onTap}
                  onToggle={(view) => ctx.toggleSelected(view.key)}
                  getMenu={getMenu}
                  onContext={(view) => ctx.contextSelect(view.key)}
                />
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
