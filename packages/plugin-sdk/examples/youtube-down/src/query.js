// 查询串工具：QuickJS 没有 URL / URLSearchParams，googlevideo 直链与 signatureCipher 都靠这里解析。

function decode(text) {
  try {
    return decodeURIComponent(text.replace(/\+/g, " "));
  } catch {
    return text;
  }
}

/** 解析 `a=1&b=2`（`+` 视为空格，同名取首个，非法转义保留原文）。 */
export function parseQuery(query) {
  const out = Object.create(null);
  for (const part of String(query || "").split("&")) {
    if (!part) continue;
    const eq = part.indexOf("=");
    const key = decode(eq < 0 ? part : part.slice(0, eq));
    if (!(key in out)) out[key] = decode(eq < 0 ? "" : part.slice(eq + 1));
  }
  return out;
}

function splitUrl(url) {
  const hash = url.indexOf("#");
  const base = hash < 0 ? url : url.slice(0, hash);
  const q = base.indexOf("?");
  return { path: q < 0 ? base : base.slice(0, q), query: q < 0 ? "" : base.slice(q + 1) };
}

/** 取 URL 查询参数（无则 undefined）。 */
export function urlParam(url, name) {
  return parseQuery(splitUrl(String(url)).query)[name];
}

/** 设置 URL 查询参数：已有同名参数原位替换首个，否则追加。值按 encodeURIComponent 编码。 */
export function setUrlParam(url, name, value) {
  const { path, query } = splitUrl(String(url));
  const pair = `${encodeURIComponent(name)}=${encodeURIComponent(value)}`;
  const parts = query ? query.split("&") : [];
  const at = parts.findIndex((p) => decode(p.split("=", 1)[0]) === name);
  if (at >= 0) parts[at] = pair;
  else parts.push(pair);
  return `${path}?${parts.join("&")}`;
}
