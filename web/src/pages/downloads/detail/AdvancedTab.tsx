import { useT } from '../../../i18n'
import type { DownloadTaskView } from '../model/task'
import { routeLabel } from '../../../lib/taskRoute'
import { DetailRow } from './DetailRow'

export function AdvancedTab({ view }: { view: DownloadTaskView }) {
  const t = useT()
  const dto = view.dto
  return (
    <div className="flex flex-col">
      <DetailRow label={t('infoSourcePage')}>{view.referrer === '' ? t('detailNotSet') : view.referrer}</DetailRow>
      <DetailRow label={t('taskProxy')}>{dto && dto.proxyUrl !== '' ? dto.proxyUrl : t('detailFollowGlobal')}</DetailRow>
      {dto && dto.autoRoute !== '' ? <DetailRow label={t('taskRoute')}>{routeLabel(t, dto.autoRoute)}</DetailRow> : null}
      <DetailRow label={t('taskIgnoreTlsErrors')}>{dto?.ignoreTlsErrors ? '✓' : '—'}</DetailRow>
    </div>
  )
}
