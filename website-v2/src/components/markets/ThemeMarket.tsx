/** 主题市场岛:拉取主题索引 → 搜索 → 卡片网格 + 截图大图。 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { markets } from "@/i18n/messages/markets";
import type { Lang } from "@/i18n/config";
import { filterThemes, themeAssetUrl, type ThemeIndex } from "./theme-data";
import { THEMES_REPO, githubUrl } from "@/lib/ecosystem";
import { GridFill, SearchField, SkeletonGrid, StateMessage } from "./MarketUI";
import { ThemeCard } from "./ThemeCard";
import { ThemeLightbox, type LightboxState } from "./ThemeLightbox";

export default function ThemeMarket({ lang }: { lang: Lang }) {
  const t = markets[lang];
  const [index, setIndex] = useState<ThemeIndex | null>(null);
  const [error, setError] = useState(false);
  const [query, setQuery] = useState("");
  const [lightbox, setLightbox] = useState<LightboxState | null>(null);

  useEffect(() => {
    fetch(themeAssetUrl("index.json"))
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((data: ThemeIndex) => setIndex(data))
      .catch(() => setError(true));
  }, []);

  const filtered = useMemo(() => filterThemes(index?.themes ?? [], query), [index, query]);
  const navigate = useCallback((i: number) => setLightbox((s) => (s ? { ...s, index: i } : s)), []);
  const close = useCallback(() => setLightbox(null), []);
  const loading = index === null && !error;

  return (
    <div>
      <div className="section-tight flex flex-col gap-4 pb-6 sm:flex-row sm:items-center sm:justify-between">
        <SearchField
          value={query}
          onChange={setQuery}
          placeholder={t.themes.searchPlaceholder}
          label={t.common.search}
          clearLabel={t.common.clear}
        />
        {index && (
          <span className="num font-mono text-[11px] text-subtle" aria-live="polite">
            {t.common.results(filtered.length)}
          </span>
        )}
      </div>

      <div className="border-t border-line">
        {loading && <SkeletonGrid media />}
        {error && (
          <StateMessage
            tone="danger"
            title={t.themes.loadError}
            hint={t.common.retryHint}
            link={{ href: githubUrl(THEMES_REPO), label: `github.com/${THEMES_REPO}` }}
          />
        )}
        {index && filtered.length === 0 && <StateMessage title={t.themes.empty} />}
        {index && filtered.length > 0 && (
          <div className="cells grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
            {filtered.map((theme, i) => (
              <ThemeCard
                key={theme.id}
                theme={theme}
                index={i}
                onPreview={(vi) => setLightbox({ theme, index: vi })}
                t={t.themes}
                lang={lang}
              />
            ))}
            <GridFill count={filtered.length} />
          </div>
        )}
      </div>

      {lightbox && (
        <ThemeLightbox state={lightbox} onClose={close} onNavigate={navigate} t={t.themes} closeLabel={t.common.close} />
      )}
    </div>
  );
}
