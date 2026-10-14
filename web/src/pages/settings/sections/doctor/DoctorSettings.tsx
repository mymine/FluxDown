// 环境诊断（GPUI `crates/settings/src/sections/doctor.rs`）：运行 agent.diagnostics.run，展示检查项与就地修复。
// 桌面专属的检查项（浏览器 NMH 注册与拉起、浏览器策略、协议/文件关联、开机自启、系统通知、以 root 运行）与需要
// 桌面会话的修复（重新注册、打开日志目录、请求管理员授权、打开系统设置、测试通知）在 Web 隐藏。

import { useState } from 'react'
import { useT } from '../../../../i18n'
import type { TFunction } from '../../../../i18n'
import { copyText } from '../../../../lib/copy'
import { rpc } from '../../../../lib/rpc'
import type { DaemonDiagnosticsDescribe, DiagnosticCheckDto, DiagnosticLevel, DiagnosticsReportDto, ErrorReason } from '../../../../lib/rpc'
import { RpcError } from '../../../../lib/rpc'
import { rpcErrorText } from '../../../../lib/rpcErrorText'
import { Button, toast } from '../../../../ui'
import { SettingsCustomRow, SettingsPage, SettingsSection, camel, useSettingsReadOnly } from '../../kit'

const DESKTOP_ONLY_CHECKS: ReadonlySet<string> = new Set([
  'nmh_binary',
  'nmh_manifest',
  'nmh_browser',
  'nmh_relay',
  'nmh_launch',
  'nmh_policy',
  'nmh_comspec',
  'nmh_ownership',
  'url_protocol',
  'torrent_association',
  'autostart',
  'notifications',
  'elevated_run',
])
const DESKTOP_ONLY_ACTIONS: ReadonlySet<string> = new Set([
  'reregister',
  'use_this_install',
  'register',
  'open_log_dir',
  'openLogDir',
  'test_notification',
  'fix_dir_access',
  'enable_autostart',
  'open_settings',
  'fix_comspec',
])

/** 修复失败的可操作说明（同 GPUI `doctor::repair_outcome`）；其余错误回退通用文案。 */
const REPAIR_REASON_KEYS: Partial<Record<ErrorReason, string>> = {
  elevationCancelled: 'doctorRepairCancelled',
  elevationUnavailable: 'doctorRepairUnavailable',
  runningElevated: 'doctorRepairRunningElevated',
  repairIncomplete: 'doctorRepairIncomplete',
  repairNotApplicable: 'doctorRepairNotApplicable',
}

function repairErrorText(error: unknown, t: TFunction): string {
  const reason = error instanceof RpcError ? error.reason : undefined
  const key = reason ? REPAIR_REASON_KEYS[reason] : undefined
  return key ? t(key) : t('doctorRepairFailed', { error: rpcErrorText(error, t) })
}

const LEVEL_KEYS: Record<DiagnosticLevel, string> = {
  ok: 'doctorLevelOk',
  warn: 'doctorLevelWarn',
  error: 'doctorLevelError',
  info: 'doctorLevelInfo',
}
const LEVEL_TEXT: Record<DiagnosticLevel, string> = {
  ok: 'text-success',
  warn: 'text-warning',
  error: 'text-destructive',
  info: 'text-muted-foreground',
}

interface Loaded {
  report: DiagnosticsReportDto
  checks: DiagnosticCheckDto[]
  env: DaemonDiagnosticsDescribe | null
}

function checkTitle(t: TFunction, check: DiagnosticCheckDto): string {
  const label = t(`doctorCheck${camel(check.id)}`)
  return check.target ? `${label} · ${check.target}` : label
}

/** 纯文本报告（复制到反馈）。 */
function renderReport(t: TFunction, loaded: Loaded): string {
  const { report, checks } = loaded
  let out = `FluxDown ${report.appVersion} · ${report.platform} · ${report.generatedAtUnixMs}\nagent data dir: ${report.agentDataDir}\ndaemon connected: ${report.daemonConnected}\n`
  if (loaded.env) out += `daemon: ${loaded.env.service.serviceVersion} · tasks ${loaded.env.tasks} · queues ${loaded.env.queues} · groups ${loaded.env.groups} · log dir ${loaded.env.logDir}\n`
  out += '\n'
  for (const check of checks) {
    out += `[${check.level.toUpperCase()}] ${t(`doctorCheck${camel(check.id)}`)}`
    if (check.target) out += ` (${check.target})`
    out += `: ${check.detail}\n`
    if (check.hint) out += `  hint: ${check.hint}\n`
  }
  return out
}

function CheckRow({ check, busy, onRepair, repairing }: { check: DiagnosticCheckDto; busy: boolean; onRepair: () => void; repairing: boolean }) {
  const t = useT()
  const repair = check.repair && !DESKTOP_ONLY_ACTIONS.has(check.repair.action) ? check.repair : null
  return (
    <SettingsCustomRow className="flex flex-wrap items-start gap-x-3 gap-y-2">
      <div className={`w-16 shrink-0 text-xs font-medium ${LEVEL_TEXT[check.level]}`}>{t(LEVEL_KEYS[check.level])}</div>
      <div className="flex min-w-0 flex-1 basis-56 flex-col gap-0.5">
        <div className="text-sm text-foreground">{checkTitle(t, check)}</div>
        {check.detail ? <div className="break-words text-xs text-muted-foreground">{check.detail}</div> : null}
        {check.hint ? <div className="text-xs text-foreground">{t(`doctorHint${camel(check.hint)}`)}</div> : null}
      </div>
      {repair ? (
        <Button loading={repairing} disabled={busy} onClick={onRepair}>
          {t(`doctorAction${camel(repair.action)}`)}
        </Button>
      ) : null}
    </SettingsCustomRow>
  )
}

export function DoctorSettings() {
  const t = useT()
  const disabled = useSettingsReadOnly()
  const [loaded, setLoaded] = useState<Loaded | null>(null)
  const [running, setRunning] = useState(false)
  const [repairingTag, setRepairingTag] = useState<string | null>(null)

  const run = async () => {
    if (running) return
    setRunning(true)
    try {
      const [report, env] = await Promise.all([rpc.agent.diagnostics.run(), rpc.daemon.diagnostics.describe().catch(() => null)])
      const checks = report.checks.filter((check) => !DESKTOP_ONLY_CHECKS.has(check.id))
      setLoaded({ report, checks, env })
      const issues = checks.filter((check) => check.level === 'warn' || check.level === 'error').length
      if (issues === 0) toast.key('doctorRunDoneHealthy', 'success')
      else toast.key('doctorRunDoneIssues', 'warning', { n: issues })
    } catch (error) {
      toast.error(rpcErrorText(error, t))
    } finally {
      setRunning(false)
    }
  }

  /** `tag` 带行序：同名队列 / RSS 源 / 分类会产生相同的 id·target。修复失败也重新诊断（可能只完成了一部分）。 */
  const repair = async (check: DiagnosticCheckDto, tag: string) => {
    if (!check.repair || repairingTag !== null) return
    setRepairingTag(tag)
    try {
      await rpc.agent.diagnostics.repair(check.repair)
    } catch (error) {
      toast.error(repairErrorText(error, t))
    }
    try {
      const report = await rpc.agent.diagnostics.run()
      setLoaded((prev) => ({ report, checks: report.checks.filter((item) => !DESKTOP_ONLY_CHECKS.has(item.id)), env: prev?.env ?? null }))
    } catch (error) {
      toast.error(rpcErrorText(error, t))
    } finally {
      setRepairingTag(null)
    }
  }

  const issues = loaded?.checks.filter((check) => check.level === 'warn' || check.level === 'error').length ?? 0
  const summary = !loaded ? t('doctorNeverRun') : issues === 0 ? t('doctorAllHealthy') : t('doctorIssuesFound', { n: issues })
  const busy = disabled || running || repairingTag !== null

  return (
    <SettingsPage title={t('settingsCatDoctor')} description={t('settingsCatDoctorDesc')}>
      <SettingsSection title={t('doctorTitle')} subtitle={t('doctorDesc')}>
        <SettingsCustomRow className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0 text-sm text-muted-foreground">
            <div>{summary}</div>
            {loaded ? <div className="text-xs text-text-tertiary">{t('doctorLastRun', { time: new Date(loaded.report.generatedAtUnixMs).toLocaleString() })}</div> : null}
          </div>
          <div className="flex gap-2">
            <Button
              disabled={disabled || !loaded}
              onClick={() => {
                if (!loaded) return
                copyText(renderReport(t, loaded))
                toast.key('doctorCopied', 'success')
              }}
            >
              {t('doctorCopyReport')}
            </Button>
            <Button variant="primary" loading={running} disabled={busy} onClick={() => void run()}>
              {running ? t('doctorRunning') : t('doctorRun')}
            </Button>
          </div>
        </SettingsCustomRow>
      </SettingsSection>
      {loaded ? (
        <>
          <SettingsSection>
            {loaded.checks.map((check, index) => {
              const tag = `${index}-${check.id}-${check.target}`
              return <CheckRow key={tag} check={check} busy={busy} repairing={repairingTag === tag} onRepair={() => void repair(check, tag)} />
            })}
          </SettingsSection>
          <SettingsSection title={t('doctorEnvTitle')}>
            <SettingsCustomRow>
              <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 font-mono text-xs">
                <dt className="text-text-tertiary">version</dt>
                <dd className="break-all text-foreground">{loaded.report.appVersion}</dd>
                <dt className="text-text-tertiary">platform</dt>
                <dd className="break-all text-foreground">{loaded.report.platform}</dd>
                <dt className="text-text-tertiary">agent data</dt>
                <dd className="break-all text-foreground">{loaded.report.agentDataDir}</dd>
                {loaded.env ? (
                  <>
                    <dt className="text-text-tertiary">daemon</dt>
                    <dd className="break-all text-foreground">
                      {loaded.env.service.serviceVersion} · tasks {loaded.env.tasks} · queues {loaded.env.queues} · groups {loaded.env.groups}
                    </dd>
                    <dt className="text-text-tertiary">log dir</dt>
                    <dd className="break-all text-foreground">{loaded.env.logDir}</dd>
                  </>
                ) : null}
              </dl>
            </SettingsCustomRow>
          </SettingsSection>
        </>
      ) : null}
    </SettingsPage>
  )
}
