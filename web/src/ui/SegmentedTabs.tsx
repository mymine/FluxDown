import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type WheelEvent } from 'react'
import { cn } from '../lib/cn'

export interface SegmentedItem<V extends string> {
  value: V
  label: ReactNode
  disabled?: boolean
}

/** 渐隐宽度（px），与 GPUI `segmented_tabs` 的 `spacing.xl` 一致。 */
const FADE = 24

/**
 * 分段标签（GPUI `segmented_tabs`）：nav-hover 轨道内，选中项 surface 底 + 细阴影 + 中等字重。
 * 轨道高 `density.control`，项高再减 4。
 *
 * 放不下全部标签时（如窄侧栏里的 BT 详情）轨道收缩到可用宽度，标签行横向滚动：纵向滚轮
 * 同样横滚，被裁一侧渐隐提示，选中项变化时自动滚入视野。
 */
export function SegmentedTabs<V extends string>({
  items,
  value,
  onValueChange,
  className,
  'aria-label': ariaLabel,
}: {
  items: readonly SegmentedItem<V>[]
  value: V
  onValueChange: (value: V) => void
  className?: string
  'aria-label'?: string
}) {
  const scroller = useRef<HTMLDivElement>(null)
  const [fades, setFades] = useState<{ left: boolean; right: boolean }>({ left: false, right: false })

  const measure = useCallback(() => {
    const el = scroller.current
    if (!el) return
    const left = el.scrollLeft > 0.5
    const right = el.scrollLeft < el.scrollWidth - el.clientWidth - 0.5
    setFades((current) => (current.left === left && current.right === right ? current : { left, right }))
  }, [])

  useEffect(() => {
    const el = scroller.current
    if (!el) return
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    for (const child of Array.from(el.children)) observer.observe(child)
    measure()
    return () => observer.disconnect()
  }, [items, measure])

  // 选中项最小幅度滚入视野（含首次渲染，例如面板打开时停在靠后的标签）。
  useLayoutEffect(() => {
    const el = scroller.current
    const tab = el?.querySelector<HTMLElement>('[aria-selected="true"]')
    if (!el || !tab) return
    const left = tab.offsetLeft
    const right = left + tab.offsetWidth
    if (left < el.scrollLeft) el.scrollLeft = left
    else if (right > el.scrollLeft + el.clientWidth) el.scrollLeft = right - el.clientWidth
    measure()
  }, [value, items, measure])

  // 鼠标滚轮只有纵向增量：没有横向增量时把纵向转成横滚（触控板横扫保持原生行为）。
  const onWheel = (event: WheelEvent<HTMLDivElement>) => {
    const el = event.currentTarget
    if (event.deltaX !== 0 || event.deltaY === 0 || el.scrollWidth <= el.clientWidth) return
    el.scrollLeft += event.deltaY
  }

  const mask =
    fades.left || fades.right
      ? `linear-gradient(to right, ${fades.left ? 'transparent' : '#000'} 0, #000 ${fades.left ? FADE : 0}px, #000 calc(100% - ${fades.right ? FADE : 0}px), ${fades.right ? 'transparent' : '#000'} 100%)`
      : undefined

  return (
    <div
      className={cn(
        'inline-flex h-control min-w-0 max-w-full items-center rounded-[calc(var(--fx-components-tab-radius)+1px)] bg-nav-hover p-0.5 coarse:h-11',
        className,
      )}
    >
      <div
        ref={scroller}
        role="tablist"
        aria-label={ariaLabel}
        onScroll={measure}
        onWheel={onWheel}
        className="relative flex h-full min-w-0 items-center gap-0.5 overflow-x-auto [scrollbar-width:none]"
        style={mask ? { maskImage: mask, WebkitMaskImage: mask } : undefined}
      >
        {items.map((item) => {
          const active = item.value === value
          return (
            <button
              key={item.value}
              type="button"
              role="tab"
              aria-selected={active}
              disabled={item.disabled}
              onClick={() => onValueChange(item.value)}
              className={cn(
                'inline-flex h-[calc(var(--fx-density-control)-4px)] shrink-0 items-center whitespace-nowrap rounded-[var(--fx-components-tab-radius)] px-3 text-sm transition-colors coarse:h-full',
                active
                  ? 'bg-surface font-medium text-foreground shadow-sm'
                  : 'text-muted-foreground hover:text-foreground',
                'disabled:pointer-events-none disabled:opacity-50',
              )}
            >
              {item.label}
            </button>
          )
        })}
      </div>
    </div>
  )
}
