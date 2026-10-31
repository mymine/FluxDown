import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowDown, ArrowUp, Radio } from 'lucide-react'
import { useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { RefObject } from 'react'
import { useT } from '../../../i18n'
import { cn } from '../../../lib/cn'
import { ContextMenuArea, EmptyState, RevealCallout } from '../../../ui'
import type { Translate } from '../table/text'
import { copyEntry, PollError, RowCopyButton, SkeletonRows, useMeasuredRoot, useNowSecs } from './btParts'
import {
  lastOkText,
  mapTrackerRows,
  nextAnnounceText,
  nextTrackerSort,
  showNoneWorking,
  splitMiddle,
  TRACKER_DOT,
  TRACKER_FILTERS,
  TRACKER_TEXT,
  trackerCounts,
  trackerLayout,
  trackerStatusKey,
  trackerView,
} from './btStatus'
import type { TrackerCounts, TrackerFilter, TrackerLayout, TrackerRow, TrackerSort, TrackerSortKey } from './btStatus'
import type { BtTrackerStatus } from '../../../lib/rpc'
import { useBtStatus } from './useBtStatus'

const DASH = '—'

const GRID: Record<Exclude<TrackerLayout, 'card'>, string> = {
  table: 'grid-cols-[minmax(7.5rem,9.5rem)_minmax(0,1fr)_3.25rem_6rem_6.5rem]',
  wide: 'grid-cols-[minmax(7.5rem,9.5rem)_minmax(0,1.4fr)_3.25rem_minmax(0,1fr)_6rem_6.5rem]',
}

function StatusCell({ status, label }: { status: BtTrackerStatus; label: string }) {
  return (
    <span className={cn('flex min-w-0 items-center gap-1.5 text-xs font-medium', TRACKER_TEXT[status])}>
      <span aria-hidden className={cn('size-2 shrink-0 rounded-full', TRACKER_DOT[status])} />
      <span className="truncate">{label}</span>
    </span>
  )
}

/** URL：头部按宽度截断、尾部固定，视觉上是中间省略；文本仍连续可选。 */
function UrlText({ url, title }: { url: string; title: string }) {
  const { head, tail } = splitMiddle(url)
  return (
    <span title={title} className="flex min-w-0 font-mono text-xs text-foreground">
      <span className="min-w-0 truncate whitespace-pre">{head}</span>
      {tail === '' ? null : <span className="shrink-0 whitespace-pre">{tail}</span>}
    </span>
  )
}

function SourceChip({ t, source, hint }: { t: Translate; source: TrackerRow['source']; hint: string }) {
  return (
    <span title={hint} className="shrink-0 cursor-help rounded-sm bg-muted px-1.5 py-px text-caption text-text-tertiary">
      {t(source === 'torrent' ? 'btTrackerSourceTorrent' : 'btTrackerSourceSession')}
    </span>
  )
}

function Row({ row, layout, t, now }: { row: TrackerRow; layout: TrackerLayout; t: Translate; now: number }) {
  const status = t(trackerStatusKey(row.status))
  const peers = row.peers === null ? DASH : String(row.peers)
  const next = nextAnnounceText(t, row.nextAnnounceAt, now) ?? DASH
  const last = lastOkText(t, row.lastOkAt, now)
  const message = row.message
  const copyError = row.detail === '' ? null : row.detail
  const messageLine =
    message === '' ? null : (
      <span className="flex min-w-0 items-center gap-1">
        <span
          title={copyError ?? message}
          className={cn('block min-w-0 truncate text-xs', row.status === 'notWorking' ? 'text-destructive' : 'text-muted-foreground')}
        >
          {message}
        </span>
        {copyError === null ? null : <RowCopyButton label={t('btCopyTrackerError')} value={copyError} doneKey="btTrackerErrorCopied" />}
      </span>
    )
  const menu = [
    copyEntry(t('btCopyTrackerUrl'), row.url, 'btTrackerUrlCopied', t),
    ...(copyError === null ? [] : [{ ...copyEntry(t('btCopyTrackerError'), copyError, 'btTrackerErrorCopied', t), key: 'copyError' }]),
  ]
  // 悬停 URL：完整地址 + 来源说明（表格版式不放来源徽标，说明只在这里）。
  const hint = t(row.source === 'torrent' ? 'btTrackerSourceTorrentHint' : 'btTrackerSourceSessionHint')
  const urlTitle = `${row.url}\n${hint}`

  if (layout === 'card') {
    return (
      <ContextMenuArea entries={menu} title={row.url} className="group/row border-b border-hairline px-1 py-2 hover:bg-row-hover">
        <div className="flex items-center gap-2">
          <StatusCell status={row.status} label={status} />
          <SourceChip t={t} source={row.source} hint={hint} />
          <span className="tabular ml-auto shrink-0 text-xs text-muted-foreground">{peers}</span>
          <RowCopyButton label={t('btCopyTrackerUrl')} value={row.url} doneKey="btTrackerUrlCopied" />
        </div>
        <div className="mt-1 flex">
          <UrlText url={row.url} title={urlTitle} />
        </div>
        {messageLine ? <div className="mt-0.5">{messageLine}</div> : null}
        <div className="tabular mt-1 flex flex-wrap gap-x-3 text-xs text-text-tertiary">
          <span>
            {t('btTrackerNextAnnounce')}: {next}
          </span>
          <span>
            {t('btTrackerLastOk')}: {last}
          </span>
        </div>
      </ContextMenuArea>
    )
  }

  return (
    <ContextMenuArea entries={menu} title={row.url} className="group/row border-b border-hairline hover:bg-row-hover">
      <div className={cn('grid items-center gap-x-3 px-1 py-1.5', GRID[layout])}>
        <StatusCell status={row.status} label={status} />
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-1.5">
            <UrlText url={row.url} title={urlTitle} />
            {layout === 'wide' ? <SourceChip t={t} source={row.source} hint={hint} /> : null}
            <RowCopyButton label={t('btCopyTrackerUrl')} value={row.url} doneKey="btTrackerUrlCopied" />
          </div>
          {layout === 'table' ? messageLine : null}
        </div>
        <span className="tabular text-right text-xs text-muted-foreground">{peers}</span>
        {layout === 'wide' ? <div className="min-w-0">{messageLine}</div> : null}
        <span className="tabular truncate text-xs text-muted-foreground">{next}</span>
        <span className="tabular truncate text-xs text-muted-foreground">{last}</span>
      </div>
    </ContextMenuArea>
  )
}

interface SortProps {
  sort: TrackerSort
  onSort: (key: TrackerSortKey) => void
}

function SortButton({ label, sortKey, sort, onSort, t, className }: SortProps & { label: string; sortKey: TrackerSortKey; t: Translate; className?: string }) {
  const active = sort !== null && sort.key === sortKey
  const Icon = active && sort.desc ? ArrowDown : ArrowUp
  return (
    <button
      type="button"
      title={t('btTrackerSortHint')}
      aria-sort={active ? (sort.desc ? 'descending' : 'ascending') : 'none'}
      onClick={() => onSort(sortKey)}
      className={cn(
        'flex min-w-0 items-center gap-0.5 text-left transition-colors hover:text-foreground',
        active && 'text-foreground',
        className,
      )}
    >
      <span className="truncate">{label}</span>
      <Icon aria-hidden className={cn('size-3 shrink-0', active ? 'opacity-100' : 'opacity-0')} />
    </button>
  )
}

function Header({ layout, t, sort, onSort }: { layout: Exclude<TrackerLayout, 'card'>; t: Translate } & SortProps) {
  const common = { sort, onSort, t }
  return (
    <div className={cn('grid gap-x-3 border-b border-hairline px-1 pb-1 text-xs text-text-tertiary', GRID[layout])}>
      <SortButton {...common} sortKey="status" label={t('btTrackerStatus')} />
      <SortButton {...common} sortKey="url" label={t('btTrackerUrl')} />
      <SortButton {...common} sortKey="peers" label={t('btTrackerPeers')} className="justify-end" />
      {layout === 'wide' ? <span>{t('btTrackerMessage')}</span> : null}
      <SortButton {...common} sortKey="nextAnnounce" label={t('btTrackerNextAnnounce')} />
      <SortButton {...common} sortKey="lastOk" label={t('btTrackerLastOk')} />
    </div>
  )
}

function FilterTabs({
  t,
  counts,
  filter,
  onFilter,
}: {
  t: Translate
  counts: TrackerCounts
  filter: TrackerFilter
  onFilter: (filter: TrackerFilter) => void
}) {
  return (
    <div className="flex flex-wrap gap-1.5">
      {TRACKER_FILTERS.filter((f) => f === 'all' || counts[f] > 0 || f === filter).map((f) => {
        const active = f === filter
        const label = f === 'all' ? t('btTrackerFilterAll') : t(trackerStatusKey(f))
        return (
          <button
            key={f}
            type="button"
            aria-pressed={active}
            onClick={() => onFilter(f)}
            className={cn(
              'tabular rounded-sm px-2 py-0.5 text-xs transition-colors',
              active ? 'bg-muted font-medium text-foreground' : 'text-text-tertiary hover:bg-row-hover hover:text-foreground',
            )}
          >
            {label} {counts[f]}
          </button>
        )
      })}
    </div>
  )
}

/** 最近的可滚动祖先（overflow-y 为 auto / scroll）。 */
function findScrollParent(el: HTMLElement): HTMLElement | null {
  for (let node = el.parentElement; node; node = node.parentElement) {
    const overflowY = getComputedStyle(node).overflowY
    if (overflowY === 'auto' || overflowY === 'scroll') return node
  }
  return null
}

const ROW_OVERSCAN = 8
const ROW_ESTIMATE: Record<TrackerLayout, number> = { card: 96, table: 44, wide: 36 }

/** 虚拟化行列表：滚动容器是祖先面板，`scrollMargin` = 列表顶部相对滚动内容顶部的偏移。 */
function VirtualRows({ rows, layout, t, now, rootRef }: { rows: TrackerRow[]; layout: TrackerLayout; t: Translate; now: number; rootRef: RefObject<HTMLDivElement | null> }) {
  const listRef = useRef<HTMLDivElement | null>(null)
  const [scrollEl, setScrollEl] = useState<HTMLElement | null>(null)
  const [scrollMargin, setScrollMargin] = useState(0)

  useLayoutEffect(() => {
    const list = listRef.current
    if (list === null) return
    const scroller = findScrollParent(list)
    setScrollEl(scroller)
    if (scroller === null) return
    const update = () => {
      const offset = list.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop
      setScrollMargin(Math.round(offset))
    }
    update()
    const observer = new ResizeObserver(update)
    if (rootRef.current) observer.observe(rootRef.current)
    return () => observer.disconnect()
  }, [rootRef])

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollEl,
    estimateSize: () => ROW_ESTIMATE[layout],
    getItemKey: (index) => rows[index]?.key ?? index,
    overscan: ROW_OVERSCAN,
    scrollMargin,
  })

  return (
    <div ref={listRef} className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
      {virtualizer.getVirtualItems().map((item) => {
        const row = rows[item.index]
        if (row === undefined) return null
        return (
          <div
            key={item.key}
            ref={virtualizer.measureElement}
            data-index={item.index}
            className="absolute left-0 top-0 w-full"
            style={{ transform: `translateY(${item.start - scrollMargin}px)` }}
          >
            <Row row={row} layout={layout} t={t} now={now} />
          </div>
        )
      })}
    </div>
  )
}

function DhtRow({ enabled, layout, t }: { enabled: boolean; layout: TrackerLayout; t: Translate }) {
  const status: BtTrackerStatus = enabled ? 'working' : 'notContacted'
  const label = t(enabled ? 'btTrackerDhtOn' : 'btTrackerDhtOff')
  if (layout === 'card') {
    return (
      <div className="flex items-center gap-2 border-b border-hairline px-1 py-2">
        <StatusCell status={status} label={label} />
        <span className="ml-auto font-mono text-xs text-muted-foreground">{t('btTrackerDht')}</span>
      </div>
    )
  }
  return (
    <div className={cn('grid items-center gap-x-3 border-b border-hairline px-1 py-1.5', GRID[layout])}>
      <StatusCell status={status} label={label} />
      <span className="font-mono text-xs text-muted-foreground">{t('btTrackerDht')}</span>
    </div>
  )
}

export function BtTrackersTab({ taskId }: { taskId: string }) {
  const t = useT()
  const { data, error } = useBtStatus(taskId)
  const now = useNowSecs()
  const { ref, width } = useMeasuredRoot()
  const layout = trackerLayout(width)
  const [filter, setFilter] = useState<TrackerFilter>('all')
  const [sort, setSort] = useState<TrackerSort>(null)
  const rows = useMemo(() => (data ? mapTrackerRows(data.trackers) : []), [data])
  const counts = useMemo(() => trackerCounts(rows), [rows])
  const view = useMemo(() => trackerView(rows, filter, sort).flatMap((index) => rows[index] ?? []), [rows, filter, sort])
  const warn = data !== null && showNoneWorking(data.trackers)
  const onSort = (key: TrackerSortKey) => setSort((current) => nextTrackerSort(current, key))

  return (
    <div ref={ref} className="flex flex-col gap-2">
      <RevealCallout visible={warn} tone="warning" title={t('btTabTrackers')} body={t('btTrackersNoneWorking')} contentClassName="" />
      <PollError message={error} />
      {data === null ? (
        error === null ? <SkeletonRows /> : null
      ) : (
        <>
          {rows.length === 0 ? null : <FilterTabs t={t} counts={counts} filter={filter} onFilter={setFilter} />}
          <div className="flex flex-col">
            {layout === 'card' ? null : <Header layout={layout} t={t} sort={sort} onSort={onSort} />}
            <DhtRow enabled={data.dhtEnabled} layout={layout} t={t} />
            {rows.length === 0 ? (
              <EmptyState icon={Radio} title={t('btTrackersEmpty')} className="py-6" />
            ) : view.length === 0 ? (
              <EmptyState icon={Radio} title={t('btTrackersFilterEmpty')} className="py-6" />
            ) : (
              <VirtualRows key={layout} rows={view} layout={layout} t={t} now={now} rootRef={ref} />
            )}
          </div>
        </>
      )}
    </div>
  )
}
