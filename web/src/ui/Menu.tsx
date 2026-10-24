// 菜单：同一份条目模型三种呈现——
//   桌面 `<ActionMenu>`（Radix DropdownMenu）、桌面右键 `<ContextMenuArea>`（Radix ContextMenu）、
//   移动端（<=820px / 触屏）底部 Sheet 里的 44px 行列表（子菜单为「进入 / 返回」两级）。
// 页面只描述 `MenuEntry[]`，不关心呈现方式。

import * as Dropdown from '@radix-ui/react-dropdown-menu'
import * as Context from '@radix-ui/react-context-menu'
import { Check, ChevronLeft, ChevronRight } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useCallback, useRef, useState } from 'react'
import type { ReactNode, TouchEvent } from 'react'
import { useT } from '../i18n'
import { cn } from '../lib/cn'
import { useIsMobile } from './hooks'
import { Icon } from './Icon'
import { Sheet } from './Sheet'
import { PathText } from './PathText'

export type MenuEntry =
  | {
      type: 'item'
      key: string
      label: string
      icon?: LucideIcon
      onSelect: () => void
      destructive?: boolean
      disabled?: boolean
      shortcut?: string
      /** 标签下方的单行次要说明（超长省略）。 */
      description?: string
      /** 说明是文件路径：中间省略、保留末段目录名，宽度封顶。 */
      descriptionIsPath?: boolean
      /** 选中态：右侧显示勾选。 */
      checked?: boolean
    }
  | { type: 'check'; key: string; label: string; checked: boolean; onSelect: () => void; disabled?: boolean }
  | { type: 'sub'; key: string; label: string; icon?: LucideIcon; entries: readonly MenuEntry[]; disabled?: boolean }
  | { type: 'label'; key: string; label: string }
  | { type: 'separator'; key: string }

const ITEM =
  'flex min-h-control cursor-pointer select-none items-center gap-2 rounded-sm px-2 text-sm text-foreground outline-none ' +
  'data-[highlighted]:bg-nav-hover data-[disabled]:pointer-events-none data-[disabled]:opacity-50'
const PANEL =
  'animate-fx-pop z-50 max-h-[calc(100dvh-16px)] min-w-[10rem] max-w-[calc(100vw-16px)] overflow-y-auto rounded-md border border-hairline bg-surface p-1 shadow-md'

type Kit = typeof Dropdown | typeof Context

function MenuDescription({ entry }: { entry: Extract<MenuEntry, { type: 'item' }> }) {
  if (!entry.description) return null
  if (entry.descriptionIsPath) return <PathText path={entry.description} className="max-w-[26rem] text-caption text-text-tertiary" />
  return <span className="truncate text-caption text-text-tertiary">{entry.description}</span>
}

function renderEntries(kit: Kit, entries: readonly MenuEntry[]): ReactNode {
  return entries.map((entry) => {
    switch (entry.type) {
      case 'separator':
        return <kit.Separator key={entry.key} className="my-1 h-px bg-hairline" />
      case 'label':
        return (
          <kit.Label key={entry.key} className="px-2 py-1 text-caption text-text-tertiary">
            {entry.label}
          </kit.Label>
        )
      case 'check':
        return (
          <kit.CheckboxItem
            key={entry.key}
            checked={entry.checked}
            disabled={entry.disabled}
            onSelect={entry.onSelect}
            className={cn(ITEM, 'relative pl-7')}
          >
            <kit.ItemIndicator className="absolute left-2">
              <Icon icon={Check} size="md" />
            </kit.ItemIndicator>
            {entry.label}
          </kit.CheckboxItem>
        )
      case 'sub':
        return (
          <kit.Sub key={entry.key}>
            <kit.SubTrigger disabled={entry.disabled} className={cn(ITEM, 'data-[state=open]:bg-nav-hover')}>
              {entry.icon ? <Icon icon={entry.icon} size="md" /> : null}
              <span className="flex-1">{entry.label}</span>
              <Icon icon={ChevronRight} size="md" className="text-text-tertiary" />
            </kit.SubTrigger>
            <kit.Portal>
              <kit.SubContent className={PANEL}>{renderEntries(kit, entry.entries)}</kit.SubContent>
            </kit.Portal>
          </kit.Sub>
        )
      case 'item':
        return (
          <kit.Item
            key={entry.key}
            disabled={entry.disabled}
            onSelect={entry.onSelect}
            className={cn(ITEM, entry.destructive && 'text-destructive')}
          >
            {entry.icon ? <Icon icon={entry.icon} size="md" /> : null}
            <span className="flex min-w-0 flex-1 flex-col py-1">
              <span>{entry.label}</span>
              <MenuDescription entry={entry} />
            </span>
            {entry.shortcut ? <span className="text-caption text-text-tertiary">{entry.shortcut}</span> : null}
            {entry.checked ? <Icon icon={Check} size="md" className="text-nav-selected-icon" /> : null}
          </kit.Item>
        )
    }
  })
}

/** 移动端：底部面板里的 44px 行；子菜单原地替换为二级列表。 */
function SheetList({ entries, close, title }: { entries: readonly MenuEntry[]; close: () => void; title: string }) {
  const t = useT()
  const [stack, setStack] = useState<{ label: string; entries: readonly MenuEntry[] }[]>([])
  const current = stack.length > 0 ? stack[stack.length - 1] : undefined
  const list = current?.entries ?? entries
  return (
    <div className="flex flex-col py-1" aria-label={current?.label ?? title}>
      {current ? (
        <button
          type="button"
          className="flex min-h-touch items-center gap-2 px-4 text-sm text-muted-foreground active:bg-nav-hover"
          onClick={() => setStack((s) => s.slice(0, -1))}
        >
          <Icon icon={ChevronLeft} />
          {current.label} · {t('back')}
        </button>
      ) : null}
      {list.map((entry) => {
        switch (entry.type) {
          case 'separator':
            return <div key={entry.key} className="my-1 h-px bg-hairline" />
          case 'label':
            return (
              <div key={entry.key} className="px-4 pt-2 pb-1 text-caption text-text-tertiary">
                {entry.label}
              </div>
            )
          default: {
            const disabled = entry.disabled === true
            const onClick = () => {
              if (entry.type === 'sub') setStack((s) => [...s, { label: entry.label, entries: entry.entries }])
              else {
                close()
                entry.onSelect()
              }
            }
            const icon = entry.type === 'item' || entry.type === 'sub' ? entry.icon : undefined
            return (
              <button
                key={entry.key}
                type="button"
                disabled={disabled}
                onClick={onClick}
                className={cn(
                  'flex min-h-touch items-center gap-3 px-4 text-left text-sm active:bg-nav-hover disabled:opacity-50',
                  entry.type === 'item' && entry.destructive ? 'text-destructive' : 'text-foreground',
                )}
              >
                {icon ? <Icon icon={icon} /> : entry.type === 'check' ? <span className="size-4" /> : null}
                <span className="flex min-w-0 flex-1 flex-col">
                  <span>{entry.label}</span>
                  {entry.type === 'item' ? <MenuDescription entry={entry} /> : null}
                </span>
                {(entry.type === 'check' || entry.type === 'item') && entry.checked ? <Icon icon={Check} className="text-nav-selected-icon" /> : null}
                {entry.type === 'sub' ? <Icon icon={ChevronRight} className="text-text-tertiary" /> : null}
              </button>
            )
          }
        }
      })}
    </div>
  )
}

/** 下拉动作菜单：桌面 Radix 下拉，移动端底部面板。`trigger` 必须是可聚焦元素（如 `<Button>`）。 */
export function ActionMenu({
  trigger,
  entries,
  title,
  align = 'end',
}: {
  trigger: ReactNode
  entries: readonly MenuEntry[]
  /** 移动端面板标题。 */
  title: string
  align?: 'start' | 'center' | 'end'
}) {
  const mobile = useIsMobile()
  const [open, setOpen] = useState(false)
  if (mobile) {
    return (
      <>
        <span className="inline-flex" onClick={() => setOpen(true)}>
          {trigger}
        </span>
        <Sheet open={open} onOpenChange={setOpen} side="bottom" title={title}>
          <SheetList entries={entries} close={() => setOpen(false)} title={title} />
        </Sheet>
      </>
    )
  }
  return (
    <Dropdown.Root>
      <Dropdown.Trigger asChild>{trigger}</Dropdown.Trigger>
      <Dropdown.Portal>
        <Dropdown.Content align={align} sideOffset={4} collisionPadding={8} className={PANEL}>
          {renderEntries(Dropdown, entries)}
        </Dropdown.Content>
      </Dropdown.Portal>
    </Dropdown.Root>
  )
}

/** 仅在右键菜单内容挂载（即菜单打开）时才求值 entries。 */
function LazyEntries({ resolve }: { resolve: () => readonly MenuEntry[] }) {
  return <>{renderEntries(Context, resolve())}</>
}

const LONG_PRESS_MS = 450

/**
 * 右键菜单区域：桌面右键（Radix ContextMenu），触屏长按 → 底部面板。
 * `entries` 可为函数（在打开时才计算，适合依赖选中状态的菜单）。
 */
export function ContextMenuArea({
  children,
  entries,
  title,
  disabled,
  className,
}: {
  children: ReactNode
  entries: readonly MenuEntry[] | (() => readonly MenuEntry[])
  title: string
  disabled?: boolean
  className?: string
}) {
  const mobile = useIsMobile()
  const [sheet, setSheet] = useState<readonly MenuEntry[] | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const start = useRef<{ x: number; y: number } | null>(null)
  const resolve = useCallback(() => (typeof entries === 'function' ? entries() : entries), [entries])

  const cancel = () => {
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = null
  }
  const onTouchStart = (event: TouchEvent) => {
    if (disabled) return
    const touch = event.touches[0]
    if (!touch) return
    start.current = { x: touch.clientX, y: touch.clientY }
    cancel()
    timer.current = setTimeout(() => {
      timer.current = null
      setSheet(resolve())
    }, LONG_PRESS_MS)
  }
  const onTouchMove = (event: TouchEvent) => {
    const touch = event.touches[0]
    const origin = start.current
    if (!touch || !origin) return
    if (Math.hypot(touch.clientX - origin.x, touch.clientY - origin.y) > 10) cancel()
  }

  if (disabled) return <div className={className}>{children}</div>
  if (mobile) {
    return (
      <div
        className={className}
        onTouchStart={onTouchStart}
        onTouchMove={onTouchMove}
        onTouchEnd={cancel}
        onTouchCancel={cancel}
        onContextMenu={(event) => {
          event.preventDefault()
          setSheet(resolve())
        }}
      >
        {children}
        <Sheet open={sheet !== null} onOpenChange={(open) => !open && setSheet(null)} side="bottom" title={title}>
          {sheet ? <SheetList entries={sheet} close={() => setSheet(null)} title={title} /> : null}
        </Sheet>
      </div>
    )
  }
  return (
    <Context.Root>
      <Context.Trigger asChild>
        <div className={className}>{children}</div>
      </Context.Trigger>
      <Context.Portal>
        <Context.Content collisionPadding={8} className={PANEL}>
          <LazyEntries resolve={resolve} />
        </Context.Content>
      </Context.Portal>
    </Context.Root>
  )
}
