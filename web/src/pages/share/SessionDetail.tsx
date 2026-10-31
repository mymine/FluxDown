// 传输会话诊断视图：路径时间线、各路径 RTT / 丢包 / cwnd / 速率、对端 NAT 与运营商。
// 分享面板的接收方行、任务详情「传输诊断」页、网络诊断页的会话列表共用；文案键与 GPUI 同源（`shareDiag*`）。

import { useT } from '../../i18n'
import { Badge } from '../../ui'
import type { BadgeTone } from '../../ui'
import type { SharePathKind, ShareSessionDetailDto } from '../../lib/rpc'
import { formatBytes } from '../downloads/dialogs/utils'
import { NAT_KEYS, PATH_EVENT_KEYS, PATH_KIND_KEYS, SESSION_ROLE_KEYS, bytesSplit, currentPathId, dateTimeLabel, rateLabel, servedFileLabel, sourceBytesRows } from './model'

const PATH_TONE: Record<SharePathKind, BadgeTone> = {
  directLan: 'success',
  directWan: 'success',
  relay: 'warning',
  cdn: 'success',
  bridge: 'neutral',
  swarm: 'success',
  unknown: 'neutral',
}

export function PathBadge({ kind }: { kind: SharePathKind }) {
  const t = useT()
  return <Badge tone={PATH_TONE[kind]}>{t(PATH_KIND_KEYS[kind])}</Badge>
}

const W = 320
const H = 56

/** 速率采样迷你面积图（无坐标轴；峰值标在右上）。 */
function RateSpark({ rates, label }: { rates: readonly number[]; label: string }) {
  if (rates.length < 2) return null
  const peak = Math.max(...rates, 1)
  const x = (index: number) => (index / (rates.length - 1)) * W
  const y = (value: number) => H - (Math.max(0, value) / peak) * (H - 4)
  const line = rates.map((value, index) => `${x(index).toFixed(1)},${y(value).toFixed(1)}`).join(' ')
  return (
    <div className="relative">
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="h-14 w-full text-primary" role="img" aria-label={label}>
        <polygon points={`0,${H} ${line} ${W},${H}`} fill="currentColor" fillOpacity={0.18} />
        <polyline points={line} fill="none" stroke="currentColor" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
      </svg>
      <span className="tabular pointer-events-none absolute top-0 right-1 text-[10px] leading-none text-text-tertiary">{rateLabel(peak)}</span>
    </div>
  )
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5 rounded-md border border-hairline bg-chrome px-3 py-2">
      <div className="truncate text-xs text-text-tertiary">{label}</div>
      <div className="tabular truncate text-sm font-medium text-foreground" title={value}>
        {value}
      </div>
    </div>
  )
}

const msText = (ms: number): string => `${Math.round(ms)} ms`

/** 单个会话的完整诊断。 */
export function SessionDetailView({ detail }: { detail: ShareSessionDetailDto }) {
  const t = useT()
  const { summary } = detail
  const split = bytesSplit(summary.bytes)
  const current = currentPathId(summary.events)
  const peerName = detail.peerDeviceName ?? ''
  const peerIsp = detail.peerIsp ?? ''
  const served = detail.swarmFiles ?? []
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <PathBadge kind={summary.pathKind} />
        <Badge>{t(SESSION_ROLE_KEYS[detail.role])}</Badge>
        <Badge tone={detail.ended ? 'neutral' : 'accent'}>{t(detail.ended ? 'shareNetSessionEnded' : 'shareNetSessionActive')}</Badge>
      </div>
      {summary.error ? (
        <div className="break-all text-xs text-destructive">
          {t('shareDiagError')}: {summary.error}
        </div>
      ) : null}

      <div className="grid grid-cols-2 gap-2 min-[560px]:grid-cols-3">
        <Fact label={t('shareDiagPeer')} value={peerName === '' ? t('shareSenderUnknown') : peerName} />
        <Fact label={t('shareDiagPeerNat')} value={t(NAT_KEYS[detail.peerNatType])} />
        <Fact label={t('shareDiagPeerCarrier')} value={peerIsp === '' ? '—' : peerIsp} />
        <Fact label={t('shareDiagStarted')} value={dateTimeLabel(summary.startedAt) || '—'} />
        <Fact label={t('shareDiagConnectMs')} value={msText(summary.connectMs)} />
        <Fact label={t('shareDiagFirstDirect')} value={summary.firstDirectMs === undefined ? t('shareDiagNotDirect') : msText(summary.firstDirectMs)} />
        <Fact
          label={t('shareDiagBytes')}
          value={sourceBytesRows(summary.bytes)
            .map((row) => `${t(row.labelKey)} ${formatBytes(row.bytes)}${row.kind === 'direct' && split.percent !== null ? ` (${split.percent}%)` : ''}`)
            .join(' · ')}
        />
        <Fact label={t('shareDiagVerifyFailures')} value={String(summary.verifyFailures)} />
      </div>

      <RateSpark rates={detail.samples.map((sample) => sample.rateBps)} label={t('infoSpeed')} />

      {served.length === 0 ? null : (
        <div className="flex min-w-0 flex-col gap-1">
          <div className="text-xs font-medium text-foreground">{t('shareDiagSwarmFiles')}</div>
          <ul className="flex flex-col gap-0.5 text-xs">
            {served.map((file) => (
              <li key={file.rootHash} className="flex min-w-0 items-baseline gap-3">
                <span className="min-w-0 flex-1 truncate text-foreground" title={file.fileName ?? file.rootHash}>
                  {servedFileLabel(file)}
                </span>
                <span className="tabular shrink-0 text-text-tertiary">
                  {formatBytes(file.bytes)} · {t('shareDiagSwarmChunks', { count: file.chunks })}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex min-w-0 flex-col gap-1">
        <div className="text-xs font-medium text-foreground">{t('shareDiagPaths')}</div>
        {summary.paths.length === 0 ? (
          <div className="text-xs text-text-tertiary">{t('shareDiagNoPaths')}</div>
        ) : (
          <div className="min-w-0 overflow-x-auto rounded-md border border-hairline">
            <table className="w-full min-w-[560px] text-left text-xs">
              <thead className="bg-chrome text-text-tertiary">
                <tr>
                  <th className="px-2 py-1.5 font-normal">{t('shareDiagColPath')}</th>
                  <th className="px-2 py-1.5 font-normal">{t('shareDiagColRtt')}</th>
                  <th className="px-2 py-1.5 font-normal">{t('shareDiagColLoss')}</th>
                  <th className="px-2 py-1.5 font-normal">{t('shareDiagColCwnd')}</th>
                  <th className="px-2 py-1.5 font-normal">{t('shareDiagColTraffic')}</th>
                  <th className="px-2 py-1.5 font-normal">{t('shareDiagColBandwidth')}</th>
                </tr>
              </thead>
              <tbody className="tabular">
                {summary.paths.map((path) => (
                  <tr key={path.id} className="border-t border-hairline align-top">
                    <td className="max-w-[220px] px-2 py-1.5">
                      <div className="flex flex-col gap-0.5">
                        <span className="flex flex-wrap items-center gap-1">
                          <PathBadge kind={path.kind} />
                          {path.id === current ? <Badge tone="accent">{t('shareDiagCurrentPath')}</Badge> : null}
                        </span>
                        <span className="truncate text-text-tertiary" title={path.remote}>
                          {path.remote}
                        </span>
                      </div>
                    </td>
                    <td className="px-2 py-1.5">{msText(path.rttMs)}</td>
                    <td className="px-2 py-1.5">
                      {path.lost} / {path.congestionEvents}
                    </td>
                    <td className="px-2 py-1.5">
                      {formatBytes(path.cwnd)} · MTU {path.mtu}
                    </td>
                    <td className="px-2 py-1.5">
                      ↑{formatBytes(path.sent)} ↓{formatBytes(path.recv)}
                    </td>
                    <td className="px-2 py-1.5">{path.bwEstimate > 0 ? `${formatBytes(path.bwEstimate / 8)}/s` : '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <div className="flex min-w-0 flex-col gap-1">
        <div className="text-xs font-medium text-foreground">{t('shareDiagTimeline')}</div>
        {summary.events.length === 0 ? (
          <div className="text-xs text-text-tertiary">{t('shareDiagNoEvents')}</div>
        ) : (
          <ol className="flex max-h-40 flex-col gap-0.5 overflow-y-auto text-xs text-muted-foreground">
            {summary.events.map((event, index) => (
              <li key={`${event.at}:${index}`} className="tabular flex min-w-0 gap-2">
                <span className="shrink-0 text-text-tertiary">{dateTimeLabel(event.at).slice(11) || event.at}</span>
                <span className="shrink-0">{PATH_EVENT_KEYS[event.kind] ? t(PATH_EVENT_KEYS[event.kind] as string) : event.kind}</span>
                <span className="min-w-0 truncate" title={event.path}>
                  {event.path}
                </span>
              </li>
            ))}
          </ol>
        )}
      </div>
    </div>
  )
}
