// 下载页侧栏（移植 crates/downloads/src/components/sidebar.rs）：状态 / 队列 / 设备三个分区。

import { useNavigate } from '@tanstack/react-router'
import {
  ChevronRight,
  CircleAlert,
  CircleArrowDown,
  CircleCheck,
  CirclePause,
  Cpu,
  Globe,
  Plus,
  Layers,
  Rows3,
  Settings,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../../../i18n'
import { categoryIconByKey } from '../../../lib/category-icons'
import { cn } from '../../../lib/cn'
import { LATER_QUEUE_ID, MAIN_QUEUE_ID, rpc, useAgent, useConnection, usePref, usePrefBool } from '../../../lib/rpc'
import { cloudPresenceKnown } from '../../../lib/cloud-presence'
import type { CustomCategoryDto, QueueDto } from '../../../lib/rpc'
import { ContextMenuArea, Icon, confirmDialog } from '../../../ui'
import { toastRpcError } from '../../../lib/rpcToast'
import type { MenuEntry } from '../../../ui'
import { openQueueManager } from '../dialogs'
import { otherDevices } from '../model/devices'
import { LOCAL_DEVICE, SIDEBAR_SECTION_PREFS, STATUS_FILTERS, filterMatches, sameSelection } from '../model/filters'
import type { DownloadFilter, DownloadStatusFilter, SidebarSelection } from '../model/filters'
import type { DownloadTaskView } from '../model/task'
import { categoryLabel, useDownloads } from '../state'

type Section = 'status' | 'queues' | 'devices'

const STATUS_ICON: Record<DownloadStatusFilter, LucideIcon> = {
  all: Layers,
  incomplete: CircleArrowDown,
  completed: CircleCheck,
  failed: CircleAlert,
  paused: CirclePause,
}

const STATUS_LABEL_KEY: Record<DownloadStatusFilter, string> = {
  all: 'tabAll',
  incomplete: 'statusIncomplete',
  completed: 'statusCompleted',
  failed: 'tabError',
  paused: 'statusPaused',
}

const SECTION_LABEL_KEY: Record<Section, string> = {
  status: 'sidebarStatus',
  queues: 'sidebarQueues',
  devices: 'deviceSection',
}

const ROW =
  'flex h-nav-row coarse:min-h-touch w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-sm outline-none focus-visible:ring-1 focus-visible:ring-primary'

function hideSection(key: string) {
  rpc.agent.preferences.patch({ values: { [key]: false } }).catch(toastRpcError)
}

/** 仅在计数相关字段（与进度无关）变化时才换新数组引用；进度帧沿用旧引用。 */
function useStableViews(views: readonly DownloadTaskView[]): readonly DownloadTaskView[] {
  const signature = views
    .map((view) => `${view.key}|${view.state}|${view.queueId}|${view.toDevice}|${view.name}|${view.url}|${view.saveDir}`)
    .join('\n')
  const [stable, setStable] = useState(views)
  const [seen, setSeen] = useState(signature)
  if (seen !== signature) {
    setSeen(signature)
    setStable(views)
  }
  return stable
}

/** 可折叠容器（CSS grid 行高过渡，对应 GPUI 折叠动画）。 */
function Collapse({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div
      className={cn('grid transition-[grid-template-rows] duration-200 ease-in-out', open ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]')}
      inert={!open}
    >
      <div className="min-h-0 overflow-hidden">{children}</div>
    </div>
  )
}

function Trailing({ count, selected, dot }: { count: number; selected: boolean; dot?: boolean }) {
  return (
    <span className="flex shrink-0 items-center gap-1">
      {dot ? <span className="size-1.5 rounded-full bg-success" /> : null}
      {count > 0 ? (
        <span className={cn('text-caption tabular', selected ? 'text-muted-foreground' : 'text-text-tertiary')}>{count}</span>
      ) : null}
    </span>
  )
}

function NavRow({
  selected,
  label,
  icon,
  iconClass,
  count,
  dot,
  indent,
  onClick,
}: {
  selected: boolean
  label: string
  icon: LucideIcon
  iconClass?: string
  count: number
  dot?: boolean
  indent?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={selected ? 'true' : undefined}
      className={cn(ROW, indent && 'pl-6', selected ? 'bg-nav-selected text-foreground' : 'text-muted-foreground hover:bg-nav-hover')}
    >
      <Icon
        icon={icon}
        size={indent ? 'md' : 'lg'}
        className={iconClass ?? (selected ? 'text-nav-selected-icon' : 'text-muted-foreground')}
      />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <Trailing count={count} selected={selected} dot={dot} />
    </button>
  )
}

function SectionHeader({
  section,
  open,
  onToggle,
  trailing,
}: {
  section: Section
  open: boolean
  onToggle: () => void
  trailing?: ReactNode
}) {
  const t = useT()
  const prefKey = SIDEBAR_SECTION_PREFS[section]
  const entries: MenuEntry[] = [{ type: 'item', key: 'hide', label: t('hideSection'), onSelect: () => hideSection(prefKey) }]
  const label = t(SECTION_LABEL_KEY[section])
  return (
    <ContextMenuArea entries={entries} title={label}>
      <div className="group/header flex h-section-header coarse:min-h-touch items-center justify-between gap-1 rounded-md pr-1 text-caption font-medium text-text-tertiary hover:text-muted-foreground">
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={open}
          className="flex h-full min-w-0 flex-1 items-center px-2 text-left outline-none focus-visible:ring-1 focus-visible:ring-primary"
        >
          <span className="truncate">{label}</span>
        </button>
        <span className="flex shrink-0 items-center gap-0.5 mobile:visible desktop:invisible desktop:group-hover/header:visible desktop:group-focus-within/header:visible">
          {trailing}
          <button
            type="button"
            tabIndex={-1}
            aria-hidden
            onClick={onToggle}
            className="flex size-5 coarse:size-touch items-center justify-center"
          >
            <Icon icon={ChevronRight} size="sm" className={cn('transition-transform duration-200', open && 'rotate-90')} />
          </button>
        </span>
      </div>
    </ContextMenuArea>
  )
}

export function Sidebar({ onNavigate }: { onNavigate?: () => void }) {
  const t = useT()
  const navigate = useNavigate()
  const d = useDownloads()
  const { views, categories, queues, cloudDevices, linkedDevices, sidebarSelection: current } = d
  const cloudConnection = useAgent((snapshot) => snapshot.cloudConnection, undefined)
  const localReady = useConnection().phase === 'ready'
  const cloudReady = cloudPresenceKnown(cloudConnection, localReady)

  const [sectionOpen, setSectionOpen] = useState<Record<Section, boolean>>({ status: true, queues: true, devices: true })
  const [expanded, setExpanded] = useState<DownloadStatusFilter | null>('all')

  const showStatus = usePrefBool(SIDEBAR_SECTION_PREFS.status, true)
  const showQueues = usePrefBool(SIDEBAR_SECTION_PREFS.queues, true)
  const showCategory = usePrefBool(SIDEBAR_SECTION_PREFS.category, true)
  const loggedIn = useAgent((snapshot) => snapshot.session !== null && snapshot.session !== undefined, false)
  const devicesPref = usePref<unknown>(SIDEBAR_SECTION_PREFS.devices)
  // 设备区三态：显式设置按值；未设置时有任何设备才显示。
  const showDevices =
    typeof devicesPref === 'boolean' ? devicesPref : loggedIn || linkedDevices.length > 0

  const categoryEntries = useMemo<CustomCategoryDto[]>(
    () => categories.visible().map((rule) => rule.dto).filter((dto) => dto.builtinType !== 'all'),
    [categories],
  )

  // 计数只取决于与进度无关的字段；进度帧每帧产出新的 views 数组，这里在签名不变时沿用
  // 上一个数组引用，使下面的 useMemo 不按帧对全部任务 × 全部过滤器重算。
  const stableViews = useStableViews(views)
  const counts = useMemo(() => {
    const views = stableViews
    const filterCounts = new Map<string, number>()
    const queueCounts = new Map<string, number>()
    const deviceCounts = new Map<string, number>()
    const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)
    const filters: DownloadFilter[] = []
    for (const status of STATUS_FILTERS) {
      filters.push({ status, category: null })
      for (const dto of categoryEntries) filters.push({ status, category: dto.id })
    }
    for (const view of views) {
      if (view.source === 'local') {
        bump(queueCounts, view.queueId)
        bump(deviceCounts, LOCAL_DEVICE)
      } else {
        bump(deviceCounts, view.toDevice)
      }
      for (const filter of filters) {
        if (filterMatches(filter, view, categories)) bump(filterCounts, `${filter.status}|${filter.category ?? ''}`)
      }
    }
    return { filterCounts, queueCounts, deviceCounts }
  }, [stableViews, categories, categoryEntries])

  const filterCount = (filter: DownloadFilter) => counts.filterCounts.get(`${filter.status}|${filter.category ?? ''}`) ?? 0

  const select = (selection: SidebarSelection) => {
    d.setSidebarSelection(selection)
    onNavigate?.()
  }
  const toggleSection = (section: Section) => setSectionOpen((prev) => ({ ...prev, [section]: !prev[section] }))
  const isSelected = (selection: SidebarSelection) => sameSelection(current, selection)

  const openCategoryEditor = () => {
    onNavigate?.()
    void navigate({ to: '/settings/$category', params: { category: 'general' } })
  }

  const categoryMenu = (dto: CustomCategoryDto): MenuEntry[] => {
    const entries: MenuEntry[] = []
    if (!dto.isBuiltin) entries.push({ type: 'item', key: 'edit', label: t('editCategory'), onSelect: openCategoryEditor })
    entries.push({ type: 'item', key: 'add', label: t('addCategory'), onSelect: openCategoryEditor })
    return entries
  }

  const queueLabel = (queue: QueueDto) =>
    queue.queueId === MAIN_QUEUE_ID ? t('mainQueue') : queue.queueId === LATER_QUEUE_ID ? t('laterQueue') : queue.name

  const queueMenu = (queue: QueueDto): MenuEntry[] => {
    const run = (action: 'start' | 'stop') => () => {
      rpc.daemon.queue[action]({ queueId: queue.queueId }).catch(toastRpcError)
    }
    const entries: MenuEntry[] = []
    if (!queue.isRunning) entries.push({ type: 'item', key: 'start', label: t('startQueueAction'), onSelect: run('start') })
    else if (queue.queueId !== MAIN_QUEUE_ID) entries.push({ type: 'item', key: 'stop', label: t('stopQueueAction'), onSelect: run('stop') })
    entries.push({ type: 'item', key: 'manage', label: t('manageQueueAction'), onSelect: () => openQueueManager(queue.queueId) })
    if (queue.queueId !== MAIN_QUEUE_ID) {
      entries.push(
        { type: 'separator', key: 'sep' },
        {
          type: 'item',
          key: 'delete',
          label: t('deleteQueueAction'),
          destructive: true,
          onSelect: () => {
            void confirmDialog({
              title: t('deleteQueueAction'),
              description: t('queueDeleteConfirmDesc', { name: queueLabel(queue) || queue.queueId }),
              intent: 'destructive',
              okLabel: t('delete'),
            }).then((ok) => {
              if (ok) rpc.daemon.queue.delete({ queueId: queue.queueId }).catch(toastRpcError)
            })
          },
        },
      )
    }
    return entries
  }

  const devices: { id: string; label: string; online: boolean | null }[] = [
    { id: LOCAL_DEVICE, label: t('thisDevice'), online: true },
    ...otherDevices(cloudDevices).map((device) => ({
      id: device.deviceId,
      label: device.name || device.deviceId,
      online: cloudReady ? device.isOnline : null,
    })),
    ...linkedDevices.map((device) => ({
      id: device.fingerprint,
      label: device.name || device.fingerprint,
      online: localReady ? device.online : null,
    })),
  ]
  const openAddDevice = () => {
    onNavigate?.()
    void navigate({ to: '/settings/$category', params: { category: 'account' } })
  }

  const sections: ReactNode[] = []

  if (showStatus) {
    sections.push(
      <div key="status" className="flex w-full flex-col">
        <SectionHeader section="status" open={sectionOpen.status} onToggle={() => toggleSection('status')} />
        <Collapse open={sectionOpen.status}>
          {STATUS_FILTERS.map((status) => {
            const filter: DownloadFilter = { status, category: null }
            const selection: SidebarSelection = { kind: 'download', filter }
            const count = filterCount(filter)
            const selected = isSelected(selection)
            const hasChildren = showCategory && categoryEntries.length > 0
            const open = expanded === status
            const iconClass =
              status === 'failed' && count > 0
                ? 'text-destructive'
                : status === 'incomplete' && count > 0
                  ? 'text-nav-selected-icon'
                  : selected
                    ? 'text-nav-selected-icon'
                    : 'text-muted-foreground'
            const toggle = () => setExpanded((prev) => (prev === status ? null : status))
            return (
              <div key={status} className="flex flex-col">
                <div className="group/row relative">
                  <button
                    type="button"
                    aria-current={selected ? 'true' : undefined}
                    aria-expanded={hasChildren ? open : undefined}
                    onClick={() => {
                      toggle()
                      // 有子项时移动端点击只展开+选中，保持抽屉打开以便选择分类。
                      d.setSidebarSelection(selection)
                      if (!hasChildren) onNavigate?.()
                    }}
                    className={cn(ROW, selected ? 'bg-nav-selected text-foreground' : 'text-muted-foreground hover:bg-nav-hover')}
                  >
                    <span className="flex size-[var(--fx-icon-lg)] shrink-0 items-center justify-center">
                      <Icon
                        icon={STATUS_ICON[status]}
                        size="lg"
                        className={cn(iconClass, hasChildren && 'desktop:group-hover/row:invisible')}
                      />
                    </span>
                    <span className="min-w-0 flex-1 truncate">{t(STATUS_LABEL_KEY[status])}</span>
                    <Trailing count={count} selected={selected} />
                  </button>
                  {hasChildren ? (
                    <button
                      type="button"
                      tabIndex={-1}
                      aria-hidden
                      onClick={toggle}
                      className="absolute left-2 top-1/2 hidden size-[var(--fx-icon-lg)] -translate-y-1/2 items-center justify-center rounded-sm text-foreground desktop:group-hover/row:flex"
                    >
                      <Icon icon={ChevronRight} size="md" className={cn('transition-transform duration-200', open && 'rotate-90')} />
                    </button>
                  ) : null}
                </div>
                {hasChildren ? (
                  <Collapse open={open}>
                    {categoryEntries.map((dto) => {
                      const childFilter: DownloadFilter = { status, category: dto.id }
                      const childSelection: SidebarSelection = { kind: 'download', filter: childFilter }
                      return (
                        <ContextMenuArea key={dto.id} entries={() => categoryMenu(dto)} title={categoryLabel(t, dto)}>
                          <NavRow
                            indent
                            selected={isSelected(childSelection)}
                            label={categoryLabel(t, dto)}
                            icon={categoryIconByKey(dto.icon)}
                            count={filterCount(childFilter)}
                            onClick={() => select(childSelection)}
                          />
                        </ContextMenuArea>
                      )
                    })}
                  </Collapse>
                ) : null}
              </div>
            )
          })}
        </Collapse>
      </div>,
    )
  }

  if (showQueues) {
    sections.push(
      <div key="queues" className="flex w-full flex-col">
        <SectionHeader
          section="queues"
          open={sectionOpen.queues}
          onToggle={() => toggleSection('queues')}
          trailing={
            <button
              type="button"
              aria-label={t('manageQueueAction')}
              onClick={() => openQueueManager()}
              className="flex size-5 coarse:size-touch items-center justify-center rounded-sm text-muted-foreground hover:bg-nav-hover hover:text-foreground"
            >
              <Icon icon={Settings} size="sm" />
            </button>
          }
        />
        <Collapse open={sectionOpen.queues}>
          {queues.map((queue) => {
            const selection: SidebarSelection = { kind: 'queue', queueId: queue.queueId }
            const label = queueLabel(queue)
            return (
              <ContextMenuArea key={queue.queueId} entries={() => queueMenu(queue)} title={label}>
                <NavRow
                  selected={isSelected(selection)}
                  label={label}
                  icon={Rows3}
                  count={counts.queueCounts.get(queue.queueId) ?? 0}
                  dot={queue.isRunning}
                  onClick={() => select(selection)}
                />
              </ContextMenuArea>
            )
          })}
        </Collapse>
      </div>,
    )
  }

  if (showDevices) {
    sections.push(
      <div key="devices" className="flex w-full flex-col">
        <SectionHeader section="devices" open={sectionOpen.devices} onToggle={() => toggleSection('devices')} />
        <Collapse open={sectionOpen.devices}>
          {devices.map((device) => {
            const selection: SidebarSelection = { kind: 'device', deviceId: device.id }
            return (
              <NavRow
                key={device.id}
                selected={isSelected(selection)}
                label={device.online === null ? `${device.label} · ${t('devicePresenceUnknown')}` : device.label}
                icon={device.id === LOCAL_DEVICE ? Cpu : Globe}
                count={counts.deviceCounts.get(device.id) ?? 0}
                dot={device.online === true && device.id !== LOCAL_DEVICE}
                onClick={() => select(selection)}
              />
            )
          })}
          <NavRow
            selected={false}
            label={t('addDeviceEntry')}
            icon={Plus}
            count={0}
            onClick={openAddDevice}
          />
        </Collapse>
      </div>,
    )
  }

  return (
    <nav className="flex h-full w-full min-w-0 flex-col gap-3 overflow-y-auto overflow-x-hidden bg-chrome px-2 pt-2 pb-safe">
      {sections}
    </nav>
  )
}
