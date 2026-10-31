// 设置页布局原语（GPUI `crates/settings/src/ui.rs`：页 → 子 Tab → 分组卡片 → 设置行）。
// 页面内容在 SettingsLayout 的滚动容器里；这里只负责标题、Tab、分组与行。

import { Info } from 'lucide-react'
import { Children, useLayoutEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../../../i18n'
import { cn } from '../../../lib/cn'
import { Card, Icon, SegmentedTabs, Tooltip, useIsCoarsePointer, useIsMobile } from '../../../ui'
import { SETTINGS_ERROR_KEYS, clearSettingsError, useSettingsError, useSettingsReadOnly } from './writeStore'

export interface SettingsTabDef {
  id: string
  label: string
  content: ReactNode
}

/**
 * 一个设置分类页：标题 + 描述 + 可选子 Tab（≥2 个 Tab 才显示）+ 内容。
 * 写回失败 / 连接只读时顶部显示提示条。
 */
export function SettingsPage({
  title,
  description,
  tabs,
  initialTabId,
  children,
}: {
  title: string
  description: string
  tabs?: readonly SettingsTabDef[]
  /** 首次显示的子 Tab（深链接进入时指定）；缺省为第一个。 */
  initialTabId?: string
  children?: ReactNode
}) {
  const t = useT()
  const mobile = useIsMobile()
  const [tabId, setTabId] = useState<string>(initialTabId ?? tabs?.[0]?.id ?? '')
  const activeTab = tabs?.find((tab) => tab.id === tabId) ?? tabs?.[0]
  const error = useSettingsError()
  const readOnly = useSettingsReadOnly()
  const banner = error
    ? { text: `${t(SETTINGS_ERROR_KEYS[error.kind])}${error.kind === 'invalidArgument' && error.detail ? `: ${error.detail}` : ''}`, destructive: true }
    : readOnly
      ? { text: t('localServiceDisconnected'), destructive: false }
      : null
  return (
    <div className="flex min-h-full flex-col bg-surface">
      <header className="sticky top-0 z-10 flex flex-col gap-3 border-b border-hairline bg-surface px-4 pt-4 pb-3 desktop:px-7">
        <div className="flex min-w-0 flex-col gap-0.5">
          {mobile ? null : <h1 className="truncate text-title font-semibold text-foreground">{title}</h1>}
          <p className="text-xs text-muted-foreground">{description}</p>
        </div>
        {tabs && tabs.length > 1 ? (
          <SegmentedTabs items={tabs.map((tab) => ({ value: tab.id, label: tab.label }))} value={activeTab?.id ?? ''} onValueChange={setTabId} className="self-start" />
        ) : null}
        {banner ? (
          <div
            role={banner.destructive ? 'alert' : 'status'}
            className={cn(
              'flex items-start gap-2 rounded-md border px-3 py-2 text-xs',
              banner.destructive ? 'border-destructive/40 bg-destructive/10 text-destructive' : 'border-hairline bg-nav-hover text-muted-foreground',
            )}
          >
            <span className="min-w-0 flex-1">{banner.text}</span>
            {banner.destructive ? (
              <button type="button" className="shrink-0 underline coarse:min-h-touch" onClick={clearSettingsError}>
                {t('close')}
              </button>
            ) : null}
          </div>
        ) : null}
      </header>
      <div className="flex flex-col gap-4 px-4 pt-5 pb-safe desktop:px-7 desktop:pb-8">
        <div className="flex w-full max-w-[860px] flex-col gap-4 pb-6">{activeTab ? activeTab.content : children}</div>
      </div>
    </div>
  )
}

/** 一组设置：可选小节标题/副标题 + 一张卡片（行间 hairline）。 */
export function SettingsSection({
  title,
  subtitle,
  children,
  className,
}: {
  title?: string
  subtitle?: string
  children: ReactNode
  className?: string
}) {
  const rows = Children.toArray(children)
  if (rows.length === 0) return null
  return (
    <section className={cn('flex w-full flex-col', className)}>
      {title || subtitle ? (
        <div className="flex flex-col gap-0.5 px-1 pb-2">
          {title ? <h2 className="text-caption font-medium text-text-tertiary">{title}</h2> : null}
          {subtitle ? <p className="text-xs text-muted-foreground">{subtitle}</p> : null}
        </div>
      ) : null}
      <Card className="flex w-full flex-col overflow-hidden [&>*+*]:border-t [&>*+*]:border-hairline">{rows}</Card>
    </section>
  )
}

/**
 * 行标题 + 说明。说明只占一行，超出时截断；超出或有 `help`（长说明）时标题旁显示信息图标，
 * 鼠标悬浮图标即显示完整说明。触屏无 hover，改为点击图标原地展开。
 * 对齐 GPUI `SettingsRow` 的单行说明规则。
 */
function RowLabel({ title, description, help }: { title: ReactNode; description?: ReactNode; help: string | undefined }) {
  const coarse = useIsCoarsePointer()
  const [open, setOpen] = useState(false)
  const [overflows, setOverflows] = useState(false)
  const descriptionRef = useRef<HTMLDivElement>(null)
  const expanded = coarse && open
  useLayoutEffect(() => {
    const node = descriptionRef.current
    // 展开时说明不截断，无从测量；保留收起时的结论。
    if (!node || expanded) return
    const measure = () => setOverflows(node.scrollWidth > node.clientWidth)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(node)
    return () => observer.disconnect()
  }, [expanded, description])
  const expandable = Boolean(help) || overflows
  const details = (
    <div className="flex flex-col gap-2 whitespace-pre-line">
      {overflows && description ? <div>{description}</div> : null}
      {help ? <div>{help}</div> : null}
    </div>
  )
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <div className="flex items-center gap-1 text-sm text-foreground">
        <span className="min-w-0">{title}</span>
        {expandable ? (
          <Tooltip content={details}>
            <button
              type="button"
              aria-expanded={coarse ? open : undefined}
              aria-label={help ?? (typeof description === 'string' ? description : undefined)}
              onClick={coarse ? () => setOpen((value) => !value) : undefined}
              className="inline-flex cursor-help items-center justify-center rounded-sm text-text-tertiary hover:text-foreground coarse:size-8"
            >
              <Icon icon={Info} />
            </button>
          </Tooltip>
        ) : null}
      </div>
      {description ? (
        <div
          ref={descriptionRef}
          className={cn('text-xs text-muted-foreground', !expanded && 'truncate')}
        >
          {description}
        </div>
      ) : null}
      {help && expanded ? <div className="whitespace-pre-line text-xs text-muted-foreground">{help}</div> : null}
    </div>
  )
}

/**
 * 「标题 + 说明 + 右侧控件」行。
 * - 桌面：标题列（保底 160px）与控件同一行；`vertical` 时控件在标题下方独占一行。
 * - 移动端：默认控件换到标题下方全宽；`compact`（开关等小控件）保持同行。
 */
export function SettingsRow({
  title,
  description,
  help,
  children,
  footer,
  vertical = false,
  compact = false,
  disabled = false,
  className,
}: {
  title: ReactNode
  description?: ReactNode
  /** 标题旁信息图标，点击展开长说明。 */
  help?: string
  children?: ReactNode
  /** 行下方附加区（随值动态出现的提示等），与本行同属一格。 */
  footer?: ReactNode
  vertical?: boolean
  compact?: boolean
  disabled?: boolean
  className?: string
}) {
  const label = <RowLabel title={title} description={description} help={help} />
  const stacked = vertical
  const row = (
    <div
      className={cn(
        'w-full px-4',
        stacked ? 'py-3' : 'flex min-h-12 gap-3 py-2 desktop:items-center desktop:gap-4',
        !stacked && !compact && 'mobile:flex-col mobile:py-3',
        !stacked && compact && 'items-center coarse:min-h-touch',
        disabled && 'pointer-events-none opacity-50',
        footer === undefined && className,
      )}
    >
      {stacked ? (
        <div className="flex w-full flex-col gap-2">
          {label}
          {children ? <div className="w-full min-w-0">{children}</div> : null}
        </div>
      ) : (
        <>
          <div className="min-w-0 flex-1 desktop:min-w-[160px]">{label}</div>
          {children ? (
            <div className={cn('min-w-0', compact ? 'shrink-0' : 'mobile:w-full desktop:max-w-full')}>{children}</div>
          ) : null}
        </>
      )}
    </div>
  )
  if (footer === undefined) return row
  return (
    <div className={cn('w-full', className)}>
      {row}
      <div className={cn('px-4', disabled && 'opacity-50')}>{footer}</div>
    </div>
  )
}

/** 整行自渲染（列表、按钮组）。 */
export function SettingsCustomRow({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn('w-full px-4 py-3', className)}>{children}</div>
}
