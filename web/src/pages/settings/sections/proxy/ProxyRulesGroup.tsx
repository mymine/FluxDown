// 站点代理规则分组（crates/settings/src/sections/proxy_rules.rs）。
// 排序：GPUI 用拖拽抓手；Web 用上移 / 下移按钮（键盘 / 触屏都可用）。

import { ArrowDown, ArrowUp, Ellipsis, Network } from 'lucide-react'
import { useMemo, useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { cn } from '../../../../lib/cn'
import { rpc } from '../../../../lib/rpc'
import { rpcErrorText } from '../../../../lib/rpcErrorText'
import {
  displayEndpoint,
  manualEndpoint,
  moveRule,
  parseProxyRules,
  removeRule,
  routeLines,
  setRuleEnabled,
} from '../../../../lib/proxyRules'
import type { ProxyRule } from '../../../../lib/proxyRules'
import type { ProxyRouteResponse } from '../../../../lib/rpc'
import {
  ActionMenu,
  Badge,
  Button,
  EmptyState,
  Icon,
  Input,
  InputWithAction,
  Switch,
  Tooltip,
  confirmDialog,
  toast,
  useIsMobile,
} from '../../../../ui'
import type { MenuEntry } from '../../../../ui'
import { useDaemonReady } from '../../../downloads/statusbar/widget'
import { SettingsCustomRow, SettingsRow, SettingsSection, useDaemonValue } from '../../kit'
import { InfoLine } from './InfoLine'
import { ProxyRuleDialog } from './ProxyRuleDialog'

const ACTION_LABEL = {
  direct: 'proxyRuleActionDirect',
  system: 'proxyRuleActionSystem',
  manual: 'proxyRuleActionManual',
} as const

type RouteOutcome = { stamp: string; response: ProxyRouteResponse } | { stamp: string; error: string }

interface EditorState {
  /** 递增：强制对话框重挂载以重置草稿。 */
  serial: number
  existing: ProxyRule | null
}

/** 把端点切出来用等宽字体显示。 */
function WithEndpoint({ text, endpoint }: { text: string; endpoint: string }) {
  const at = endpoint === '' ? -1 : text.indexOf(endpoint)
  if (at < 0) return <>{text}</>
  return (
    <>
      {text.slice(0, at)}
      <span className="font-mono">{endpoint}</span>
      {text.slice(at + endpoint.length)}
    </>
  )
}

function RuleRow({
  rule,
  index,
  count,
  manualMissing,
  disabled,
  onMove,
  onEdit,
  onDelete,
  onToggle,
}: {
  rule: ProxyRule
  index: number
  count: number
  manualMissing: boolean
  disabled: boolean
  onMove: (delta: -1 | 1) => void
  onEdit: () => void
  onDelete: () => void
  onToggle: (enabled: boolean) => void
}) {
  const t = useT()
  const mobile = useIsMobile()
  const title = rule.name || rule.pattern
  const actionLabel =
    rule.action === 'custom' ? `${t('proxyRuleActionCustom')} · ${displayEndpoint(rule.proxy)}` : t(ACTION_LABEL[rule.action])
  const first = index === 0
  const last = index === count - 1

  const entries: MenuEntry[] = [
    { type: 'item', key: 'up', label: t('moveUpAction'), onSelect: () => onMove(-1), disabled: disabled || first },
    { type: 'item', key: 'down', label: t('moveDownAction'), onSelect: () => onMove(1), disabled: disabled || last },
    { type: 'item', key: 'edit', label: t('proxyRuleEdit'), onSelect: onEdit, disabled },
    { type: 'separator', key: 'sep' },
    { type: 'item', key: 'delete', label: t('delete'), onSelect: onDelete, destructive: true, disabled },
  ]

  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded-md px-2 py-1.5 hover:bg-row-hover">
      <Tooltip content={t('proxyRuleOrder', { index: index + 1 })}>
        <Badge className="shrink-0 font-mono tabular-nums">{index + 1}</Badge>
      </Tooltip>
      <Switch checked={rule.enabled} disabled={disabled} aria-label={t('proxyRuleEnabled')} onCheckedChange={onToggle} />
      <div className="flex min-w-0 flex-1 basis-40 flex-col gap-0.5">
        <div className={cn('truncate text-sm', rule.enabled ? 'text-foreground' : 'text-text-tertiary')}>{title}</div>
        <div className={cn('flex min-w-0 items-center gap-2 text-xs', rule.enabled ? 'text-muted-foreground' : 'text-text-tertiary')}>
          <Badge className="shrink-0">{t(rule.match === 'regex' ? 'proxyRuleMatchRegexShort' : 'proxyRuleMatchDomain')}</Badge>
          <span className="min-w-0 truncate font-mono">{rule.pattern}</span>
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <Badge tone={rule.action === 'direct' ? 'neutral' : 'accent'} className="max-w-48 truncate">
          {actionLabel}
        </Badge>
        {rule.action === 'manual' && manualMissing ? <Badge tone="warning">{t('proxyNotConfigured')}</Badge> : null}
        {mobile ? (
          <ActionMenu
            title={title}
            entries={entries}
            trigger={
              <Button variant="ghost" iconOnly aria-label={t('moreActions')} title={t('moreActions')} className="text-muted-foreground hover:text-foreground">
                <Icon icon={Ellipsis} />
              </Button>
            }
          />
        ) : (
          <>
            <Tooltip content={t('moveUpAction')}>
              <Button variant="ghost" iconOnly className="text-muted-foreground hover:text-foreground" aria-label={t('moveUpAction')} disabled={disabled || first} onClick={() => onMove(-1)}>
                <Icon icon={ArrowUp} size="md" />
              </Button>
            </Tooltip>
            <Tooltip content={t('moveDownAction')}>
              <Button variant="ghost" iconOnly className="text-muted-foreground hover:text-foreground" aria-label={t('moveDownAction')} disabled={disabled || last} onClick={() => onMove(1)}>
                <Icon icon={ArrowDown} size="md" />
              </Button>
            </Tooltip>
            <Button variant="outline" disabled={disabled} onClick={onEdit}>
              {t('proxyRuleEdit')}
            </Button>
            <Button variant="ghost" className="text-destructive" disabled={disabled} onClick={onDelete}>
              {t('delete')}
            </Button>
          </>
        )}
      </div>
    </div>
  )
}

export function ProxyRulesGroup() {
  const t = useT()
  const ready = useDaemonReady()
  const disabled = !ready
  const raw = useDaemonValue('proxy_rules')
  const mode = useDaemonValue('proxy_mode')
  const proxyType = useDaemonValue('proxy_type')
  const proxyHost = useDaemonValue('proxy_host')
  const proxyPort = useDaemonValue('proxy_port')
  const list = useMemo(() => parseProxyRules(raw), [raw])
  const manualMissing = manualEndpoint(proxyType, proxyHost, proxyPort) === ''

  const [editor, setEditor] = useState<EditorState | null>(null)
  const serial = useRef(0)
  const [url, setUrl] = useState('')
  const [busy, setBusy] = useState(false)
  const [outcome, setOutcome] = useState<RouteOutcome | null>(null)
  const generation = useRef(0)

  const write = async (operation: () => Promise<void>) => {
    try {
      await operation()
    } catch (error) {
      toast.error(rpcErrorText(error, t))
    }
  }

  const openEditor = (existing: ProxyRule | null) => {
    serial.current += 1
    setEditor({ serial: serial.current, existing })
  }

  const confirmDelete = async (rule: ProxyRule) => {
    const ok = await confirmDialog({
      title: t('proxyRuleDeleteConfirm', { name: rule.name || rule.pattern }),
      description: t('proxyRuleDeleteDesc'),
      intent: 'destructive',
      okLabel: t('delete'),
    })
    if (ok) await write(() => removeRule(rule.id))
  }

  // 结果只在 stamp 与当前（模式 + 规则原文）一致时显示：改了规则或模式后旧结果自动隐藏。
  const stamp = `${mode}|${raw}`
  const canCheck = ready && !busy && url.trim() !== ''
  const check = async () => {
    const target = url.trim()
    if (!canCheck || target === '') return
    const sent = stamp
    const mine = ++generation.current
    setBusy(true)
    try {
      const response = await rpc.daemon.config.proxyRoute({ url: target, rules: raw })
      if (mine === generation.current) setOutcome({ stamp: sent, response })
    } catch (error) {
      if (mine === generation.current) setOutcome({ stamp: sent, error: rpcErrorText(error, t) })
    } finally {
      if (mine === generation.current) setBusy(false)
    }
  }

  const shown = outcome && outcome.stamp === stamp ? outcome : null
  const lines = shown && 'response' in shown ? routeLines(t, shown.response) : null
  const endpoint = shown && 'response' in shown ? shown.response.endpoint : ''

  const result = shown ? (
    <div className="flex flex-col gap-0.5 text-xs" aria-live="polite">
      {'error' in shown ? (
        <div className="text-destructive">{shown.error}</div>
      ) : lines ? (
        <>
          {lines.title ? (
            <div className={lines.title.tone === 'accent' ? 'text-accent-text' : 'text-foreground'}>{lines.title.text}</div>
          ) : null}
          <div className={lines.detail.tone === 'warning' ? 'text-warning' : 'text-muted-foreground'}>
            <WithEndpoint text={lines.detail.text} endpoint={endpoint} />
          </div>
        </>
      ) : null}
    </div>
  ) : undefined

  return (
    <SettingsSection title={t('proxyRulesTitle')} subtitle={t('proxyRulesDesc')}>
      <SettingsCustomRow className="flex flex-col gap-1 px-2 py-2">
        {list.length === 0 ? (
          <EmptyState icon={Network} title={t('proxyRulesEmptyTitle')} description={t('proxyRulesEmptyDesc')} />
        ) : (
          list.map((rule, index) => (
            <RuleRow
              key={rule.id || `idx-${index}`}
              rule={rule}
              index={index}
              count={list.length}
              manualMissing={manualMissing}
              disabled={disabled}
              onMove={(delta) => void write(() => moveRule(rule.id, delta))}
              onEdit={() => openEditor(rule)}
              onDelete={() => void confirmDelete(rule)}
              onToggle={(enabled) => void write(() => setRuleEnabled(rule.id, enabled))}
            />
          ))
        )}
        <div className="flex flex-wrap justify-end gap-2 px-2 pt-2 narrow:[&>*]:flex-1">
          <Button variant="primary" disabled={disabled} onClick={() => openEditor(null)}>
            {t('proxyRulesAdd')}
          </Button>
        </div>
      </SettingsCustomRow>
      <SettingsRow
        title={t('proxyRouteTest')}
        description={t('proxyRouteTestDesc')}
        vertical
        {...(result ? { footer: <div className="px-4 pb-3">{result}</div> } : {})}
      >
        <InputWithAction
          input={
            <Input
              value={url}
              inputMode="url"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              aria-label={t('proxyRouteTest')}
              placeholder={t('proxyRouteUrlPlaceholder')}
              disabled={disabled}
              onChange={(event) => setUrl(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') void check()
              }}
            />
          }
          action={
            <Button loading={busy} disabled={!canCheck} onClick={() => void check()}>
              {t(busy ? 'proxyRouteChecking' : 'proxyRouteCheck')}
            </Button>
          }
        />
      </SettingsRow>
      <InfoLine text={t('proxyRulesScope')} />
      {editor ? (
        <ProxyRuleDialog key={editor.serial} existing={editor.existing} list={list} onClose={() => setEditor(null)} />
      ) : null}
    </SettingsSection>
  )
}
