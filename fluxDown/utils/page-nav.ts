/**
 * 同一 tab 内页面 URL 变化是否算「换页」（需要清空嗅探投影）。
 *
 * content（sensor）与 background（tab-state）共用这一判定，保证两端对同一次 URL 变化
 * 得出同样结论：content 据此决定是否丢弃待发队列，background 据此决定是否清空资源 / 清单。
 */

/** URL 变化的来源。 */
export type PageUrlChangeKind =
  /** 顶层帧新文档启动（content script 首次上报）。 */
  | "document"
  /** history.pushState：新历史条目，SPA 路由跳转。 */
  | "push"
  /** history.replaceState：改写当前条目，常见于补追踪参数 / 规范化 URL。 */
  | "replace"
  /** popstate：前进 / 后退。 */
  | "pop"
  /** tabs.onUpdated：浏览器侧观察到的 URL 变化，无法区分整页导航与同文档改写。 */
  | "tab";

/**
 * hash 路由（`#/video/1`、`#!/watch`）是页面身份的一部分；普通锚点（`#comments`、`#t=30`）不是。
 */
function hashRoute(u: URL): string {
  return /^#!?\//.test(u.hash) ? u.hash : "";
}

/** origin+pathname+search（+hash 路由）：普通锚点导航不算换页。 */
export function pageUrlKey(url: string): string {
  try {
    const u = new URL(url);
    return `${u.origin}${u.pathname}${u.search}${hashRoute(u)}`;
  } catch {
    return url;
  }
}

/** origin+pathname（+hash 路由，忽略 query 与普通锚点）。 */
function pagePathKey(url: string): string {
  try {
    const u = new URL(url);
    return `${u.origin}${u.pathname}${hashRoute(u)}`;
  } catch {
    return url;
  }
}

/**
 * `from` → `to` 是否换页。
 *
 * - push / pop / document：query 不同即换页（`watch?v=A` → `watch?v=B`）。
 * - replace / tab：只有路径变化才算换页。replaceState 是站点改写当前条目（如 B 站加载后补
 *   `?vd_source=`），按 query 判定会把刚嗅探到的播放清单清掉且再也补不回来；tabs.onUpdated
 *   对同文档改写也报 `status: "loading"`，同样不能据 query 判定，query 级换页交给 content
 *   的 push / pop / document 上报。
 * - `from` 为空（首次登记）不算换页。
 */
export function isPageChange(kind: PageUrlChangeKind, from: string, to: string): boolean {
  if (!from || !to) return false;
  if (kind === "replace" || kind === "tab") return pagePathKey(from) !== pagePathKey(to);
  return pageUrlKey(from) !== pageUrlKey(to);
}
