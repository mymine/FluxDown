// FluxMesh面板 / 接收确认框的外壳：桌面居中对话框，窄屏（≤820px）底部抽屉（`Sheet side="bottom"`）。

import type { ReactNode } from 'react'
import { Dialog, Sheet, useIsMobile } from '../../ui'
import type { DialogSize } from '../../ui'

export function ShareSurface({
  open,
  onOpenChange,
  title,
  description,
  footer,
  children,
  size = 'md',
  persistent,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: ReactNode
  description?: ReactNode
  footer?: ReactNode
  children: ReactNode
  size?: DialogSize
  persistent?: boolean
}) {
  const mobile = useIsMobile()
  if (mobile) {
    return (
      <Sheet open={open} onOpenChange={onOpenChange} side="bottom" title={title} footer={footer}>
        <div className="flex flex-col gap-3 px-4 pb-3">
          {description ? <p className="text-xs text-muted-foreground">{description}</p> : null}
          {children}
        </div>
      </Sheet>
    )
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={title} description={description} footer={footer} size={size} persistent={persistent}>
      <div className="flex flex-col gap-3 pb-2">{children}</div>
    </Dialog>
  )
}
