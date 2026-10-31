// 举报分享：5 个原因单选（顺序固定）+ 补充说明 + 联系方式；提交经 `agent.share.report`。
// 与 GPUI `crates/share/src/report.rs` 对齐。

import { useState } from 'react'
import { useT } from '../../i18n'
import { rpc, SHARE_REPORT_REASONS } from '../../lib/rpc'
import type { ShareReportReason } from '../../lib/rpc'
import { ConfirmFooter, FieldError, FieldLabel, Input, Textarea, toast } from '../../ui'
import { accountErrorText } from '../settings/sections/account/errorText'
import { ShareSurface } from './ShareSurface'

const REASON_KEYS: Readonly<Record<ShareReportReason, string>> = {
  copyright: 'shareReportReasonCopyright',
  porn: 'shareReportReasonPorn',
  illegal: 'shareReportReasonIllegal',
  malware: 'shareReportReasonMalware',
  other: 'shareReportReasonOther',
}

const DETAIL_MAX = 2000
const CONTACT_MAX = 200

export function ReportDialog({ shareId, onClose }: { shareId: string; onClose: () => void }) {
  const t = useT()
  const [reason, setReason] = useState<ShareReportReason>('copyright')
  const [detail, setDetail] = useState('')
  const [contact, setContact] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const d = detail.trim()
      const c = contact.trim()
      await rpc.agent.share.report({ shareId, reason, ...(d === '' ? {} : { detail: d }), ...(c === '' ? {} : { contact: c }) })
      toast.success(t('shareReportSent'))
      onClose()
    } catch (failure) {
      setError(accountErrorText(t, failure))
      setBusy(false)
    }
  }

  return (
    <ShareSurface
      open
      onOpenChange={(open) => !open && !busy && onClose()}
      title={t('shareReportTitle')}
      persistent
      footer={<ConfirmFooter cancelLabel={t('cancel')} okLabel={t('shareReportSubmit')} onCancel={onClose} onOk={() => void submit()} loading={busy} />}
    >
      <div role="radiogroup" aria-label={t('shareReportTitle')} className="flex flex-col gap-1">
        {SHARE_REPORT_REASONS.map((key) => (
          <label key={key} className="flex cursor-pointer items-center gap-2 py-0.5 text-sm text-foreground">
            <input type="radio" name="share-report-reason" className="accent-primary" checked={reason === key} onChange={() => setReason(key)} />
            {t(REASON_KEYS[key])}
          </label>
        ))}
      </div>
      <div className="flex flex-col gap-1.5">
        <FieldLabel>{t('shareReportDetail')}</FieldLabel>
        <Textarea rows={3} maxLength={DETAIL_MAX} value={detail} onChange={(event) => setDetail(event.target.value)} />
      </div>
      <div className="flex flex-col gap-1.5">
        <FieldLabel>{t('shareReportContact')}</FieldLabel>
        <Input maxLength={CONTACT_MAX} value={contact} onChange={(event) => setContact(event.target.value)} />
      </div>
      {error ? <FieldError>{error}</FieldError> : null}
    </ShareSurface>
  )
}
