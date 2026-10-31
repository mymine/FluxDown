// 详情面板标题栏 / 详情页内的「分享」按钮：未完成 / 文件丢失 / 远程任务 / 未登录时置灰并以 tooltip 说明原因；
// 未编入FluxMesh时不渲染。

import { Share2 } from 'lucide-react'
import { useT } from '../../i18n'
import { Button, Icon, Tooltip } from '../../ui'
import { useSendState } from './send'
import { openSharePanel } from './store'

interface ShareableTask {
  taskId: string
  source: 'local' | 'remote'
  state: string
  fileMissing: boolean
}

export function ShareTaskButton({ views, iconOnly = true }: { views: readonly ShareableTask[]; iconOnly?: boolean }) {
  const t = useT()
  const state = useSendState(views)
  if (state === 'hidden') return null
  const enabled = state === 'ok'
  const label = t('sharePanelTitle')
  // 置灰按钮不触发指针事件：tooltip 挂在外层 span 上。
  return (
    <Tooltip content={enabled ? label : t(state)}>
      <span className="inline-flex">
        <Button
          variant={iconOnly ? 'ghost' : 'outline'}
          iconOnly={iconOnly}
          icon={iconOnly ? undefined : Share2}
          aria-label={label}
          disabled={!enabled}
          onClick={() => openSharePanel(views.map((view) => view.taskId))}
        >
          {iconOnly ? <Icon icon={Share2} /> : label}
        </Button>
      </span>
    </Tooltip>
  )
}
