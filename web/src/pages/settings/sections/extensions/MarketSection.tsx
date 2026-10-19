// 插件市场：`daemon.plugin.marketList` 拉取索引，前端关键字过滤 + 分页展开，`marketInstall` 安装。

import { CircleAlert, Info, Package, Search, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { RpcError, rpc } from '../../../../lib/rpc'
import type { InstalledPlugin, MarketEntryDto, PluginDto } from '../../../../lib/rpc'
import { Badge, Button, EmptyState, Icon, Input, Spinner } from '../../../../ui'
import { BlockTitle, ExtLink, IconButton, ListCard, ListRow } from './common'
import { detailFromMarket } from './detail'
import type { PluginDetail } from './detail'
import { extensionErrorText } from './errors'
import {
  MARKET_PAGE_SIZE,
  filterMarket,
  hostsToConfirm,
  installedVersionYanked,
  latestPerPlugin,
  marketAction,
  permissionsToConfirm,
  riskOf,
  yankedLabelKey,
} from './logic'
import type { ConsentSet } from './logic'
import { PluginConsentDialog, RiskBadge } from './PluginConsentDialog'
import type { ConsentView } from './PluginConsentDialog'

interface MarketState {
  loading: boolean
  error: string | null
  entries: MarketEntryDto[]
}

export function MarketSection({
  stale,
  plugins,
  onInstalled,
  onInstallFailed,
  onShowDetail,
}: {
  stale: boolean
  plugins: readonly PluginDto[]
  onInstalled: (result: InstalledPlugin, confirmed: ConsentSet | null) => void | Promise<void>
  onInstallFailed: (error: unknown) => void
  onShowDetail: (detail: PluginDetail) => void
}) {
  const t = useT()
  const [market, setMarket] = useState<MarketState>({ loading: false, error: null, entries: [] })
  const [query, setQuery] = useState('')
  const [limit, setLimit] = useState(MARKET_PAGE_SIZE)
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set())
  const [confirm, setConfirm] = useState<{ view: ConsentView; resolve: (ok: boolean) => void } | null>(null)
  const requested = useRef(false)

  const load = async () => {
    requested.current = true
    setMarket((current) => ({ ...current, loading: true, error: null }))
    try {
      const entries = await rpc.daemon.plugin.marketList()
      setMarket({ loading: false, error: null, entries })
      setLimit(MARKET_PAGE_SIZE)
    } catch (error) {
      setMarket((current) => ({ ...current, loading: false, error: extensionErrorText(t, error) }))
    }
  }
  const loadRef = useRef(load)
  loadRef.current = load

  // 首次进入插件页且连接就绪时自动拉取一次（与 GPUI `ensure_market_loaded` 一致）。
  useEffect(() => {
    if (!requested.current && !stale) void loadRef.current()
  }, [stale])

  const install = async (entry: MarketEntryDto, installed: PluginDto | undefined) => {
    const pluginId = entry.pluginId
    if (pending.has(pluginId)) return
    const permissions = permissionsToConfirm(entry, installed)
    const networkHosts = hostsToConfirm(entry, installed)
    const confirmed: ConsentSet = { permissions, networkHosts }
    if (permissions.length > 0 || networkHosts.length > 0) {
      const updating = installed !== undefined
      const name = entry.name === '' ? pluginId : entry.name
      const ok = await new Promise<boolean>((resolve) => {
        setConfirm({
          resolve,
          view: {
            title: t(updating ? 'pluginPermConfirmUpdateTitle' : 'pluginPermConfirmInstallTitle', { name }),
            intro: t(updating ? 'pluginPermConfirmUpdateBody' : 'pluginPermConfirmInstallBody', { version: entry.version }),
            permissions,
            networkHosts,
            risk: riskOf(entry.permissions),
            okLabel: t(updating ? 'pluginPermConfirmUpdateOk' : 'pluginPermConfirmInstallOk'),
          },
        })
      })
      if (!ok) return
    }
    setPending((current) => new Set(current).add(pluginId))
    try {
      await onInstalled(await rpc.daemon.plugin.marketInstall({ pluginId, version: entry.version }), confirmed)
    } catch (error) {
      // 用户确认的版本已不是最新：刷新目录，让下一次点击按新版本重新确认权限。
      if (error instanceof RpcError && error.reason === 'marketVersionChanged') await load()
      onInstallFailed(error)
    } finally {
      setPending((current) => {
        const next = new Set(current)
        next.delete(pluginId)
        return next
      })
    }
  }

  const installedById = useMemo(() => new Map(plugins.map((plugin) => [plugin.identity, plugin])), [plugins])

  const filtered = useMemo(() => filterMarket(latestPerPlugin(market.entries), query), [market.entries, query])
  const remaining = Math.max(0, filtered.length - limit)

  let body
  if (market.loading && market.entries.length === 0) {
    body = (
      <div className="flex items-center gap-2 py-3 text-xs text-muted-foreground">
        <Spinner size="md" />
        {t('pluginCommonLoading')}
      </div>
    )
  } else if (market.error) {
    body = (
      <div className="flex items-start gap-2 text-xs text-muted-foreground">
        <Icon icon={CircleAlert} size="md" className="mt-0.5 text-destructive" />
        <span className="min-w-0 flex-1 break-words">{t('marketLoadFailed', { message: market.error })}</span>
      </div>
    )
  } else if (market.entries.length === 0) {
    body = <EmptyState icon={Package} title={t('marketEmpty')} />
  } else {
    body = (
      <>
        <Input
          inputMode="search"
          className="coarse:pr-12"
          value={query}
          placeholder={t('marketSearchPlaceholder')}
          aria-label={t('marketSearchPlaceholder')}
          onChange={(event) => {
            setQuery(event.target.value)
            setLimit(MARKET_PAGE_SIZE)
          }}
          trailing={
            query === '' ? (
              <span className="pr-2 text-text-tertiary">
                <Icon icon={Search} size="md" />
              </span>
            ) : (
              <Button variant="ghost" iconOnly className="text-muted-foreground hover:text-foreground" aria-label={t('componentsManualPathClear')} onClick={() => setQuery('')}>
                <Icon icon={X} size="md" />
              </Button>
            )
          }
        />
        {filtered.length === 0 ? (
          <EmptyState icon={Search} title={t('marketSearchNoResult')} />
        ) : (
          <ListCard>
            {filtered.slice(0, limit).map((entry) => {
              const installedPlugin = installedById.get(entry.pluginId)
              const action = marketAction(entry, installedPlugin)
              const isPending = pending.has(entry.pluginId)
              const yankedKey = yankedLabelKey(entry.yanked)
              const installedYanked = installedPlugin ? installedVersionYanked(market.entries, installedPlugin) : null
              const installedYankedKey = installedYanked ? yankedLabelKey(installedYanked) : null
              return (
                <ListRow
                  key={entry.pluginId}
                  info={
                    <>
                      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                        <span className="min-w-0 break-words text-sm font-medium text-foreground">{entry.name === '' ? entry.pluginId : entry.name}</span>
                        <span className="tabular text-xs text-muted-foreground">v{entry.version}</span>
                        {entry.author ? <span className="text-xs text-muted-foreground">{entry.author}</span> : null}
                        {yankedKey ? <Badge tone="destructive">{t(yankedKey)}</Badge> : null}
                        {installedYankedKey ? <Badge tone="destructive">{t('pluginInstalledVersionYanked', { label: t(installedYankedKey) })}</Badge> : null}
                        <RiskBadge risk={riskOf(entry.permissions)} />
                      </div>
                      {entry.homepage ? <ExtLink href={entry.homepage} /> : null}
                      {entry.description ? <div className="line-clamp-2 break-words text-xs text-muted-foreground">{entry.description}</div> : null}
                    </>
                  }
                  actions={
                    <>
                      <IconButton icon={Info} label={t('pluginDetailDescription')} onClick={() => onShowDetail(detailFromMarket(entry))} />
                      <Button
                        variant="outline"
                        loading={isPending}
                        disabled={action === 'installed' || action === 'unavailable' || stale}
                        onClick={() => void install(entry, installedPlugin)}
                      >
                        {isPending
                          ? t(action === 'update' ? 'marketUpdatingButton' : 'marketInstallingButton')
                          : t(
                              action === 'install'
                                ? 'marketInstallButton'
                                : action === 'update'
                                  ? 'marketUpdateButton'
                                  : action === 'installed'
                                    ? 'marketInstalledButton'
                                    : 'marketUnavailableButton',
                            )}
                      </Button>
                    </>
                  }
                />
              )
            })}
          </ListCard>
        )}
        {remaining > 0 ? (
          <div className="flex justify-center">
            <Button variant="ghost" onClick={() => setLimit((current) => current + MARKET_PAGE_SIZE)}>
              {t('marketShowMore', { count: remaining })}
            </Button>
          </div>
        ) : null}
      </>
    )
  }

  return (
    <section className="flex w-full flex-col gap-2 pt-4">
      <BlockTitle
        title={t('marketSectionTitle')}
        description={t('marketSectionDesc')}
        trailing={
          <Button variant="ghost" loading={market.loading} disabled={stale} onClick={() => void load()}>
            {t('marketRefreshTooltip')}
          </Button>
        }
      />
      {body}
      <PluginConsentDialog
        consent={confirm?.view ?? null}
        onCancel={() => {
          confirm?.resolve(false)
          setConfirm(null)
        }}
        onOk={() => {
          confirm?.resolve(true)
          setConfirm(null)
        }}
      />
    </section>
  )
}
