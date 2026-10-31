import { Info } from 'lucide-react'
import { Icon } from '../../../../ui'
import { SettingsCustomRow } from '../../kit'

/** info 图标 + 说明文字的整行。 */
export function InfoLine({ text }: { text: string }) {
  return (
    <SettingsCustomRow className="flex items-start gap-2">
      <Icon icon={Info} size="md" className="mt-0.5 shrink-0 text-text-tertiary" />
      <span className="min-w-0 flex-1 text-xs text-muted-foreground">{text}</span>
    </SettingsCustomRow>
  )
}
