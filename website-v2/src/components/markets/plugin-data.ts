/**
 * 插件市场数据契约(对应 zerx-lab/fluxdown-plugin-index 的 index.json)与纯函数。
 * 索引经同源代理 `/api/plugins/*` 拉取(服务端中转 raw.githubusercontent.com,大陆可达 + CDN 缓存)。
 */
import { PLUGIN_INDEX_REPO, githubUrl } from "@/lib/ecosystem";

/** 索引条目:每插件每版本一条(append-only 分片 flatten 而来)。 */
export interface MarketEntry {
  pluginId: string;
  version: string;
  sequence: number;
  contentHash: string;
  minAppVersion?: string;
  name?: string;
  description?: string;
  author?: string;
  homepage?: string;
  mirrors?: string[];
  publishTime?: string;
  yanked?: string;
  tags?: string[];
  permissions?: string[];
}

export interface MarketIndex {
  indexId?: string;
  sequence?: number;
  updated?: string;
  entries?: MarketEntry[];
}

/** 按 pluginId 聚合后的插件(含全部历史版本,降序)。 */
export interface PluginGroup {
  pluginId: string;
  latest: MarketEntry;
  versions: MarketEntry[];
}

export type YankState = "deprecated" | "vulnerable" | "malicious";

/** 每插件的提交历史深链——逐版本变更可独立追踪。 */
export function commitsUrl(pluginId: string): string {
  return `${githubUrl(PLUGIN_INDEX_REPO)}/commits/main/plugins/${encodeURIComponent(pluginId)}`;
}

/** 优先取 jsDelivr 镜像(大陆可达 + CDN),回退首个镜像。 */
export function bestMirror(entry: MarketEntry): string | null {
  const mirrors = entry.mirrors ?? [];
  return mirrors.find((m) => m.includes("jsdelivr")) ?? mirrors[0] ?? null;
}

/** ISO 时间戳 → YYYY-MM-DD(确定性、与语言无关)。 */
export function isoDate(iso?: string): string {
  return iso && iso.length >= 10 ? iso.slice(0, 10) : "";
}

/** `sha256:<hex>` → 前 12 位短哈希(展示用)。 */
export function shortHash(hash: string): string {
  const hex = hash.startsWith("sha256:") ? hash.slice(7) : hash;
  return hex.slice(0, 12);
}

export function yankState(yanked?: string): YankState | null {
  return yanked === "deprecated" || yanked === "vulnerable" || yanked === "malicious" ? yanked : null;
}

/** 把索引条目按 pluginId 聚合,版本按 sequence 降序,组按最新版发布时间降序。 */
export function groupPlugins(entries: MarketEntry[]): PluginGroup[] {
  const byId = new Map<string, MarketEntry[]>();
  for (const e of entries) {
    const list = byId.get(e.pluginId);
    if (list) list.push(e);
    else byId.set(e.pluginId, [e]);
  }
  const groups: PluginGroup[] = [];
  for (const [pluginId, versions] of byId) {
    versions.sort((a, b) => b.sequence - a.sequence);
    groups.push({ pluginId, latest: versions[0]!, versions });
  }
  groups.sort((a, b) => (b.latest.publishTime ?? "").localeCompare(a.latest.publishTime ?? ""));
  return groups;
}

/** 可用筛选:all + 数据中出现过的权限 + resolver 标签。 */
export function pluginFilters(groups: PluginGroup[]): string[] {
  const set = new Set<string>();
  for (const g of groups) {
    for (const perm of g.latest.permissions ?? []) set.add(perm);
    if ((g.latest.tags ?? []).includes("resolver")) set.add("resolver");
  }
  return ["all", ...set];
}

export function filterPlugins(groups: PluginGroup[], filter: string, query: string): PluginGroup[] {
  let list = groups;
  if (filter !== "all") {
    list = list.filter((g) =>
      filter === "resolver"
        ? (g.latest.tags ?? []).includes("resolver")
        : (g.latest.permissions ?? []).includes(filter),
    );
  }
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter((g) => {
    const p = g.latest;
    return (
      g.pluginId.toLowerCase().includes(q) ||
      (p.name ?? "").toLowerCase().includes(q) ||
      (p.description ?? "").toLowerCase().includes(q) ||
      (p.author ?? "").toLowerCase().includes(q) ||
      (p.tags ?? []).some((tag) => tag.toLowerCase().includes(q))
    );
  });
}
