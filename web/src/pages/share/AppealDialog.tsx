// 申诉已下架的分享：填写说明后经 `agent.share.appeal` 提交（每个分享一次）。
// 与 GPUI `crates/share/src/appeal.rs` 对齐。

import { useState } from 'react'
import { useT } from '../../i18n'
import { rpc } from '../../lib/rpc'
import { ConfirmFooter, FieldError, FieldLabel, Textarea, toast } from '../../ui'
import { accountErrorText } from '../settings/sections/account/errorText'
import { ShareSurface } from './ShareSurface'

const STATEMENT_MAX = 1000

export function AppealDialog({ shareId, onClose }: { shareId: string; onClose: () => void }) {
  const t = useT()
  const [statement, setStatement] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const trimmed = statement.trim()

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      await rpc.agent.share.appeal({ shareId, statement: trimmed })
      toast.success(t('shareAppealSent'))
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
      title={t('shareAppealTitle')}
      persistent
      footer={
        <ConfirmFooter
          cancelLabel={t('cancel')}
          okLabel={t('shareReportSubmit')}
          onCancel={onClose}
          onOk={() => void submit()}
          okDisabled={trimmed === ''}
          loading={busy}
        />
      }
    >
      <div className="flex flex-col gap-1.5">
        <FieldLabel>{t('shareAppealStatement')}</FieldLabel>
        <Textarea rows={5} maxLength={STATEMENT_MAX} value={statement} onChange={(event) => setStatement(event.target.value)} />
      </div>
      {error ? <FieldError>{error}</FieldError> : null}
    </ShareSurface>
  )
}
