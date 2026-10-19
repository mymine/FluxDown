/** 插件市场岛:拉取开放索引 → 聚合版本 → 搜索 / 筛选 → 卡片网格 + 详情弹层。 */
import { useEffect, useMemo, useState } from "react";
import { markets } from "@/i18n/messages/markets";
import type { Lang } from "@/i18n/config";
import { cn } from "@/lib/utils";
import { filterPlugins, groupPlugins, pluginFilters, type MarketIndex, type PluginGroup } from "./plugin-data";
import { PLUGIN_INDEX_REPO, githubUrl } from "@/lib/ecosystem";
import { GridFill, SearchField, SkeletonGrid, StateMessage } from "./MarketUI";
import { PluginCard } from "./PluginCard";
import { PluginDetail } from "./PluginDetail";
import { withBase } from "@/lib/base";

export default function PluginMarket({ lang }: { lang: Lang }) {
  const t = markets[lang];
  const [groups, setGroups] = useState<PluginGroup[] | null>(null);
  const [error, setError] = useState(false);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [selected, setSelected] = useState<PluginGroup | null>(null);

  useEffect(() => {
    fetch(withBase("/api/plugins/index.json"))
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((data: MarketIndex) => setGroups(groupPlugins(data.entries ?? [])))
      .catch(() => setError(true));
  }, []);

  const filters = useMemo(() => pluginFilters(groups ?? []), [groups]);
  const filtered = useMemo(() => filterPlugins(groups ?? [], filter, query), [groups, filter, query]);
  const loading = groups === null && !error;

  return (
    <div>
      <div className="section-tight flex flex-col gap-4 pb-6 sm:flex-row sm:items-center sm:justify-between">
        <SearchField
          value={query}
          onChange={setQuery}
          placeholder={t.plugins.searchPlaceholder}
          label={t.common.search}
          clearLabel={t.common.clear}
        />
        {groups && (
          <div className="flex flex-wrap items-center gap-3">
            {filters.length > 1 && (
              <div
                role="group"
                aria-label={t.plugins.filterLabel}
                className="inline-flex flex-wrap rounded-lg p-0.5 shadow-[inset_0_0_0_1px_var(--line-strong)]"
              >
                {filters.map((f) => (
                  <button
                    key={f}
                    type="button"
                    aria-pressed={filter === f}
                    onClick={() => setFilter(f)}
                    className={cn(
                      "h-7 rounded-md px-3 font-mono text-[11.5px] transition-colors focus-visible:outline-2 focus-visible:outline-accent",
                      filter === f ? "bg-inset text-fg" : "text-subtle hover:text-fg",
                    )}
                  >
                    {f === "all" ? t.plugins.filter.all : f === "resolver" ? t.plugins.filter.resolver : f}
                  </button>
                ))}
              </div>
            )}
            <span className="num font-mono text-[11px] text-subtle" aria-live="polite">
              {t.common.results(filtered.length)}
            </span>
          </div>
        )}
      </div>

      <div className="border-t border-line">
        {loading && <SkeletonGrid />}
        {error && (
          <StateMessage
            tone="danger"
            title={t.plugins.loadError}
            hint={t.common.retryHint}
            link={{ href: githubUrl(PLUGIN_INDEX_REPO), label: `github.com/${PLUGIN_INDEX_REPO}` }}
          />
        )}
        {groups && filtered.length === 0 && <StateMessage title={t.plugins.empty} />}
        {groups && filtered.length > 0 && (
          <div className="cells grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
            {filtered.map((g, i) => (
              <PluginCard key={g.pluginId} group={g} index={i} onOpen={() => setSelected(g)} t={t.plugins} />
            ))}
            <GridFill count={filtered.length} />
          </div>
        )}
      </div>

      {selected && (
        <PluginDetail group={selected} onClose={() => setSelected(null)} t={t.plugins} closeLabel={t.common.close} />
      )}
    </div>
  );
}
