import { useT } from '../../../i18n'
import { Spinner } from '../../../ui'
import { useDownloads } from '../state'

/**
 * 任务表分页载入期间的占位（取代列表与空态）：此时本地任务不完整，不能显示「暂无下载任务」，
 * 也不能展示半份列表。显示服务端任务总数与已载入行数。
 */
export function TaskLoading() {
  const t = useT()
  const { taskTotal, loadedTasks } = useDownloads()
  return (
    <div role="status" aria-live="polite" className="flex h-full flex-col items-center justify-center gap-2 bg-surface p-6 text-center">
      <Spinner />
      <div className="text-sm font-medium text-foreground">{t('tasksLoadingTitle')}</div>
      <div className="text-xs text-muted-foreground">{t('tasksLoadingSubtitle', { total: taskTotal })}</div>
      {loadedTasks > 0 ? (
        <div className="text-xs tabular text-text-tertiary">
          {t('webTasksLoadingProgress', { loaded: loadedTasks, total: taskTotal })}
        </div>
      ) : null}
    </div>
  )
}
