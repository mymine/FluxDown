/**
 * 主题市场数据契约(对应 zerx-lab/fluxdown-themes 的 index.json)与资源读取。
 * 所有资源经同源代理 `/api/themes/*`(服务端中转 raw.githubusercontent.com,大陆可达 + CDN 缓存)。
 */
import type { Lang } from "@/i18n/config";
import { href } from "@/i18n/routing";
import { normalizeFluxThemeJson, type FluxThemeJson } from "@/lib/theme-builder";
import { withBase } from "@/lib/base";

export interface VariantAsset {
  /** 仓库相对路径,如 themes/<id>/theme.dark.json */
  theme: string;
  screenshot: string;
}

/** GPUI 客户端主题资源(与 Flutter 变体独立,单文件)。 */
export interface GpuiAsset {
  /** 仓库相对路径,如 themes/<id>/gpui.json */
  theme: string;
  screenshot?: string;
}

export interface MarketTheme {
  id: string;
  name: string;
  author: string;
  version: string;
  description?: string;
  tags?: string[];
  variants: Record<string, VariantAsset>;
  /** 其他客户端的主题资源;缺失时仅有 Flutter 变体。 */
  clients?: { gpui?: GpuiAsset };
}

export interface ThemeIndex {
  themes: MarketTheme[];
}

/** 仓库相对路径 → 同源代理 URL。 */
export function themeAssetUrl(path: string): string {
  return withBase(`/api/themes/${path}`);
}

const VARIANT_ORDER = ["dark", "light"];

/** 变体按 dark → light 排序(未知变体排最前,与旧版 indexOf 语义一致)。 */
export function orderedVariants(theme: MarketTheme): [string, VariantAsset][] {
  return Object.entries(theme.variants).sort(
    (a, b) => VARIANT_ORDER.indexOf(a[0]) - VARIANT_ORDER.indexOf(b[0]),
  );
}

/** 主题的 GPUI 资源(无则 undefined)。 */
export function gpuiAsset(theme: MarketTheme): GpuiAsset | undefined {
  return theme.clients?.gpui;
}

/** 主题构建器 GPUI 深链接:编辑器读取 client/market 参数并经 /api/themes 拉取导入。 */
export function gpuiEditorHref(path: string, lang: Lang): string {
  return `${href("/theme-builder", lang)}?client=gpui&market=${encodeURIComponent(path)}`;
}

/** 大图幻灯片:Flutter 变体截图在前(下标与 orderedVariants 一致),GPUI 截图(若有)追加在末尾。 */
export interface PreviewSlide {
  /** 变体键;GPUI 截图为 null */
  variant: string | null;
  screenshot: string;
}

export function previewSlides(theme: MarketTheme): PreviewSlide[] {
  const slides: PreviewSlide[] = orderedVariants(theme).map(([variant, asset]) => ({
    variant,
    screenshot: asset.screenshot,
  }));
  const gpuiShot = gpuiAsset(theme)?.screenshot;
  if (gpuiShot) slides.push({ variant: null, screenshot: gpuiShot });
  return slides;
}

export function filterThemes(themes: MarketTheme[], query: string): MarketTheme[] {
  const q = query.trim().toLowerCase();
  if (!q) return themes;
  return themes.filter(
    (th) =>
      th.name.toLowerCase().includes(q) ||
      th.author.toLowerCase().includes(q) ||
      th.id.includes(q) ||
      (th.tags ?? []).some((tag) => tag.toLowerCase().includes(q)) ||
      (q === "gpui" && gpuiAsset(th) !== undefined),
  );
}

/** 拉取主题 JSON 并触发浏览器下载(代理无 Content-Disposition,走 blob)。文件名:<id>-theme.dark.json */
export async function downloadThemeFile(path: string): Promise<void> {
  const res = await fetch(themeAssetUrl(path));
  if (!res.ok) throw new Error(String(res.status));
  const blob = await res.blob();
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = path.split("/").slice(-2).join("-");
  a.click();
  URL.revokeObjectURL(a.href);
}

const tokenCache = new Map<string, Promise<FluxThemeJson>>();

/** 读取并规范化主题 token(同一路径只请求一次),供卡片渲染调色板。 */
export function loadThemeTokens(path: string): Promise<FluxThemeJson> {
  let pending = tokenCache.get(path);
  if (!pending) {
    pending = fetch(themeAssetUrl(path))
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((raw: unknown) => normalizeFluxThemeJson(raw));
    pending.catch(() => tokenCache.delete(path));
    tokenCache.set(path, pending);
  }
  return pending;
}
