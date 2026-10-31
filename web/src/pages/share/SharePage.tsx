// FluxMesh页（活动栏路由 `/share`，可隐藏）：顶部一行摘要（额度 / 贡献 / 共享开关 / 额度详情），下方「我的分享」列表。
// 对应 GPUI `crates/share` 的分享页；接收记录、完整额度、网络诊断分别在下载任务详情、设置 › 账户、设置 › FluxMesh。

import { Share2 } from 'lucide-react'
import { useT } from '../../i18n'
import { EmptyState } from '../../ui'
import { MyShares } from './MyShares'
import { ShareSummary } from './ShareSummary'
import { useShareAvailable } from './state'

export function SharePage() {
  const t = useT()
  const available = useShareAvailable()

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col bg-surface text-foreground">
      <header className="flex flex-none flex-col gap-3 border-b border-hairline px-6 pt-4 pb-3 mobile:px-4">
        <h1 className="text-title font-semibold text-foreground">{t('shareNavTitle')}</h1>
        {available ? <ShareSummary /> : null}
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden px-6 pt-5 pb-6 mobile:px-4 mobile:pt-4">
        <div className="mx-auto flex w-full min-w-0 max-w-5xl flex-col gap-4">
          {available ? <MyShares /> : <EmptyState icon={Share2} title={t('errReasonShareUnavailable')} />}
        </div>
      </div>
    </div>
  )
}
