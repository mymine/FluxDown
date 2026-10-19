// 推介页共用小部件：提示卡 / 加载失败 / 分页条。

import { ChevronLeft, ChevronRight, Info } from 'lucide-react'
import type { ReactNode } from 'react'
import { useT } from '../../../../i18n'
import { Button, Card, Icon, Spinner } from '../../../../ui'
import { pageCount, REFERRAL_PAGE_SIZE } from '../account/billing'

export function NoticeCard({ children }: { children: ReactNode }) {
  return (
    <Card className="flex items-start gap-2 border-warning/30 bg-warning/8 px-3 py-2.5 text-xs text-warning">
      <Icon icon={Info} className="mt-px shrink-0" />
      <span className="min-w-0 flex-1 break-words">{children}</span>
    </Card>
  )
}

export function LoadingBlock() {
  return (
    <div className="flex items-center justify-center py-12">
      <Spinner />
    </div>
  )
}

export function LoadFailed({ message, onRetry }: { message: string; onRetry: () => void }) {
  const t = useT()
  return (
    <div className="flex flex-col items-center gap-2 py-10 text-center">
      <div className="text-sm text-muted-foreground">{message}</div>
      <Button onClick={onRetry}>{t('accountDevicesRetry')}</Button>
    </div>
  )
}

/** 多页时才渲染：上一页 / 「第 x / y 页」/ 下一页。 */
export function Pager({ page, total, onPage }: { page: number; total: number; onPage: (page: number) => void }) {
  const t = useT()
  const pages = pageCount(total, REFERRAL_PAGE_SIZE)
  if (pages <= 1) return null
  return (
    <div className="flex items-center justify-center gap-3 pt-1">
      <Button variant="ghost" iconOnly aria-label={t('progressWindowPrevious')} disabled={page <= 1} onClick={() => onPage(page - 1)}>
        <Icon icon={ChevronLeft} />
      </Button>
      <span className="text-xs text-muted-foreground tabular">{t('accountReferralPageIndicator', { x: page, y: pages })}</span>
      <Button variant="ghost" iconOnly aria-label={t('progressWindowNext')} disabled={page >= pages} onClick={() => onPage(page + 1)}>
        <Icon icon={ChevronRight} />
      </Button>
    </div>
  )
}
