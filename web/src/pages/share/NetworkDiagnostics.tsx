// 「网络诊断」：endpoint ID、首选中继、NAT、公网地址与运营商、各中继延迟、传输会话、全量 iroh 指标、
// iroh tracing 日志（按级别与正则过滤）、原始快照 JSON（与上报 FluxCloud 的逐字相同）。
// 页签挂载期间才轮询；切走即停。

import { Copy, RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../../i18n'
import { cn } from '../../lib/cn'
import { copyText } from '../../lib/copy'
import { RpcError, rpc } from '../../lib/rpc'
import type { ShareLogLevel, ShareLogLineDto, ShareNetSnapshotDto } from '../../lib/rpc'
import { Badge, Button, Input, Select, toast } from '../../ui'
import type { SelectOption } from '../../ui'
import { accountErrorKey } from '../settings/sections/account/errorText'
import { NAT_KEYS } from './model'
import { SessionDetailView } from './SessionDetail'
import { useShareState } from './state'
import { useShareDiagnostics } from './useShareDiagnostics'

const LOG_LEVELS: readonly SelectOption<ShareLogLevel>[] = (['error', 'warn', 'info', 'debug', 'trace'] as const).map((level) => ({
  value: level,
  label: level.toUpperCase(),
}))
const LOG_LIMIT = 300
const LOG_DEBOUNCE_MS = 300

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5 rounded-md border border-hairline bg-chrome px-3 py-2">
      <div className="truncate text-xs text-text-tertiary">{label}</div>
      <div className="min-w-0 break-all text-sm text-foreground">{children}</div>
    </div>
  )
}

function Section({ title, trailing, children }: { title: string; trailing?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section className="flex min-w-0 flex-col gap-2">
      <div className="flex min-w-0 items-center justify-between gap-2">
        <h2 className="text-sm font-medium text-foreground">{title}</h2>
        {trailing}
      </div>
      {children}
    </section>
  )
}

function Overview({ snapshot }: { snapshot: ShareNetSnapshotDto }) {
  const t = useT()
  const { netStatus } = useShareState()
  const report = snapshot.netReport
  const relays = Object.entries(report?.relayLatencyMs ?? {}).sort((a, b) => a[1] - b[1])
  const carrier = [netStatus.province, netStatus.isp].filter((part): part is string => part !== null && part !== '').join(' ')
  const publicAddrs = [report?.globalV4, report?.globalV6].filter((addr): addr is string => addr !== undefined && addr !== '')
  return (
    <div className="grid grid-cols-1 gap-2 min-[640px]:grid-cols-2">
      <Fact label={t('shareNetEndpointId')}>
        <span className="font-mono text-xs">{snapshot.endpointId}</span>
      </Fact>
      <Fact label={t('shareNetHomeRelay')}>
        {snapshot.homeRelay ? (
          <span className="flex flex-wrap items-center gap-2">
            <span className="text-xs">{snapshot.homeRelay.url}</span>
            <Badge tone={snapshot.homeRelay.connected ? 'success' : 'warning'}>
              {t(snapshot.homeRelay.connected ? 'shareNetConnected' : 'shareNetNotConnected')}
            </Badge>
          </span>
        ) : (
          '—'
        )}
      </Fact>
      <Fact label={t('shareNetNat')}>{t(NAT_KEYS[report?.natType ?? 'unknown'])}</Fact>
      <Fact label={t('shareNetCarrier')}>{carrier === '' ? '—' : carrier}</Fact>
      <Fact label={t('shareNetGlobalAddrs')}>{publicAddrs.length === 0 ? '—' : publicAddrs.join(' · ')}</Fact>
      <Fact label={t('shareNetRelays')}>
        {relays.length === 0 ? (
          '—'
        ) : (
          <ul className="flex flex-col gap-0.5">
            {relays.map(([url, ms]) => (
              <li key={url} className="tabular flex min-w-0 justify-between gap-2 text-xs">
                <span className="min-w-0 truncate" title={url}>
                  {url}
                </span>
                <span className="shrink-0">{Math.round(ms)} ms</span>
              </li>
            ))}
          </ul>
        )}
      </Fact>
    </div>
  )
}

function Metrics({ metrics }: { metrics: Readonly<Record<string, unknown>> }) {
  const t = useT()
  const rows = useMemo(() => Object.entries(metrics).sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)), [metrics])
  if (rows.length === 0) return null
  return (
    <div className="max-h-80 min-w-0 overflow-auto rounded-md border border-hairline">
      <table className="w-full text-left text-xs">
        <thead className="sticky top-0 bg-chrome text-text-tertiary">
          <tr>
            <th className="px-2 py-1.5 font-normal">{t('shareNetMetricName')}</th>
            <th className="px-2 py-1.5 font-normal">{t('shareNetMetricValue')}</th>
          </tr>
        </thead>
        <tbody className="tabular">
          {rows.map(([name, value]) => (
            <tr key={name} className="border-t border-hairline">
              <td className="px-2 py-1 font-mono break-all">{name}</td>
              <td className="px-2 py-1">{typeof value === 'object' ? JSON.stringify(value) : String(value)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function Logs() {
  const t = useT()
  const [level, setLevel] = useState<ShareLogLevel>('info')
  const [regex, setRegex] = useState('')
  const [lines, setLines] = useState<readonly ShareLogLineDto[]>([])
  const [invalid, setInvalid] = useState(false)
  const [nonce, setNonce] = useState(0)

  useEffect(() => {
    let cancelled = false
    const timer = setTimeout(() => {
      const pattern = regex.trim()
      rpc.agent.share
        .logs({ level, limit: LOG_LIMIT, ...(pattern === '' ? {} : { regex: pattern }) })
        .then((result) => {
          if (cancelled) return
          setLines(result.lines)
          setInvalid(false)
        })
        .catch((error: unknown) => {
          if (cancelled) return
          if (error instanceof RpcError && error.field === 'regex') {
            setInvalid(true)
            return
          }
          toast.error(t(accountErrorKey(error)))
        })
    }, LOG_DEBOUNCE_MS)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [level, regex, nonce, t])

  return (
    <Section
      title={t('shareNetLogs')}
      trailing={
        <Button variant="ghost" icon={RefreshCw} onClick={() => setNonce((value) => value + 1)}>
          {t('shareLogsRefresh')}
        </Button>
      }
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-muted-foreground">{t('shareLogLevelLabel')}</span>
        <Select value={level} options={LOG_LEVELS} aria-label={t('shareLogLevelLabel')} className="w-28" onValueChange={setLevel} />
        <Input
          value={regex}
          invalid={invalid}
          placeholder={t('shareLogFilterPlaceholder')}
          aria-label={t('shareLogFilterPlaceholder')}
          spellCheck={false}
          className="min-w-0 flex-1 font-mono text-xs"
          onChange={(event) => setRegex(event.target.value)}
        />
      </div>
      {invalid ? <div className="text-xs text-destructive">{t('shareInvalidRegex')}</div> : null}
      {lines.length === 0 ? (
        <div className="py-3 text-center text-xs text-text-tertiary">{t('shareLogsEmpty')}</div>
      ) : (
        <ol className="max-h-80 overflow-auto rounded-md border border-hairline bg-chrome p-2 font-mono text-[11px] leading-relaxed">
          {lines.map((line, index) => (
            <li key={`${line.at}:${index}`} className="flex min-w-0 gap-2 whitespace-pre-wrap break-all">
              <span className="shrink-0 text-text-tertiary">{line.at.slice(11, 23)}</span>
              <span className="w-10 shrink-0 text-muted-foreground">{line.level.toUpperCase()}</span>
              <span className="shrink-0 text-text-tertiary">{line.target}</span>
              <span className="min-w-0 text-foreground">{line.message}</span>
            </li>
          ))}
        </ol>
      )}
    </Section>
  )
}

/** `focusSessionId`：从分享面板的「诊断此会话」进入时要聚焦的会话（滚动到视野中央并高亮）。 */
export function NetworkDiagnostics({ focusSessionId = null }: { focusSessionId?: string | null }) {
  const t = useT()
  const { data } = useShareDiagnostics({ intervalMs: 5000 })
  const snapshot = data?.snapshot ?? null
  const focusedRef = useRef(false)
  const focusPresent = focusSessionId !== null && (data?.sessions.some((session) => session.sessionId === focusSessionId) ?? false)
  useEffect(() => {
    if (!focusPresent || focusedRef.current) return
    focusedRef.current = true
    document.getElementById(`share-session-${focusSessionId}`)?.scrollIntoView({ block: 'center' })
  }, [focusPresent, focusSessionId])
  const copySnapshot = useCallback(() => {
    if (!data) return
    copyText(data.snapshotJson)
    toast.success(t('shareSnapshotCopied'))
  }, [data, t])

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <Section title={t('shareNetTitle')}>
        {snapshot ? (
          <Overview snapshot={snapshot} />
        ) : (
          <div className="rounded-md bg-chrome px-3 py-3 text-xs text-muted-foreground">{t('shareNetUnbound')}</div>
        )}
      </Section>

      {data && data.sessions.length > 0 ? (
        <Section title={t('shareNetSessions')}>
          <ul className="flex flex-col gap-3">
            {data.sessions.map((session) => (
              <li
                key={session.sessionId}
                id={`share-session-${session.sessionId}`}
                className={cn('rounded-md border p-3', session.sessionId === focusSessionId ? 'border-primary' : 'border-hairline')}
              >
                <SessionDetailView detail={session} />
              </li>
            ))}
          </ul>
        </Section>
      ) : null}

      {snapshot ? (
        <Section title={t('shareNetMetrics')}>
          <Metrics metrics={snapshot.metrics} />
        </Section>
      ) : null}

      <Logs />

      {data && data.snapshotJson !== '' ? (
        <Section
          title={t('shareNetSnapshot')}
          trailing={
            <Button variant="ghost" icon={Copy} onClick={copySnapshot}>
              {t('shareCopySnapshot')}
            </Button>
          }
        >
          <pre className="max-h-80 overflow-auto rounded-md border border-hairline bg-chrome p-2 font-mono text-[11px] whitespace-pre-wrap break-all">
            {data.snapshotJson}
          </pre>
        </Section>
      ) : null}
    </div>
  )
}
