// 创建邀请码：自定义 4-16 位字母数字，留空由服务端随机生成。

import { useEffect, useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { rpc } from '../../../../lib/rpc'
import { ConfirmFooter, Dialog, FieldError, Form, FormField, Input } from '../../../../ui'
import { normalizeReferralCode, validCreateCode } from '../account/billing'
import { accountErrorKey } from '../account/errorText'

export function CreateCodeDialog({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const t = useT()
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const [errorKey, setErrorKey] = useState<string | null>(null)
  const pending = useRef(false)
  const active = useRef(true)
  useEffect(() => {
    active.current = true
    return () => {
      active.current = false
    }
  }, [])
  const invalid = !validCreateCode(value)
  const close = () => {
    if (!pending.current) onClose()
  }

  const submit = async () => {
    if (pending.current || invalid) return
    pending.current = true
    setBusy(true)
    setErrorKey(null)
    try {
      const code = normalizeReferralCode(value)
      await rpc.agent.referral.createCode(code === '' ? undefined : { code })
      if (!active.current) return
      onCreated()
      onClose()
    } catch (error) {
      if (!active.current) return
      pending.current = false
      setErrorKey(accountErrorKey(error))
      setBusy(false)
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && close()}
      title={t('accountReferralCodeCreateTitle')}
      description={t('accountReferralCodeCreateDesc')}
      size="sm"
      modalLocked={busy}
      footer={<ConfirmFooter okLabel={t('accountReferralCodeCreate')} cancelLabel={busy ? null : t('cancel')} onCancel={close} onOk={() => void submit()} okDisabled={invalid} loading={busy} />}
    >
      <Form onSubmit={() => void submit()}>
        <FormField label={t('accountReferralCodeCreateTitle')} htmlFor="referral-code" {...(invalid ? { error: t('accountReferralCodeCreateInvalid') } : {})}>
          <Input
            id="referral-code"
            value={value}
            disabled={busy}
            maxLength={32}
            placeholder={t('accountReferralCodeCreatePlaceholder')}
            autoCapitalize="characters"
            spellCheck={false}
            className="font-mono uppercase"
            invalid={invalid}
            onChange={(event) => {
              setValue(event.target.value.toUpperCase())
              setErrorKey(null)
            }}
            autoFocus
          />
        </FormField>
        {errorKey ? <FieldError>{t(errorKey)}</FieldError> : null}
        <button type="submit" className="hidden" />
      </Form>
    </Dialog>
  )
}
