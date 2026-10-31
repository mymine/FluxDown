/**
 * dl-cn.fluxdown.com：阿里云 ESA 边缘函数（站点 fluxdown.com，中国内地加速）——大陆下载镜像入口。
 *
 * 官网 /api/download 只把准入的中国大陆访客 302 到这里（见 src/pages/api/download/[filename].ts），
 * 链接鉴权沿用阿里云「URL 鉴权 A 方式」（src/lib/oss.ts::cdnDownloadUrl 签发）：
 * - `auth_key=<签发时间>-<rand>-<uid>-<md5>`，md5 串为 `<请求路径>-<签发时间>-<rand>-<uid>-<CN_CDN_AUTH_KEY>`；
 *   签发时间 + DOWNLOAD_URL_TTL_SEC 之后 403。缺失、格式错误或签名不符一律 403。
 * - 校验通过后以不带查询串的同域地址 fetch：ESA 按站点记录回源私有 OSS（STS 授权），
 *   缓存规则 `dl-cn-download-cache` 忽略查询串、边缘缓存 30 天，签名不影响命中率。
 * - `.apk` / `.ipa` 在桶内存为 `<name>.bin`（OSS 默认域名禁止分发这两类），回源时换成存储键并覆盖
 *   Content-Type / Content-Disposition。存储键规则与 src/lib/oss-storage.ts::ossStorageKey、
 *   .github/actions/oss-upload 的 bash 一致，改一处须同步另两处（ESA 控制台单文件部署，不能 import 仓库模块）。
 *
 * 部署：ESA 控制台「函数和 Pages」函数 `fluxdown-dl-cn` 粘贴本文件 → 生成版本 → 发布生产环境；
 * 函数路由 `dl-cn.fluxdown.com/*`。函数变量（生产环境）：CN_CDN_AUTH_KEY（加密存储，与官网 .env 同值）、
 * DOWNLOAD_URL_TTL_SEC（必须等于 src/lib/oss.ts 的 DOWNLOAD_URL_TTL_SEC）。
 */

import { createHash } from "node:crypto";

const RELEASE_PREFIX = "FluxDownRelease";
// 签发时间允许比边缘时钟超前的秒数（官网服务器与边缘节点的时钟偏差）。
const CLOCK_SKEW_SEC = 300;
const BROWSER_TTL = 24 * 3600;
const STORAGE_SUFFIX = ".bin";
const BLOCKED_TYPES = {
  ".apk": "application/vnd.android.package-archive",
  ".ipa": "application/octet-stream",
};
const PASS_HEADERS = [
  "content-type",
  "content-length",
  "content-range",
  "content-disposition",
  "accept-ranges",
  "etag",
  "last-modified",
];

export default {
  async fetch(request, context, env) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method Not Allowed", { status: 405, headers: { Allow: "GET, HEAD" } });
    }
    const url = new URL(request.url);
    let key;
    try {
      key = decodeURIComponent(url.pathname.slice(1));
    } catch {
      return new Response("Bad Request", { status: 400 });
    }
    const segments = key.split("/");
    if (
      segments.length !== 4 ||
      segments[0] !== RELEASE_PREFIX ||
      !/^v[A-Za-z0-9][A-Za-z0-9._-]*$/.test(segments[1]) ||
      !/^[a-z]+$/.test(segments[2]) ||
      segments.some((s) => s === "" || s === "." || s === ".." || /[\\\u0000-\u001f\u007f]/.test(s))
    ) {
      return new Response("Not Found", { status: 404 });
    }
    if (!verifyAuthKey(env, url.pathname, url.searchParams.get("auth_key"))) {
      return new Response("Forbidden", { status: 403, headers: { "cache-control": "no-store" } });
    }

    const ext = blockedExtension(key);
    const storagePath = ext ? `${url.pathname}${STORAGE_SUFFIX}` : url.pathname;
    const headers = new Headers();
    for (const name of ["range", "if-range"]) {
      const value = request.headers.get(name);
      if (value) headers.set(name, value);
    }
    let upstream;
    try {
      upstream = await fetch(`${url.origin}${storagePath}`, {
        method: request.method,
        headers,
        redirect: "manual",
      });
    } catch (error) {
      console.error("origin fetch failed", error);
      return failure(502);
    }
    if (!upstream.ok) {
      if (upstream.status === 416) {
        const out = new Headers({ "cache-control": "no-store" });
        const range = upstream.headers.get("content-range");
        if (range) out.set("content-range", range);
        return new Response(null, { status: 416, headers: out });
      }
      return failure(upstream.status === 403 || upstream.status === 404 ? 404 : 502);
    }

    const out = new Headers();
    for (const name of PASS_HEADERS) {
      const value = upstream.headers.get(name);
      if (value !== null) out.set(name, value);
    }
    if (ext) {
      const name = key.slice(key.lastIndexOf("/") + 1);
      const ascii = name.replace(/[^\x20-\x7e]|["\\]/g, "_");
      out.set("content-type", BLOCKED_TYPES[ext]);
      out.set(
        "content-disposition",
        `attachment; filename="${ascii}"; filename*=UTF-8''${encodeURIComponent(name)}`,
      );
    }
    // 链接带时效签名，浏览器缓存不跨链接复用；仍允许同一链接的分段重试命中本地缓存。
    out.set("cache-control", `private, max-age=${BROWSER_TTL}, immutable`);
    return new Response(request.method === "HEAD" ? null : upstream.body, {
      status: upstream.status,
      headers: out,
    });
  },
};

function blockedExtension(key) {
  const lower = key.toLowerCase();
  return Object.keys(BLOCKED_TYPES).find((ext) => lower.endsWith(ext)) ?? null;
}

/** 阿里云 URL 鉴权 A 方式校验；`path` 为请求里的原始（已编码）路径，与官网签名输入一致。 */
function verifyAuthKey(env, path, authKey) {
  const secret = env.CN_CDN_AUTH_KEY;
  const ttl = Number(env.DOWNLOAD_URL_TTL_SEC);
  if (!secret || !Number.isSafeInteger(ttl) || ttl <= 0 || !authKey) return false;
  const match = /^(\d{1,12})-([0-9A-Za-z]{1,64})-([0-9A-Za-z]{1,32})-([0-9a-f]{32})$/.exec(authKey);
  if (!match) return false;
  const [, ts, rand, uid, hash] = match;
  const issued = Number(ts);
  const now = Math.floor(Date.now() / 1000);
  if (issued > now + CLOCK_SKEW_SEC || now > issued + ttl) return false;
  const expected = createHash("md5").update(`${path}-${ts}-${rand}-${uid}-${secret}`).digest("hex");
  let diff = 0;
  for (let i = 0; i < 32; i++) diff |= expected.charCodeAt(i) ^ hash.charCodeAt(i);
  return diff === 0;
}

function failure(status) {
  // 不透出源站错误体或 Location。官网 302 前已确认资产存在，这里不回退 GitHub。
  return new Response(status === 404 ? "Not Found" : "Bad Gateway", {
    status,
    headers: { "cache-control": "no-store" },
  });
}
