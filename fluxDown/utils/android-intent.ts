/**
 * Android Intent URI（`intent:…#Intent;…;end`）构造：Android 版浏览器没有 Native Messaging，
 * 扩展经 Intent URI 显式指定 FluxDown 包名唤起 App，不依赖 `fluxdown://` 自定义协议。
 *
 * 数据部分保持原始链接（http / https / magnet / ed2k），命中 App `ExternalDownloadActivity`
 * 已有的 VIEW 过滤器；Cookie / 来源页 / 请求头 / 文件名作为字符串 extra 随 Intent 投递，
 * 键名与 `mobile/Android/app/.../ExternalDownloadActivity.kt` 读取的键一致。
 * 浏览器解析规则（Android `Intent.parseUri(URI_INTENT_SCHEME)`）：`S.<键>=<值>` 的键与值都经
 * `Uri.decode` 解码（不把 `+` 当空格），因此一律用 `encodeURIComponent` 编码。
 *
 * 刻意不带 `S.browser_fallback_url`：Chrome（`ExternalNavigationHandler`）在导航没有用户手势时、
 * Firefox（`AppLinksUseCases`）在「在应用中打开链接」不是「总是」时，只要有回退地址就直接导航过去
 * 而不唤起 App——扩展发起的导航正好落在这两种情况，装了 App 也会被带离当前页。
 * 不带回退地址时，两者都改为询问用户是否打开 FluxDown。
 */

/** FluxDown Android 的 applicationId（`mobile/Android/app/build.gradle.kts`）。 */
export const ANDROID_PACKAGE = "com.fluxdown.app";

/** Intent extra 键。前三个与 X / Via 等浏览器调用外部下载器时的键相同。 */
export const ANDROID_EXTRA = {
  userAgent: "User-Agent",
  cookie: "Cookie",
  referer: "Referer",
  fileName: "com.fluxdown.extra.FILE_NAME",
  /** 完整请求头，JSON 对象（`{"名字":"值"}`）。 */
  headers: "com.fluxdown.extra.HEADERS",
} as const;

export interface AndroidHandoffRequest {
  url: string;
  filename?: string;
  cookies?: string;
  referrer?: string;
  headers?: Record<string, string>;
}

/** App 的 VIEW 过滤器接收的 scheme（ftp 等其余 scheme 没有过滤器，不能经 Intent 投递）。 */
const SCHEME_PATTERN = /^(https?|magnet|ed2k):/i;

function extra(key: string, value: string): string {
  return `S.${encodeURIComponent(key)}=${encodeURIComponent(value)}`;
}

function findHeader(headers: Record<string, string>, name: string): string | undefined {
  const wanted = name.toLowerCase();
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() === wanted) return value;
  }
  return undefined;
}

/**
 * 构造唤起 FluxDown App 的 Intent URI；链接 scheme 不受 App 支持时返回 null。
 *
 * http(s) 链接的片段（`#…`）不发往服务器，直接丢弃；magnet / ed2k 中的 `#` 转义为 `%23`，
 * 避免与 `#Intent;` 分隔符混淆。
 */
export function buildAndroidIntentUrl(request: AndroidHandoffRequest): string | null {
  const url = request.url.trim();
  const match = SCHEME_PATTERN.exec(url);
  if (!match) return null;
  const scheme = match[1].toLowerCase();
  let rest = url.slice(match[0].length);
  if (scheme === "http" || scheme === "https") {
    const hash = rest.indexOf("#");
    if (hash >= 0) rest = rest.slice(0, hash);
    if (!rest.startsWith("//") || rest.length <= 2) return null;
  } else {
    rest = rest.replaceAll("#", "%23");
  }

  const headers = request.headers ?? {};
  const parts = [`scheme=${scheme}`, "action=android.intent.action.VIEW", `package=${ANDROID_PACKAGE}`];
  const filename = request.filename?.trim();
  if (filename) parts.push(extra(ANDROID_EXTRA.fileName, filename));
  if (request.cookies) parts.push(extra(ANDROID_EXTRA.cookie, request.cookies));
  if (request.referrer) parts.push(extra(ANDROID_EXTRA.referer, request.referrer));
  const userAgent = findHeader(headers, "User-Agent");
  if (userAgent) parts.push(extra(ANDROID_EXTRA.userAgent, userAgent));
  if (Object.keys(headers).length > 0) parts.push(extra(ANDROID_EXTRA.headers, JSON.stringify(headers)));
  return `intent:${rest}#Intent;${parts.join(";")};end`;
}
