/**
 * 下载镜像（dl-cn，回源私有 OSS 按流量计费）的准入判定：只有「像正常客户端」的请求才拿镜像链接，
 * 其余一律交给 GitHub，不消耗镜像流量。
 *
 * 正常客户端下载前都会先读发布清单：官网下载页 / 首页 / 更新日志在浏览器里请求 /api/release
 * 或 /api/changelog，应用内更新（含不带 User-Agent 的旧版客户端）先查 /api/release 再下载。
 * 因此以「同一访客 IP 在 24 小时内读过发布清单」为准入条件；直接批量请求 /api/download 的
 * 脚本拿不到 CDN 链接。常见脚本 / HTTP 库的 User-Agent 即使读过清单也不准入。
 *
 * 状态在进程内存（单容器部署），重启后清空：期间的首次下载回落 GitHub，下次读清单后恢复。
 */

/** 读过发布清单后的准入时长：覆盖「检查到更新后隔几小时才点下载」。 */
const VIEWER_TTL_MS = 24 * 3600 * 1000;
/** 记录上限，超出时淘汰最早写入的条目。 */
const MAX_VIEWERS = 300_000;

const viewers = new Map<string, number>();

/** 发布清单接口调用：记录访客 IP。 */
export function markReleaseViewer(ip: string, now = Date.now()): void {
  if (!ip || ip === "unknown") return;
  // 先删再写，让最近访问的 IP 排到 Map 末尾，淘汰时从头部开始。
  viewers.delete(ip);
  viewers.set(ip, now + VIEWER_TTL_MS);
  while (viewers.size > MAX_VIEWERS) {
    const oldest = viewers.keys().next();
    if (oldest.done) break;
    viewers.delete(oldest.value);
  }
}

function recentViewer(ip: string, now: number): boolean {
  const until = viewers.get(ip);
  if (until === undefined) return false;
  if (until > now) return true;
  viewers.delete(ip);
  return false;
}

const SCRIPT_USER_AGENT =
  /curl|wget|python|aiohttp|httpx|go-http-client|java\/|okhttp|apache-httpclient|libwww|node-fetch|undici|axios|got\b|powershell|scrapy|headless|phantomjs|bot\b|spider|crawler/i;

/** 是否为常见脚本 / HTTP 库 / 爬虫的 User-Agent（空 UA 不算：旧版客户端的更新器不带 UA）。 */
export function isScriptUserAgent(userAgent: string | null): boolean {
  return !!userAgent && SCRIPT_USER_AGENT.test(userAgent);
}

/** 该请求能否使用 CDN 镜像。 */
export function mayUseMirror(ip: string, userAgent: string | null, now = Date.now()): boolean {
  return !isScriptUserAgent(userAgent) && recentViewer(ip, now);
}

/** 测试用：清空记录。 */
export function resetReleaseViewers(): void {
  viewers.clear();
}
