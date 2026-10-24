/** options 页 hash 路由：`#<panel>` 或 `#<panel>?k=v`，无效 panel 回落到 general。 */

export const PANELS = [
  "general",
  "intercept",
  "sniff",
  "sites",
  "rules",
  "pageScripts",
  "remote",
  "shortcuts",
  "backup",
  "about",
] as const;

export type PanelId = (typeof PANELS)[number];

export interface Route {
  panel: PanelId;
  params: URLSearchParams;
}

export function parseHash(hash: string): Route {
  const body = hash.replace(/^#/, "");
  const queryAt = body.indexOf("?");
  const name = queryAt === -1 ? body : body.slice(0, queryAt);
  const query = queryAt === -1 ? "" : body.slice(queryAt + 1);
  const panel = (PANELS as readonly string[]).includes(name) ? (name as PanelId) : "general";
  return { panel, params: new URLSearchParams(query) };
}
