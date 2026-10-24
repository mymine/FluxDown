import { forwardRef, useCallback, useEffect, useLayoutEffect, useRef } from 'react'
import type { InputHTMLAttributes, ReactNode, TextareaHTMLAttributes } from 'react'
import { cn } from '../lib/cn'

const FIELD =
  'w-full min-w-0 rounded-md border border-input bg-transparent text-foreground placeholder:text-text-tertiary ' +
  'transition-colors hover:border-ring/60 focus-visible:border-ring focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring ' +
  'disabled:cursor-not-allowed disabled:opacity-50'

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  invalid?: boolean
  /** 输入框内右侧附件（如显示/隐藏密码按钮）。 */
  trailing?: ReactNode
}

/** 单行输入：`density.control` 高（触屏 44px）。 */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { invalid, trailing, className, ...rest },
  ref,
) {
  const field = (
    <input
      ref={ref}
      aria-invalid={invalid || undefined}
      className={cn(
        FIELD,
        'h-control px-2.5 coarse:min-h-touch',
        invalid && 'border-destructive hover:border-destructive focus-visible:border-destructive focus-visible:ring-destructive',
        trailing ? 'pr-10' : undefined,
        className,
      )}
      {...rest}
    />
  )
  if (!trailing) return field
  return (
    <div className="relative w-full min-w-0">
      {field}
      {/* 输入框内的附件按钮缩到 24px（触屏 36px）：28px 的按钮会盖住输入框上下边框。 */}
      <div className="absolute inset-y-0 right-0 flex items-center pr-0.5 [&>button]:size-6 coarse:[&>button]:size-9 coarse:[&>button]:min-h-9 coarse:[&>button]:min-w-9">
        {trailing}
      </div>
    </div>
  )
})

export interface TextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  invalid?: boolean
  /** 随内容自增高：从 1 行起、最多 `maxAutoRows` 行（含软换行），超出后框内滚动；设置后忽略 `rows`。 */
  maxAutoRows?: number
}

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(function Textarea(
  { invalid, className, rows = 3, maxAutoRows, onInput, ...rest },
  ref,
) {
  const inner = useRef<HTMLTextAreaElement | null>(null)
  const setRef = useCallback(
    (node: HTMLTextAreaElement | null) => {
      inner.current = node
      if (typeof ref === 'function') ref(node)
      else if (ref) ref.current = node
    },
    [ref],
  )
  const fit = useCallback(() => {
    const el = inner.current
    if (!el || !maxAutoRows) return
    const style = getComputedStyle(el)
    const parsedLine = parseFloat(style.lineHeight)
    const line = Number.isFinite(parsedLine) ? parsedLine : parseFloat(style.fontSize) * 1.2
    const borders = parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth)
    const padding = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
    const max = line * maxAutoRows + padding + borders
    // 先收回到 auto 再量 scrollHeight，删除内容时才能缩回去。
    el.style.height = 'auto'
    const content = el.scrollHeight + borders
    el.style.height = `${Math.min(content, max)}px`
    el.style.overflowY = content > max ? 'auto' : 'hidden'
  }, [maxAutoRows])
  // 受控值变化（含程序写入，如导入 TXT）后重新适配高度。
  useLayoutEffect(fit, [fit, rest.value])
  // 宽度变化会改变软换行行数。
  useEffect(() => {
    const el = inner.current
    if (!el || !maxAutoRows || typeof ResizeObserver === 'undefined') return
    let width = el.clientWidth
    const observer = new ResizeObserver(() => {
      if (el.clientWidth === width) return
      width = el.clientWidth
      fit()
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [fit, maxAutoRows])
  const auto = maxAutoRows !== undefined
  return (
    <textarea
      ref={setRef}
      rows={auto ? 1 : rows}
      aria-invalid={invalid || undefined}
      onInput={(event) => {
        if (auto) fit()
        onInput?.(event)
      }}
      className={cn(
        FIELD,
        auto ? 'resize-none px-2.5 py-1.5' : 'resize-y px-2.5 py-1.5',
        invalid && 'border-destructive hover:border-destructive focus-visible:border-destructive focus-visible:ring-destructive',
        className,
      )}
      {...rest}
    />
  )
})
