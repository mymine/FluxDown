/** 规则引擎使用的 URL 小工具（纯函数）。 */

/** 规范化 hostname：小写并去掉 FQDN 末尾的点（`example.com.` 与 `example.com` 等价）。 */
function normalizeHost(hostname: string): string {
  const h = hostname.toLowerCase();
  return h.endsWith(".") ? h.slice(0, -1) : h;
}

/** 扩展名合法形态：1–10 位小写字母 / 数字。 */
const EXT_RE = /^[a-z0-9]{1,10}$/;

/**
 * pathname 末段的扩展名（小写、不带点）：先去掉 `;jsessionid=…` 之类的路径参数，
 * 点号不能在段首 / 段尾，结果必须是 1–10 位字母数字，否则视为无扩展名。
 */
function extensionOfPath(pathname: string): string {
  let seg = pathname.slice(pathname.lastIndexOf("/") + 1);
  const semi = seg.indexOf(";");
  if (semi >= 0) seg = seg.slice(0, semi);
  const dot = seg.lastIndexOf(".");
  if (dot <= 0 || dot >= seg.length - 1) return "";
  const ext = seg.slice(dot + 1).toLowerCase();
  return EXT_RE.test(ext) ? ext : "";
}

/** 无法解析为 URL 时的兜底：直接在文本里找 `.ext` 后跟 `?` `#` `;` 或结尾。 */
const RAW_EXT_RE = /\.([a-zA-Z0-9]{1,10})(?:[?#;]|$)/;

export interface UrlParts {
  /** 小写 hostname，无末尾点；无法解析为空串 */
  host: string;
  /** pathname（未解码）；无法解析为空串 */
  path: string;
  /** path 扩展名（小写、不带点） */
  ext: string;
}

/** 一次 `new URL` 取出 host / path / ext（热路径用，避免各自重复解析）。 */
export function parseUrlParts(url: string): UrlParts {
  try {
    const u = new URL(url);
    return { host: normalizeHost(u.hostname), path: u.pathname, ext: extensionOfPath(u.pathname) };
  } catch {
    const m = url.match(RAW_EXT_RE);
    return { host: "", path: "", ext: m ? m[1].toLowerCase() : "" };
  }
}

/** 小写 hostname（去末尾点）；无法解析返回空串。 */
export function hostOf(url: string): string {
  try {
    return normalizeHost(new URL(url).hostname);
  } catch {
    return "";
  }
}

/** pathname（未解码）；无法解析返回空串。 */
export function pathOf(url: string): string {
  try {
    return new URL(url).pathname;
  } catch {
    return "";
  }
}

/**
 * 提取扩展名（小写、不带点）。
 *
 * 1. 取 pathname 末段扩展名（任意扩展名）。
 * 2. 仅当传入了 queryKeys 时继续：依次取这些参数的值，值须在 known 中；
 * 3. 再扫描所有 query 值，找以 `.ext` 结尾且 ext 在 known 中的值。
 *
 * 不传 queryKeys 时只看 path（与旧 extractExtension 一致）。
 */
export function extractExtension(
  url: string,
  queryKeys: readonly string[] = [],
  known?: ReadonlySet<string>,
): string {
  const fromPath = parseUrlParts(url).ext;
  if (fromPath || queryKeys.length === 0) return fromPath;

  let params: URLSearchParams;
  try {
    params = new URL(url).searchParams;
  } catch {
    return "";
  }
  const isKnown = (e: string): boolean => !known || known.has(e);

  for (const key of queryKeys) {
    const v = params.get(key);
    if (!v) continue;
    const e = v.toLowerCase().replace(/^\./, "");
    if (known?.has(e)) return e;
  }
  for (const v of params.values()) {
    const m = v.toLowerCase().match(/\.([a-z0-9]{1,10})$/);
    if (m && known && isKnown(m[1])) return m[1];
  }
  return "";
}
