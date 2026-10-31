import { Check, ChevronDown, Globe } from 'lucide-react'
import { useRef, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { useT } from '../../../i18n'
import { enabledRuleCount } from '../../../lib/proxyRules'
import { Icon, Popover, Tooltip } from '../../../ui'
import { setDaemon, useDaemonValue } from '../../settings/kit/writeStore'
import { popoverSide, RAIL_BUTTON, resolveSide, useDaemonReady } from './widget'
import type { WidgetProps } from './widget'

const MODES = [
  { value: 'none', labelKey: 'proxyModeNone' },
  { value: 'system', labelKey: 'proxyModeSystem' },
  { value: 'manual', labelKey: 'proxyModeManual' },
  { value: 'auto', labelKey: 'proxyModeAuto' },
] as const

// 只展示服务器，不读取认证字段；URL 中夹带的 userinfo 也不能进入菜单或 tooltip。
function proxyEndpoint(host: string, port: string): string {
  const raw = host.trim()
  const parsedPort = Number(port)
  if (!raw || !Number.isInteger(parsedPort) || parsedPort <= 0 || parsedPort > 65535) return ''
  const bareIpv6 = !raw.includes('://') && !raw.includes('@') && !raw.startsWith('[') && raw.indexOf(':') !== raw.lastIndexOf(':')
  const server = bareIpv6 ? `[${raw}]` : raw
  try {
    const hostname = new URL(server.includes('://') ? server : `http://${server}`).hostname
    return hostname ? `${hostname}:${parsedPort}` : ''
  } catch {
    // 非地址文本不能作为可切换的手动代理端点。
    return ''
  }
}

function focusMenuItem(event: KeyboardEvent<HTMLDivElement>): void {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return
  const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'))
  if (items.length === 0) return
  const index = items.indexOf(document.activeElement as HTMLButtonElement)
  const next = event.key === 'Home'
    ? 0
    : event.key === 'End'
      ? items.length - 1
      : event.key === 'ArrowDown'
        ? (index + 1) % items.length
        : index <= 0 ? items.length - 1 : index - 1
  event.preventDefault()
  items[next]?.focus()
}

export function ProxyControl({ orientation = 'horizontal', side }: WidgetProps) {
  const t = useT()
  const disabled = !useDaemonReady()
  const mode = useDaemonValue('proxy_mode')
  const host = useDaemonValue('proxy_host')
  const port = useDaemonValue('proxy_port')
  const ruleCount = enabledRuleCount(useDaemonValue('proxy_rules'))
  const ruleHint = ruleCount > 0 ? t('proxyRulesActiveHint', { count: ruleCount }) : ''
  const current = MODES.find((item) => item.value === mode) ?? MODES[0]
  const title = t('settingsCatProxy')
  const label = t(current.labelKey)
  const summary = ruleHint ? `${title}: ${label} · ${ruleHint}` : `${title}: ${label}`
  const endpoint = proxyEndpoint(host, port)
  const [open, setOpen] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)

  const rail = orientation === 'rail'
  return (
    <Popover
      title={title}
      side={popoverSide(orientation)}
      align="end"
      open={open}
      onOpenChange={(next) => {
        if (!disabled || !next) setOpen(next)
      }}
      trigger={
        <Tooltip content={summary} side={resolveSide(orientation, side)}>
          <button
            ref={triggerRef}
            type="button"
            disabled={disabled}
            aria-label={summary}
            aria-haspopup="dialog"
            aria-expanded={open}
            className={
              rail
                ? RAIL_BUTTON
                : 'inline-flex h-status-control min-w-status-control shrink-0 items-center justify-center gap-1 whitespace-nowrap rounded-sm px-1.5 text-caption text-muted-foreground hover:bg-nav-hover hover:text-foreground disabled:pointer-events-none disabled:opacity-50 coarse:min-h-touch coarse:min-w-touch'
            }
          >
            <Icon icon={Globe} size={rail ? 'xl' : 'lg'} />
            {rail ? null : (
              <>
                <span className="max-w-28 truncate narrow:max-w-20">{label}</span>
                {ruleCount > 0 ? (
                  <span className="rounded-sm bg-muted px-1 text-caption tabular-nums text-muted-foreground">{ruleCount}</span>
                ) : null}
                <Icon icon={ChevronDown} />
              </>
            )}
          </button>
        </Tooltip>
      }
    >
      {(close) => (
        <div
          role="menu"
          aria-label={title}
          className="flex w-64 max-w-full flex-col p-1"
          onKeyDown={focusMenuItem}
        >
          {MODES.map((item) => (
            <button
              key={item.value}
              type="button"
              role="menuitemradio"
              aria-checked={item.value === current.value}
              disabled={disabled || (item.value === 'manual' && !endpoint)}
              onClick={() => {
                if (disabled || (item.value === 'manual' && !endpoint)) return
                close()
                triggerRef.current?.focus()
                setDaemon('proxy_mode', item.value)
              }}
              className="flex min-h-control w-full items-center gap-2 rounded-sm px-2 py-1 text-left text-sm hover:bg-nav-hover disabled:pointer-events-none disabled:opacity-50 coarse:min-h-touch"
            >
              <span className="inline-flex w-3.5 shrink-0 justify-center">{item.value === current.value ? <Icon icon={Check} /> : null}</span>
              <span className="min-w-0 flex-1">
                <span className="block truncate">{t(item.labelKey)}</span>
                {item.value === 'manual' ? (
                  <span className="block truncate text-caption text-muted-foreground" title={endpoint || t('proxyConfigureInSettings')}>
                    {endpoint || t('proxyConfigureInSettings')}
                  </span>
                ) : null}
              </span>
            </button>
          ))}
          {ruleCount > 0 ? (
            <div className="mt-1 border-t border-hairline px-2 pb-1 pt-1.5 text-caption text-muted-foreground">{ruleHint}</div>
          ) : null}
        </div>
      )}
    </Popover>
  )
}
