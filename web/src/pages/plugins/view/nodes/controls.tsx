import type { ViewNode } from '../../../../lib/rpc'
import { Button, Checkbox, Input, SegmentedTabs, Select, Switch, Textarea } from '../../../../ui'
import type { ButtonVariant } from '../../../../ui'
import { useViewRender } from './context'
import { bool, num, optStr, records, str } from './props'
import { useEchoValue } from './useEchoValue'

const VARIANTS: readonly ButtonVariant[] = ['primary', 'outline', 'ghost', 'danger']

export function ButtonNode({ node }: { node: ViewNode }) {
  const { send } = useViewRender()
  const variant = str(node.p, 'variant', 'outline')
  return (
    <Button
      variant={(VARIANTS as readonly string[]).includes(variant) ? (variant as ButtonVariant) : 'outline'}
      disabled={bool(node.p, 'disabled')}
      loading={bool(node.p, 'loading')}
      onClick={() => send(node.id, { type: 'press' })}
    >
      {str(node.p, 'text')}
    </Button>
  )
}

export function SwitchNode({ node }: { node: ViewNode }) {
  const { send } = useViewRender()
  const label = optStr(node.p, 'label')
  return (
    <label className="inline-flex items-center gap-2 text-sm text-foreground">
      <Switch
        checked={bool(node.p, 'checked')}
        disabled={bool(node.p, 'disabled')}
        aria-label={label}
        onCheckedChange={(value) => send(node.id, { type: 'change', value })}
      />
      {label ? <span>{label}</span> : null}
    </label>
  )
}

export function CheckboxNode({ node }: { node: ViewNode }) {
  const { send } = useViewRender()
  const label = optStr(node.p, 'label')
  return (
    <label className="inline-flex items-center gap-2 text-sm text-foreground">
      <Checkbox
        checked={bool(node.p, 'checked')}
        disabled={bool(node.p, 'disabled')}
        aria-label={label}
        onCheckedChange={(value) => send(node.id, { type: 'change', value })}
      />
      {label ? <span>{label}</span> : null}
    </label>
  )
}

export function SelectNode({ node }: { node: ViewNode }) {
  const { send } = useViewRender()
  const options = records(node.p, 'options').map((option) => ({ value: str(option, 'value'), label: str(option, 'label') }))
  return (
    <Select
      value={str(node.p, 'value')}
      options={options}
      placeholder={optStr(node.p, 'placeholder')}
      disabled={bool(node.p, 'disabled')}
      onValueChange={(value) => send(node.id, { type: 'change', value })}
    />
  )
}

export function InputNode({ node }: { node: ViewNode }) {
  const { send } = useViewRender()
  const echo = useEchoValue(str(node.p, 'value'), (value) => send(node.id, { type: 'change', value }))
  const kind = str(node.p, 'kind', 'text')
  return (
    <Input
      type={kind === 'password' || kind === 'number' ? kind : 'text'}
      value={echo.draft}
      placeholder={optStr(node.p, 'placeholder')}
      disabled={bool(node.p, 'disabled')}
      onChange={(event) => echo.onChange(event.target.value)}
      onFocus={echo.onFocus}
      onBlur={echo.onBlur}
      onKeyDown={(event) => {
        if (event.key !== 'Enter' || event.nativeEvent.isComposing) return
        echo.flush()
        send(node.id, { type: 'submit', value: echo.draft })
      }}
    />
  )
}

export function TextareaNode({ node }: { node: ViewNode }) {
  const { send } = useViewRender()
  const echo = useEchoValue(str(node.p, 'value'), (value) => send(node.id, { type: 'change', value }))
  return (
    <Textarea
      value={echo.draft}
      rows={num(node.p, 'rows') ?? 4}
      placeholder={optStr(node.p, 'placeholder')}
      disabled={bool(node.p, 'disabled')}
      onChange={(event) => echo.onChange(event.target.value)}
      onFocus={echo.onFocus}
      onBlur={echo.onBlur}
    />
  )
}

export function TabsNode({ node }: { node: ViewNode }) {
  const { send } = useViewRender()
  const items = records(node.p, 'items').map((item) => ({ value: str(item, 'value'), label: str(item, 'label') }))
  return <SegmentedTabs items={items} value={str(node.p, 'value')} onValueChange={(value) => send(node.id, { type: 'change', value })} />
}
