// 「站点默认插件」卡片：多个插件命中同一站点时记住的选择。
// `automatic` 条目 = 无人值守下载自动选定、尚未确认（普通下载仍会询问），可在此确认；
// 列表为空时不渲染。打开页面 / 插件列表变化 / 每次变更后重新拉取。

import { Check, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useT } from '../../../../i18n'
import { rpc } from '../../../../lib/rpc'
import type { PluginDto, PluginSiteDefaultDto } from '../../../../lib/rpc'
import { Badge, Button, Card, Select, toast } from '../../../../ui'
import { BlockTitle, IconButton } from './common'
import { extensionErrorText } from './errors'

export function SiteDefaultsCard({ plugins, stale }: { plugins: readonly PluginDto[]; stale: boolean }) {
  const t = useT()
  const [entries, setEntries] = useState<readonly PluginSiteDefaultDto[]>([])
  const [busySite, setBusySite] = useState<string | null>(null)
  const names = useMemo(() => new Map(plugins.map((plugin) => [plugin.identity, plugin.name])), [plugins])

  const refresh = useCallback(async () => {
    try {
      setEntries(await rpc.daemon.plugin.siteDefaults())
    } catch {
      // 不含插件的 daemon 返回 unsupported（列表本就为空）；瞬时错误保持上一次列表，不打扰用户。
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh, plugins])

  const mutate = async (site: string, identity: string | null) => {
    if (busySite !== null) return
    setBusySite(site)
    try {
      await rpc.daemon.plugin.setSiteDefault({ site, identity })
    } catch (error) {
      toast.error(t('pluginSiteDefaultSaveFailed', { error: extensionErrorText(t, error) }))
    } finally {
      await refresh()
      setBusySite(null)
    }
  }

  if (entries.length === 0) return null
  const nameOf = (identity: string) => names.get(identity) ?? identity

  return (
    <div className="flex flex-col gap-2">
      <BlockTitle title={t('pluginSiteDefaultsTitle')} description={t('pluginSiteDefaultsDesc')} />
      <Card className="flex w-full flex-col overflow-hidden [&>*+*]:border-t [&>*+*]:border-hairline">
        {entries.map((entry) => {
          const disabled = stale || busySite !== null
          return (
            <div key={entry.site} className="flex flex-wrap items-center gap-x-3 gap-y-2 px-3 py-2">
              <div className="flex min-w-0 flex-[1_1_12rem] flex-col gap-0.5">
                <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <span className="min-w-0 break-all text-sm font-medium text-foreground">{entry.site}</span>
                  {entry.automatic ? <Badge tone="warning">{t('pluginSiteDefaultAutomatic')}</Badge> : null}
                </div>
                {entry.automatic ? <div className="text-xs text-muted-foreground">{t('pluginSiteDefaultAutomaticHint')}</div> : null}
              </div>
              <div className="ml-auto flex shrink-0 items-center gap-2">
                <Select
                  className="w-48"
                  aria-label={entry.site}
                  value={entry.identity}
                  disabled={disabled}
                  options={entry.candidates.map((identity) => ({ value: identity, label: nameOf(identity) }))}
                  onValueChange={(identity) => {
                    if (identity !== entry.identity) void mutate(entry.site, identity)
                  }}
                />
                {entry.automatic ? (
                  <Button variant="outline" icon={Check} disabled={disabled} onClick={() => void mutate(entry.site, entry.identity)}>
                    {t('pluginSiteDefaultConfirm')}
                  </Button>
                ) : null}
                <IconButton icon={Trash2} label={t('pluginSiteDefaultRemove')} onClick={() => void mutate(entry.site, null)} disabled={disabled} destructive />
              </div>
            </div>
          )
        })}
      </Card>
    </div>
  )
}
