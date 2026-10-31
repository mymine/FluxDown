// 插件市场：`daemon.plugin.marketList` 拉取索引，前端关键字过滤 + 分页展开，`marketInstall` 安装。

import { CircleAlert, Info, Package, Search, Star, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { cn } from '../../../../lib/cn'
import { RpcError, rpc, useAgent } from '../../../../lib/rpc'
import type { InstalledPlugin, MarketEntryDto, PluginDto } from '../../../../lib/rpc'
import { Badge, Button, EmptyState, Icon, Input, Select, Spinner, Tooltip } from '../../../../ui'
import { BlockTitle, ExtLink, IconButton, ListCard, ListRow } from './common'
import { detailFromMarket } from './detail'
import type { PluginDetail } from './detail'
import { extensionErrorText } from './errors'
import {
  MARKET_PAGE_SIZE,
  arrangeMarket,
  filterMarket,
  hostsToConfirm,
  installedVersionYanked,
  latestPerPlugin,
  marketAction,
  marketNeedsLoad,
  permissionsToConfirm,
  riskOf,
  yankedLabelKey,
} from './logic'
import type { ConsentSet, MarketLoadState, MarketSort } from './logic'
import { PluginConsentDialog, RiskBadge } from './PluginConsentDialog'
import type { ConsentView } from './PluginConsentDialog'

interface MarketState {
  loading: boolean
  error: string | null
  entries: MarketEntryDto[]
}

export function MarketSection({
  stale,
  active,
  plugins,
  onInstalled,
  onInstallFailed,
  onShowDetail,
}: {
  stale: boolean
  /** 插件页当前可见（组件页隐藏时为 false）；进入时检查目录是否过期。 */
  active: boolean
  plugins: readonly PluginDto[]
  onInstalled: (result: InstalledPlugin, confirmed: ConsentSet | null, updating: boolean) => void | Promise<void>
  onInstallFailed: (error: unknown) => void
  onShowDetail: (detail: PluginDetail) => void
}) {
  const t = useT()
  const [market, setMarket] = useState<MarketState>({ loading: false, error: null, entries: [] })
  const [query, setQuery] = useState('')
  const [limit, setLimit] = useState(MARKET_PAGE_SIZE)
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set())
  const [confirm, setConfirm] = useState<{ view: ConsentView; resolve: (ok: boolean) => void } | null>(null)
  const loadState = useRef<MarketLoadState>({ requested: false, retryOnReconnect: false, failed: false, loadedAt: null })
  const inflight = useRef<Promise<void> | null>(null)
  const signedIn = useAgent((snapshot) => snapshot.session !== null && snapshot.session !== undefined, false)
  const [sort, setSort] = useState<MarketSort>('default')
  const [favoritesOnlyRaw, setFavoritesOnly] = useState(false)
  const [counts, setCounts] = useState<Record<string, number>>({})
  const [mine, setMine] = useState<string[]>([])
  const [favorites, setFavorites] = useState({ failed: false })
  const [favPending, setFavPending] = useState<ReadonlySet<string>>(new Set())
  const loadFavorites = async () => {
    try {
      const result = await rpc.agent.pluginFavorite.list()
      setCounts(result.counts)
      setMine(result.mine)
      setFavorites({ failed: false })
    } catch {
      // 收藏失败不影响市场列表：计数清空、星标禁用并给出内联提示。
      setCounts({})
      setMine([])
      setFavorites({ failed: true })
    }
  }

  const fetchCatalog = async () => {
    loadState.current = { ...loadState.current, requested: true, retryOnReconnect: false, failed: false }
    setMarket((current) => ({ ...current, loading: true, error: null }))
    void loadFavorites()
    try {
      const entries = await rpc.daemon.plugin.marketList()
      loadState.current = { ...loadState.current, failed: false, loadedAt: Date.now() }
      setMarket({ loading: false, error: null, entries })
      setLimit(MARKET_PAGE_SIZE)
    } catch (error) {
      // 断连 / 超时类失败：连接恢复后自动重试一次；其他失败保持手动刷新。
      const transport = error instanceof RpcError && (error.appCode === 'unavailable' || error.appCode === 'timeout')
      loadState.current = { ...loadState.current, failed: true, retryOnReconnect: transport }
      setMarket((current) => ({ ...current, loading: false, error: extensionErrorText(t, error) }))
    }
  }
  /** 同一时刻只有一个目录请求：在途时复用其 Promise。 */
  const load = (): Promise<void> => {
    inflight.current ??= fetchCatalog().finally(() => {
      inflight.current = null
    })
    return inflight.current
  }
  const loadRef = useRef(load)
  loadRef.current = load
  const loadFavoritesRef = useRef(loadFavorites)
  loadFavoritesRef.current = loadFavorites

  // 登录态变化（登录 / 登出）且市场已请求过时重新拉收藏。
  const lastSignedIn = useRef(signedIn)
  useEffect(() => {
    if (lastSignedIn.current === signedIn) return
    lastSignedIn.current = signedIn
    if (loadState.current.requested) void loadFavoritesRef.current()
  }, [signedIn])

  // 进入插件页（含从组件页切回）且连接就绪时按需拉取：从未请求、断连失败待重试、或目录已超过
  // `MARKET_MAX_AGE_MS`（与 GPUI `ensure_market_loaded` 一致）；在途请求不重复发起。
  useEffect(() => {
    if (!active || stale || inflight.current !== null) return
    if (marketNeedsLoad(loadState.current, Date.now())) void loadRef.current()
  }, [active, stale])

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
      await onInstalled(await rpc.daemon.plugin.marketInstall({ pluginId, version: entry.version }), confirmed, installed !== undefined)
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
  const mineSet = useMemo(() => new Set(mine), [mine])
  const favoriteDisabled = !signedIn || favorites.failed
  const favoriteTitle = (pluginId: string) =>
    t(!signedIn ? 'marketFavoriteLoginRequired' : mineSet.has(pluginId) ? 'marketFavoriteRemove' : 'marketFavoriteAdd')

  const toggleFavorite = async (pluginId: string) => {
    if (favPending.has(pluginId)) return
    const target = !mineSet.has(pluginId)
    setFavPending((current) => new Set(current).add(pluginId))
    try {
      const result = await rpc.agent.pluginFavorite.set({ pluginId, favorited: target })
      setMine((current) => {
        const rest = current.filter((id) => id !== pluginId)
        return result.favorited ? [pluginId, ...rest] : rest
      })
      setCounts((current) => ({ ...current, [pluginId]: result.count }))
    } catch (error) {
      onInstallFailed(error)
    } finally {
      setFavPending((current) => {
        const next = new Set(current)
        next.delete(pluginId)
        return next
      })
    }
  }

  const favoritesOnly = favoritesOnlyRaw && signedIn
  const filtered = useMemo(
    () => arrangeMarket(filterMarket(latestPerPlugin(market.entries), query), sort, counts, mineSet, favoritesOnly),
    [market.entries, query, sort, counts, mineSet, favoritesOnly],
  )
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
        <div className="flex flex-wrap items-center gap-2">
          <div className="w-44">
            <Select<MarketSort>
              value={sort}
              aria-label={t('marketSortLabel')}
              options={[
                { value: 'default', label: t('marketSortDefault') },
                { value: 'newest', label: t('marketSortNewest') },
                { value: 'favorites', label: t('marketSortFavorites') },
              ]}
              onValueChange={(next) => {
                setSort(next)
                setLimit(MARKET_PAGE_SIZE)
              }}
            />
          </div>
          <Button
            variant={favoritesOnly ? 'primary' : 'outline'}
            icon={Star}
            aria-pressed={favoritesOnly}
            disabled={!signedIn}
            title={signedIn ? undefined : t('marketFavoriteLoginRequired')}
            onClick={() => {
              setFavoritesOnly((current) => !current)
              setLimit(MARKET_PAGE_SIZE)
            }}
          >
            {t('marketFavoritesOnly')}
          </Button>
          {favorites.failed ? <span className="text-xs text-muted-foreground">{t('marketFavoritesLoadFailed')}</span> : null}
        </div>
        {filtered.length === 0 ? (
          <EmptyState icon={favoritesOnly && query.trim() === '' ? Star : Search} title={t(favoritesOnly && query.trim() === '' ? 'marketFavoritesEmpty' : 'marketSearchNoResult')} />
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
                      <Tooltip content={favoriteTitle(entry.pluginId)}>
                        <Button
                          variant="ghost"
                          aria-label={favoriteTitle(entry.pluginId)}
                          aria-pressed={mineSet.has(entry.pluginId)}
                          disabled={favoriteDisabled || favPending.has(entry.pluginId)}
                          loading={favPending.has(entry.pluginId)}
                          onClick={() => void toggleFavorite(entry.pluginId)}
                          className={cn('gap-1 px-2 tabular', mineSet.has(entry.pluginId) ? 'text-accent-text' : 'text-muted-foreground hover:text-foreground')}
                        >
                          {favPending.has(entry.pluginId) ? null : <Icon icon={Star} size="md" className={mineSet.has(entry.pluginId) ? 'fill-current' : undefined} />}
                          {(counts[entry.pluginId] ?? 0) > 0 ? <span className="text-xs">{counts[entry.pluginId]}</span> : null}
                        </Button>
                      </Tooltip>
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
