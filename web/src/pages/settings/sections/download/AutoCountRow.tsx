import { useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../../../../i18n'
import { daemonConfigField } from '../../../../lib/rpc'
import {
  DropdownField,
  NumberField,
  SettingsRow,
  optionalText,
  setDaemonNumber,
  useDaemonNumber,
  useSettingsReadOnly,
} from '../../kit'

export function AutoCountRow({
  configKey,
  titleKey,
  descKey,
  customDefault,
  footer,
}: {
  configKey: 'default_segments' | 'cdn_max_nodes'
  titleKey: string
  descKey: string
  customDefault: number
  footer?: (value: number) => ReactNode
}) {
  const t = useT()
  const value = useDaemonNumber(configKey)
  const readOnly = useSettingsReadOnly()
  const [lastCustom, setLastCustom] = useState(customDefault)
  const field = daemonConfigField(configKey)
  const title = t(titleKey)
  const auto = value === 0
  return (
    <SettingsRow title={title} description={optionalText(t, descKey)} footer={footer?.(value)}>
      <div className="flex w-full flex-wrap items-center gap-2 desktop:w-auto desktop:justify-end">
        <DropdownField
          value={auto ? 'auto' : 'custom'}
          options={[{ value: 'auto', label: t('auto') }, { value: 'custom', label: t('customThreads') }]}
          aria-label={title}
          disabled={readOnly}
          className="mobile:flex-1"
          onValueChange={(mode) => {
            if (readOnly || (mode === 'auto') === auto) return
            if (mode === 'auto') {
              setLastCustom(value)
              setDaemonNumber(configKey, 0)
            } else {
              setDaemonNumber(configKey, Math.min(field?.max ?? lastCustom, Math.max(1, lastCustom)))
            }
          }}
        />
        {!auto ? (
          <NumberField
            value={value}
            min={1}
            {...(field?.max !== undefined ? { max: field.max } : {})}
            aria-label={`${title} · ${t('customThreads')}`}
            disabled={readOnly}
            className="mobile:flex-1"
            onCommit={(next) => {
              if (!readOnly) setDaemonNumber(configKey, next)
            }}
          />
        ) : null}
      </div>
    </SettingsRow>
  )
}
