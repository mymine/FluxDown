// RSS 页（GPUI `crates/rss`）：订阅源列（桌面 240px 可拖拽）| 条目区。
// 移动端（<=820px）为「订阅列表 → 条目详情」两级导航，详情里标题栏带返回键。
// 该页在 GPUI 里没有标题栏插槽，因此桌面标题栏保持空白。

import { ChevronLeft, Plus, Rss } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent, PointerEvent, ReactNode } from 'react'
import { useT } from '../../i18n'
import { useAgent, useConnection, useDaemon } from '../../lib/rpc'
import type { PluginDto, QueueDto, RssSourceDto } from '../../lib/rpc'
import { TitleBarSlot } from '../../shell'
import { Button, EmptyState, Icon, useIsMobile } from '../../ui'
import { sourceDisplayName } from './filter'
import { ItemPane } from './ItemPane'
import { SourceColumn } from './SourceColumn'
import { SourceEditor } from './SourceEditor'
import { useNowSeconds } from './useNow'

const EMPTY_SOURCES: readonly RssSourceDto[] = []
const EMPTY_QUEUES: readonly QueueDto[] = []
const EMPTY_PLUGINS: readonly PluginDto[] = []

const COLUMN_DEFAULT = 240
const COLUMN_MIN = 176
const COLUMN_MAX = 400
const COLUMN_STORAGE_KEY = 'fluxdown.web.rss.sourceColumnWidth'

const clampWidth = (width: number) => Math.min(COLUMN_MAX, Math.max(COLUMN_MIN, Math.round(width)))

function readStoredWidth(): number {
  try {
    const stored = Number(localStorage.getItem(COLUMN_STORAGE_KEY))
    return Number.isFinite(stored) && stored > 0 ? clampWidth(stored) : COLUMN_DEFAULT
  } catch {
    return COLUMN_DEFAULT
  }
}

/** 订阅源列宽：拖拽 / 键盘调节，范围 176–400，记忆到 localStorage。 */
function useColumnWidth() {
  const [width, setWidth] = useState(readStoredWidth)
  const drag = useRef<{ startX: number; startWidth: number } | null>(null)
  const persist = (value: number) => {
    try {
      localStorage.setItem(COLUMN_STORAGE_KEY, String(value))
    } catch {
      // 隐私模式等场景：宽度只在本次会话有效。
    }
  }
  const handleProps = {
    onPointerDown: (event: PointerEvent<HTMLDivElement>) => {
      drag.current = { startX: event.clientX, startWidth: width }
      event.currentTarget.setPointerCapture(event.pointerId)
    },
    onPointerMove: (event: PointerEvent<HTMLDivElement>) => {
      const state = drag.current
      if (state) setWidth(clampWidth(state.startWidth + event.clientX - state.startX))
    },
    onPointerUp: () => {
      if (drag.current) persist(width)
      drag.current = null
    },
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
      event.preventDefault()
      const next = clampWidth(width + (event.key === 'ArrowRight' ? 16 : -16))
      setWidth(next)
      persist(next)
    },
  }
  return { width, handleProps }
}

export function RssPage() {
  const t = useT()
  const mobile = useIsMobile()
  const rawSources = useDaemon((daemon) => daemon.rssSources, EMPTY_SOURCES)
  const queues = useDaemon((daemon) => daemon.queues, EMPTY_QUEUES)
  const plugins = useDaemon((daemon) => daemon.plugins, EMPTY_PLUGINS)
  const daemonConnected = useAgent((snapshot) => snapshot.daemonConnected, false)
  const { phase } = useConnection()
  const stale = !daemonConnected || phase !== 'ready'
  const now = useNowSeconds()
  const { width, handleProps } = useColumnWidth()

  const sources = useMemo(() => [...rawSources].sort((a, b) => a.position - b.position), [rawSources])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detailOpen, setDetailOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [oldestFirst, setOldestFirst] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [editor, setEditor] = useState<{ key: number; source: RssSourceDto | null } | null>(null)

  const selected = sources.find((source) => source.sourceId === selectedId)
  // 所选订阅被删除 / 尚未选择：桌面回落到第一个；移动端回到列表。
  const active = selected ?? sources[0]
  const showDetail = mobile ? detailOpen && selected !== undefined : true

  useEffect(() => {
    if (mobile && detailOpen && selected === undefined) setDetailOpen(false)
  }, [mobile, detailOpen, selected])

  const openEditor = (source: RssSourceDto | null) => setEditor((prev) => ({ key: (prev?.key ?? 0) + 1, source }))
  const select = (sourceId: string) => {
    setSelectedId(sourceId)
    setDetailOpen(true)
  }

  const column = (
    <SourceColumn
      sources={sources}
      selectedId={active?.sourceId ?? null}
      refreshingId={refreshing && active ? active.sourceId : null}
      now={now}
      disabled={stale}
      onSelect={select}
      onAdd={() => openEditor(null)}
    />
  )
  const onRefreshingChange = useCallback((value: boolean) => setRefreshing(value), [])

  let pane: ReactNode
  if (active) {
    pane = (
      <ItemPane
        key={active.sourceId}
        source={active}
        stale={stale}
        now={now}
        mobile={mobile}
        query={query}
        onQueryChange={setQuery}
        oldestFirst={oldestFirst}
        onToggleSort={() => setOldestFirst((value) => !value)}
        onEdit={openEditor}
        onRefreshingChange={onRefreshingChange}
      />
    )
  } else {
    pane = (
      <EmptyState
        className="h-full"
        icon={Rss}
        title={t('rssAddSource')}
        description={t('rssSidebarEmptyHint')}
        action={
          <Button variant="primary" icon={Plus} disabled={stale} onClick={() => openEditor(null)}>
            {t('rssAddSource')}
          </Button>
        }
      />
    )
  }

  return (
    <div className="flex h-full min-h-0 w-full">
      {mobile && showDetail && selected ? (
        <TitleBarSlot>
          <Button variant="ghost" iconOnly aria-label={t('back')} onClick={() => setDetailOpen(false)}>
            <Icon icon={ChevronLeft} />
          </Button>
          <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">{sourceDisplayName(selected)}</span>
        </TitleBarSlot>
      ) : null}
      {mobile ? (
        <div className="h-full min-h-0 min-w-0 flex-1">{showDetail ? pane : column}</div>
      ) : (
        <>
          <aside className="h-full shrink-0 bg-chrome" style={{ width }}>
            {column}
          </aside>
          <div
            role="separator"
            aria-orientation="vertical"
            aria-valuemin={COLUMN_MIN}
            aria-valuemax={COLUMN_MAX}
            aria-valuenow={width}
            tabIndex={0}
            className="group relative z-10 -mx-0.5 w-1 shrink-0 cursor-col-resize touch-none outline-none"
            {...handleProps}
          >
            <div className="mx-auto h-full w-px bg-hairline transition-colors group-hover:bg-primary/60 group-focus-visible:bg-primary group-active:bg-primary" />
          </div>
          <div className="h-full min-h-0 min-w-0 flex-1">{pane}</div>
        </>
      )}
      {editor ? (
        <SourceEditor
          key={editor.key}
          open
          source={editor.source}
          queues={queues}
          plugins={plugins}
          onOpenChange={(open) => {
            if (!open) setEditor(null)
          }}
        />
      ) : null}
    </div>
  )
}
