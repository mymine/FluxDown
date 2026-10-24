import type { LucideIcon } from 'lucide-react'
import { cn } from '../lib/cn'

/** GPUI icon token：sm 12 / md 14 / lg 16（随界面缩放）；xl = lg + 2（活动栏图标）。线宽 1.75 与 GPUI 一致。 */
export type IconSize = 'sm' | 'md' | 'lg' | 'xl'

const DIMENSION: Record<IconSize, string> = {
  sm: 'var(--fx-icon-sm)',
  md: 'var(--fx-icon-md)',
  lg: 'var(--fx-icon-lg)',
  xl: 'calc(var(--fx-icon-lg) + 2px)',
}

export function Icon({
  icon: Glyph,
  size = 'lg',
  className,
}: {
  icon: LucideIcon
  size?: IconSize
  className?: string
}) {
  const dimension = DIMENSION[size]
  return (
    <Glyph
      aria-hidden
      strokeWidth={1.75}
      className={cn('shrink-0', className)}
      style={{ width: dimension, height: dimension }}
    />
  )
}

/** 以 CSS 蒙版渲染的单色图标（如插件声明的 SVG）：只取图形 alpha，颜色随 `currentColor`。 */
export function MaskIcon({ mask, size = 'lg', className }: { mask: string; size?: IconSize; className?: string }) {
  const dimension = DIMENSION[size]
  return (
    <span
      aria-hidden
      className={cn('inline-block shrink-0 bg-current', className)}
      style={{
        width: dimension,
        height: dimension,
        maskImage: mask,
        WebkitMaskImage: mask,
        maskSize: 'contain',
        WebkitMaskSize: 'contain',
        maskRepeat: 'no-repeat',
        WebkitMaskRepeat: 'no-repeat',
        maskPosition: 'center',
        WebkitMaskPosition: 'center',
      }}
    />
  )
}
