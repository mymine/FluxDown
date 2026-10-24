// 授权对话框：展示插件请求的全部权限 / 网络主机 / 风险。市场安装前确认与「已装插件待授权」共用。
// 内容与 GPUI `components/plugin_consent.rs` 对齐；`full` 风险需勾选「我已了解」才能确定。

import { useEffect, useState } from 'react'
import { useT } from '../../../../i18n'
import { rpc } from '../../../../lib/rpc'
import type { PluginDto, PluginPageScriptDeclDto, PluginRisk } from '../../../../lib/rpc'
import { Badge, CheckRow, ConfirmFooter, Dialog, toast } from '../../../../ui'
import { extensionErrorText } from './errors'
import { permissionKeys, riskKeys } from './logic'

export interface ConsentView {
  /** 对话框标题（已本地化）。 */
  title: string
  /** 标题下的说明（缺省为 `pluginConsentIntro`）。 */
  intro?: string
  permissions: readonly string[]
  networkHosts: readonly string[]
  /** 页面脚本声明；市场安装前确认不携带。 */
  pageScripts?: readonly PluginPageScriptDeclDto[]
  risk: PluginRisk
  okLabel: string
}

/** 风险徽标：standard 中性、elevated warning、full destructive；悬停给出说明。 */
export function RiskBadge({ risk }: { risk: string }) {
  const t = useT()
  const keys = riskKeys(risk)
  return (
    <span title={t(keys.tooltip)}>
      <Badge tone={risk === 'full' ? 'destructive' : risk === 'elevated' ? 'warning' : 'neutral'}>{t(keys.label)}</Badge>
    </span>
  )
}

/** 纯展示：不发请求，确定 / 取消交给调用方。 */
export function PluginConsentDialog({
  consent,
  loading,
  onCancel,
  onOk,
}: {
  consent: ConsentView | null
  loading?: boolean
  onCancel: () => void
  onOk: () => void
}) {
  const t = useT()
  const [acknowledged, setAcknowledged] = useState(false)
  useEffect(() => {
    setAcknowledged(false)
  }, [consent])

  const risk = consent?.risk ?? 'standard'
  return (
    <Dialog
      open={consent !== null}
      onOpenChange={(open) => !open && !loading && onCancel()}
      title={consent?.title ?? ''}
      description={consent ? (consent.intro ?? t('pluginConsentIntro')) : undefined}
      modalLocked={loading}
      footer={
        <ConfirmFooter
          cancelLabel={t('cancel')}
          okLabel={consent?.okLabel ?? ''}
          intent={risk === 'full' ? 'destructive' : 'confirm'}
          okDisabled={risk === 'full' && !acknowledged}
          loading={loading}
          onCancel={onCancel}
          onOk={onOk}
        />
      }
    >
      {consent ? (
        <div className="flex w-full flex-col gap-2 pb-2">
          <div>
            <RiskBadge risk={risk} />
          </div>
          {consent.permissions.map((permission) => {
            const keys = permissionKeys(permission)
            return (
              <div key={permission} className="flex w-full flex-col gap-0.5">
                <div className="text-sm font-medium text-foreground">{keys ? t(keys.name) : permission}</div>
                <div className="text-xs text-muted-foreground">{t(keys ? keys.desc : 'pluginPermUnknownDesc')}</div>
              </div>
            )
          })}
          <div className="pt-2 text-caption font-medium text-text-tertiary">{t('pluginConsentNetworkHosts')}</div>
          {consent.networkHosts.length === 0 ? (
            <div className="text-xs text-muted-foreground">{t('pluginConsentNoNetworkHosts')}</div>
          ) : (
            consent.networkHosts.map((host) => (
              <div key={host} className="break-all font-mono text-xs text-foreground">
                {host}
              </div>
            ))
          )}
          {consent.pageScripts && consent.pageScripts.length > 0 ? (
            <>
              <div className="pt-2 text-caption font-medium text-text-tertiary">{t('pluginConsentPageScripts')}</div>
              {consent.pageScripts.map((script) => (
                <div key={script.id} className="flex w-full flex-col gap-0.5">
                  <div className="text-sm font-medium text-foreground">{script.title}</div>
                  <div className="break-all text-xs text-muted-foreground">{script.matches.join(', ')}</div>
                </div>
              ))}
            </>
          ) : null}
          {risk === 'full' ? <div className="pt-2 text-xs text-destructive">{t('pluginConsentFullTrustWarning')}</div> : null}
          <div className="text-xs text-muted-foreground">{t('pluginConsentNativeNotAudited')}</div>
          {risk === 'full' ? (
            <CheckRow checked={acknowledged} onCheckedChange={setAcknowledged}>
              {t('pluginConsentAcknowledge')}
            </CheckRow>
          ) : null}
        </div>
      ) : null}
    </Dialog>
  )
}

/** 已安装插件待授权：确定后以插件**完整** requested 集合调用 `daemon.plugin.consent`。 */
export function GrantConsentDialog({ plugin, onClose }: { plugin: PluginDto | null; onClose: () => void }) {
  const t = useT()
  const [loading, setLoading] = useState(false)
  const consent: ConsentView | null = plugin
    ? {
        title: t('pluginConsentTitle', { name: plugin.name }),
        permissions: plugin.permissions,
        networkHosts: plugin.networkHosts,
        pageScripts: plugin.contributions.pageScripts,
        risk: plugin.risk,
        okLabel: t('pluginConsentGrant'),
      }
    : null
  const grant = async () => {
    if (!plugin || loading) return
    setLoading(true)
    try {
      await rpc.daemon.plugin.consent({ identity: plugin.identity, permissions: plugin.permissions, networkHosts: plugin.networkHosts })
      onClose()
    } catch (error) {
      toast.error(t('pluginConsentFailed', { message: extensionErrorText(t, error) }))
    } finally {
      setLoading(false)
    }
  }
  return <PluginConsentDialog consent={consent} loading={loading} onCancel={onClose} onOk={() => void grant()} />
}
