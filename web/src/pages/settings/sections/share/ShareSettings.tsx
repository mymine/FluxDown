// FluxMesh设置（GPUI `crates/share` 的设置页内容；分类位置 = `build_pages` 里 notify 之后、扩展之前）。
// 全部字段来自 `ShareLocalConfigDto`，经 `agent.share.setLocalConfig` 整体替换写回（见 `localConfig.ts`）。
// UDP 端口 / 连接模式 / portmapper / 额外公布地址变化会让 agent 重建端点。

import { useState } from 'react'
import { useT } from '../../../../i18n'
import type { ShareConnectionMode, ShareExitBehavior, ShareLogLevel } from '../../../../lib/rpc'
import { Switch } from '../../../../ui'
import type { SelectOption } from '../../../../ui'
import { claimOptions, tierLabel } from '../../../share/model'
import { takeDiagnosticsRequest } from '../../../share/diagnosticsRequest'
import { NetworkDiagnostics } from '../../../share/NetworkDiagnostics'
import { useShareCapability, useShareState } from '../../../share/state'
import { DropdownField, NumberField, SettingsPage, SettingsRow, SettingsSection, TextField } from '../../kit'
import { formatBytes } from '../../../downloads/dialogs/utils'
import { formatDailyCap, formatAddrList, parseAddrList, parseDailyCap, parsePort, portmapperChoice, portmapperWire, rateFromWire, rateToWire, saveLocalConfig } from './localConfig'
import type { PortmapperChoice } from './localConfig'

const MODES: readonly ShareConnectionMode[] = ['auto', 'relayOnly', 'directOnly']
const MODE_KEYS: Readonly<Record<ShareConnectionMode, string>> = {
  auto: 'shareSetModeAuto',
  relayOnly: 'shareSetModeRelayOnly',
  directOnly: 'shareSetModeDirectOnly',
}
const EXITS: readonly ShareExitBehavior[] = ['prompt', 'convertToCloud', 'quit']
const EXIT_KEYS: Readonly<Record<ShareExitBehavior, string>> = {
  prompt: 'shareSetExitPrompt',
  convertToCloud: 'shareSetExitConvertToCloud',
  quit: 'shareSetExitQuit',
}
const LOG_LEVELS: readonly ShareLogLevel[] = ['error', 'warn', 'info', 'debug', 'trace']
const LOG_KEYS: Readonly<Record<ShareLogLevel, string>> = {
  error: 'shareLogLevelError',
  warn: 'shareLogLevelWarn',
  info: 'shareLogLevelInfo',
  debug: 'shareLogLevelDebug',
  trace: 'shareLogLevelTrace',
}
const AUTO_RELAY = ''

export function ShareSettings() {
  const t = useT()
  const { localConfig: config, policySummary, available: serviceAvailable } = useShareState()
  const capability = useShareCapability()
  const available = capability && serviceAvailable
  // 分享面板「诊断此会话」带来的请求：直接落在网络诊断页签并聚焦该会话。
  const [focus] = useState(() => takeDiagnosticsRequest())

  const tiers = policySummary.liveTiers
  const tierOptions: SelectOption<string>[] = tiers.map((tier) => ({ value: tier.id, label: tierLabel(t, tier) }))
  const tierValue = config.defaultTierId ?? tiers[0]?.id ?? ''
  const claimChoices: SelectOption<string>[] = claimOptions(config.defaultMaxClaims).map((count) => ({
    value: String(count),
    label: count === 0 ? t('shareClaimsUnlimited') : t('shareClaimsTimes', { count }),
  }))
  const modeOptions: SelectOption<ShareConnectionMode>[] = MODES.map((mode) => ({ value: mode, label: t(MODE_KEYS[mode]) }))
  const relayOptions: SelectOption<string>[] = [
    { value: AUTO_RELAY, label: t('shareSetRelayAuto') },
    ...policySummary.relays.map((relay) => ({ value: relay.url, label: relay.region === '' ? relay.url : `${relay.region} · ${relay.url}` })),
  ]
  // 固定的中继已不在云端列表里：保留当前值，避免下拉显示成「自动」而静默改写。
  if (config.pinnedRelay !== null && !relayOptions.some((option) => option.value === config.pinnedRelay)) {
    relayOptions.push({ value: config.pinnedRelay, label: config.pinnedRelay })
  }
  const exitOptions: SelectOption<ShareExitBehavior>[] = EXITS.map((exit) => ({ value: exit, label: t(EXIT_KEYS[exit]) }))
  const portmapperOptions: SelectOption<PortmapperChoice>[] = [
    { value: 'auto', label: t('shareSetPortAuto') },
    { value: 'on', label: t('shareNotifyOn') },
    { value: 'off', label: t('shareNotifyOff') },
  ]
  const logOptions: SelectOption<ShareLogLevel>[] = LOG_LEVELS.map((level) => ({ value: level, label: t(LOG_KEYS[level]) }))
  const defaultCap = policySummary.provideDailyCapDefaultBytes ?? 0

  const general = (
    <>
      {capability ? (
        <SettingsSection>
          <SettingsRow title={t('shareSetEnabled')} description={t('shareSetEnabledDesc')} compact>
            <Switch
              checked={config.enabled}
              aria-label={t('shareSetEnabled')}
              onCheckedChange={(enabled) => void saveLocalConfig({ enabled })}
            />
          </SettingsRow>
        </SettingsSection>
      ) : (
        <div role="status" className="rounded-md border border-hairline bg-nav-hover px-3 py-2 text-xs text-muted-foreground">
          {t('errReasonShareUnavailable')}
        </div>
      )}

      {capability && config.enabled && !available ? (
        <div role="status" className="rounded-md border border-hairline bg-nav-hover px-3 py-2 text-xs text-muted-foreground">
          {t('errReasonShareUnavailable')}
        </div>
      ) : null}

      {capability && config.enabled ? (
        <>

      <SettingsSection title={t('shareSetGroupDefaults')}>
        <SettingsRow title={t('shareSetDefaultTier')} description={t('shareSetDefaultTierDesc')}>
          <DropdownField
            value={tierValue}
            options={tierOptions}
            disabled={tierOptions.length === 0}
            aria-label={t('shareSetDefaultTier')}
            onValueChange={(defaultTierId) => void saveLocalConfig({ defaultTierId })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetDefaultClaims')} description={t('shareSetDefaultClaimsDesc')}>
          <DropdownField
            value={String(config.defaultMaxClaims)}
            options={claimChoices}
            aria-label={t('shareSetDefaultClaims')}
            onValueChange={(value) => void saveLocalConfig({ defaultMaxClaims: Number(value) })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetDefaultNotify')} description={t('shareSetDefaultNotifyDesc')} compact>
          <Switch
            checked={config.defaultNotifyOnClaim}
            aria-label={t('shareSetDefaultNotify')}
            onCheckedChange={(defaultNotifyOnClaim) => void saveLocalConfig({ defaultNotifyOnClaim })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetDefaultSwarm')} description={t('shareSetDefaultSwarmDesc')} compact>
          <Switch
            checked={config.defaultSwarm !== false}
            aria-label={t('shareSetDefaultSwarm')}
            onCheckedChange={(on) => void saveLocalConfig({ defaultSwarm: on ? null : false })}
          />
        </SettingsRow>
      </SettingsSection>

      <SettingsSection title={t('shareSetGroupNetwork')}>
        <SettingsRow title={t('shareSetMode')} description={t('shareSetModeDesc')}>
          <DropdownField
            value={config.mode}
            options={modeOptions}
            aria-label={t('shareSetMode')}
            onValueChange={(mode) => void saveLocalConfig({ mode })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetRelay')} description={t('shareSetRelayDesc')}>
          <DropdownField
            value={config.pinnedRelay ?? AUTO_RELAY}
            options={relayOptions}
            aria-label={t('shareSetRelay')}
            onValueChange={(relay) => void saveLocalConfig({ pinnedRelay: relay === AUTO_RELAY ? null : relay })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetUdpV4')} description={t('shareSetUdpV4Desc')}>
          <TextField
            value={config.udpPortV4 === null ? '' : String(config.udpPortV4)}
            placeholder={t('shareSetPortAuto')}
            aria-label={t('shareSetUdpV4')}
            onCommit={(text) => {
              const port = parsePort(text)
              if (port !== undefined) void saveLocalConfig({ udpPortV4: port })
            }}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetUdpV6')} description={t('shareSetUdpV6Desc')}>
          <TextField
            value={config.udpPortV6 === null ? '' : String(config.udpPortV6)}
            placeholder={t('shareSetPortAuto')}
            aria-label={t('shareSetUdpV6')}
            onCommit={(text) => {
              const port = parsePort(text)
              if (port !== undefined) void saveLocalConfig({ udpPortV6: port })
            }}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetPortmapper')} description={t('shareSetPortmapperDesc')}>
          <DropdownField
            value={portmapperChoice(config.portmapper)}
            options={portmapperOptions}
            aria-label={t('shareSetPortmapper')}
            onValueChange={(choice) => void saveLocalConfig({ portmapper: portmapperWire(choice) })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetExtraAddrs')} description={t('shareSetExtraAddrsDesc')} vertical>
          <TextField
            value={formatAddrList(config.extraAddrs)}
            placeholder="203.0.113.7:4433, [2001:db8::7]:4433"
            aria-label={t('shareSetExtraAddrs')}
            className="w-full"
            onCommit={(text) => void saveLocalConfig({ extraAddrs: parseAddrList(text) })}
          />
        </SettingsRow>
      </SettingsSection>

      <SettingsSection title={t('shareSetGroupTransfer')}>
        <SettingsRow title={t('shareSetAutoProvide')} description={t('shareSetAutoProvideDesc')} compact>
          <Switch
            checked={config.autoProvide !== false}
            aria-label={t('shareSetAutoProvide')}
            onCheckedChange={(on) => void saveLocalConfig({ autoProvide: on ? null : false })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetProvideDailyCap')} description={t('shareSetProvideDailyCapDesc')}>
          <TextField
            value={formatDailyCap(config.provideDailyCapBytes)}
            placeholder={t('shareSetProvideDailyCapDefault', { value: defaultCap > 0 ? formatBytes(defaultCap) : t('shareUsageUnlimited') })}
            aria-label={t('shareSetProvideDailyCap')}
            className="w-40"
            trailing={<span className="text-muted-foreground">GB</span>}
            onCommit={(text) => {
              const cap = parseDailyCap(text)
              if (cap !== undefined) void saveLocalConfig({ provideDailyCapBytes: cap })
            }}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetRateLimit')} description={t('shareSetRateLimitDesc')}>
          <NumberField
            value={rateFromWire(config.sendRateLimitBytesPerSec)}
            min={0}
            step={0.5}
            float
            unit="MB/s"
            aria-label={t('shareSetRateLimit')}
            onCommit={(value) => void saveLocalConfig({ sendRateLimitBytesPerSec: rateToWire(value) })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetExit')} description={t('shareSetExitDesc')}>
          <DropdownField
            value={config.exitBehavior}
            options={exitOptions}
            aria-label={t('shareSetExit')}
            onValueChange={(exitBehavior) => void saveLocalConfig({ exitBehavior })}
          />
        </SettingsRow>
        <SettingsRow title={t('shareSetLogLevel')} description={t('shareSetLogLevelDesc')}>
          <DropdownField
            value={config.irohLogLevel}
            options={logOptions}
            aria-label={t('shareSetLogLevel')}
            onValueChange={(irohLogLevel) => void saveLocalConfig({ irohLogLevel })}
          />
        </SettingsRow>
      </SettingsSection>
        </>
      ) : null}
    </>
  )

  return (
    <SettingsPage
      title={t('settingsCatShare')}
      description={t('settingsCatShareDesc')}
      initialTabId={focus && available ? 'network' : 'general'}
      tabs={[
        { id: 'general', label: t('settingsTabGeneral'), content: general },
        ...(available
          ? [{ id: 'network', label: t('shareTabNetwork'), content: <NetworkDiagnostics focusSessionId={focus?.sessionId ?? null} /> }]
          : []),
      ]}
    />
  )
}
