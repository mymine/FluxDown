/**
 * dl-cn.fluxdown.com：Cloudflare 边缘缓存 + 私有阿里云 OSS 回源（ICP 备案前的大陆下载镜像）。
 *
 * 官网 /api/download 只把准入的中国大陆访客 302 到这里（见 src/pages/api/download/[filename].ts），
 * 链接鉴权沿用阿里云 CDN「URL 鉴权 A 方式」（src/lib/oss.ts::cdnDownloadUrl 签发），
 * 备案后 dl-cn 改回阿里云 CDN（中国内地加速）时官网不用改。
 *
 * - 只放行 GET / HEAD 与 `${OSS_RELEASE_PREFIX}/<version>/<component>/<asset>`；桶保持私有。
 * - `auth_key=<签发时间>-<rand>-<uid>-<md5>`，md5 串为 `<请求路径>-<签发时间>-<rand>-<uid>-<CN_CDN_AUTH_KEY>`；
 *   签发时间 + DOWNLOAD_URL_TTL_SEC 之后 403。缺失、格式错误或签名不符一律 403。
 * - 回源 URL 用固定过期时间签名（V1 query 签名）：同一对象的回源 URL 恒定，CF 缓存键稳定，
 *   auth_key 不进缓存键；签名 URL 只在 CF 与 OSS 之间出现，不下发给客户端。
 * - 发布资产按版本目录不可变，边缘缓存 30 天；Range 由 CF 缓存层切片。
 * - `.apk` / `.ipa` 在桶内追加 `.bin` 存储（OSS 默认域名禁止分发它们），对外仍是原名，
 *   响应覆盖 Content-Type / Content-Disposition，见 src/lib/oss-storage.ts。
 *
 * 部署：cd website-v2/cdn && bunx wrangler deploy（见同目录 wrangler.toml）；密钥
 * OSS_ACCESS_KEY_ID / OSS_ACCESS_KEY_SECRET / CN_CDN_AUTH_KEY 用 `bunx wrangler secret put` 注入，
 * CN_CDN_AUTH_KEY 必须与官网 .env 同值。
 */

import { ossStorageKey, renamedDownloadHeaders } from "../src/lib/oss-storage";

// 2100-01-01T00:00:00Z：回源签名恒定，保证缓存键稳定。
const SIGN_EXPIRES = 4102444800;
const EDGE_TTL = 30 * 24 * 3600;
const BROWSER_TTL = 24 * 3600;
// 签发时间允许比边缘时钟超前的秒数（官网服务器与 CF 的时钟偏差）。
const CLOCK_SKEW_SEC = 300;
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
  async fetch(request, env) {
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
      segments[0] !== env.OSS_RELEASE_PREFIX ||
      !/^v[A-Za-z0-9][A-Za-z0-9._-]*$/.test(segments[1]) ||
      !/^[a-z]+$/.test(segments[2]) ||
      segments.some((s) => s === "" || s === "." || s === ".." || /[\\\u0000-\u001f\u007f]/.test(s))
    ) {
      return new Response("Not Found", { status: 404 });
    }
    if (!(await verifyAuthKey(env, url.pathname, url.searchParams.get("auth_key")))) {
      return new Response("Forbidden", { status: 403, headers: { "cache-control": "no-store" } });
    }

    let upstream;
    const controller = new AbortController();
    // 只限制响应头等待，不能在大文件开始传输 15s 后中断其响应体。
    const timeout = setTimeout(() => controller.abort(), 15000);
    try {
      const origin = await presign(env, ossStorageKey(key));
      const headers = new Headers();
      for (const name of ["range", "if-range"]) {
        const value = request.headers.get(name);
        if (value) headers.set(name, value);
      }
      upstream = await fetch(origin, {
        method: "GET",
        headers,
        redirect: "manual",
        signal: controller.signal,
        cf: { cacheEverything: true, cacheTtlByStatus: { "200-299": EDGE_TTL, "300-599": -1 } },
      });
    } catch (error) {
      console.error("OSS fetch failed", error);
      return failure(502);
    } finally {
      clearTimeout(timeout);
    }

    if (!upstream.ok) {
      await releaseBody(upstream);
      if (upstream.status === 416) {
        const headers = new Headers({ "cache-control": "no-store" });
        const range = upstream.headers.get("content-range");
        if (range) headers.set("content-range", range);
        return new Response(null, { status: 416, headers });
      }
      return failure(upstream.status === 403 || upstream.status === 404 ? 404 : 502);
    }
    const out = new Headers();
    for (const name of PASS_HEADERS) {
      const value = upstream.headers.get(name);
      if (value !== null) out.set(name, value);
    }
    const renamed = renamedDownloadHeaders(key);
    if (renamed) {
      out.set("content-type", renamed.contentType);
      out.set("content-disposition", renamed.contentDisposition);
    }
    // 链接带时效签名，浏览器缓存不跨链接复用；仍允许同一链接的分段重试命中本地缓存。
    out.set("cache-control", `private, max-age=${BROWSER_TTL}, immutable`);
    const cacheStatus = upstream.headers.get("cf-cache-status");
    if (cacheStatus) out.set("x-edge-cache", cacheStatus);
    if (request.method === "HEAD") await releaseBody(upstream);
    return new Response(request.method === "HEAD" ? null : upstream.body, {
      status: upstream.status,
      headers: out,
    });
  },
};

/** 阿里云 CDN URL 鉴权 A 方式校验；`path` 为请求里的原始（已编码）路径，与官网签名输入一致。 */
async function verifyAuthKey(env, path, authKey) {
  const secret = env.CN_CDN_AUTH_KEY;
  const ttl = Number(env.DOWNLOAD_URL_TTL_SEC);
  if (!secret || !Number.isSafeInteger(ttl) || ttl <= 0 || !authKey) return false;
  const match = /^(\d{1,12})-([0-9A-Za-z]{1,64})-([0-9A-Za-z]{1,32})-([0-9a-f]{32})$/.exec(authKey);
  if (!match) return false;
  const [, ts, rand, uid, hash] = match;
  const issued = Number(ts);
  const now = Math.floor(Date.now() / 1000);
  if (issued > now + CLOCK_SKEW_SEC || now > issued + ttl) return false;
  const digest = await crypto.subtle.digest(
    "MD5",
    new TextEncoder().encode(`${path}-${ts}-${rand}-${uid}-${secret}`),
  );
  const expected = new TextEncoder().encode(
    [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join(""),
  );
  return crypto.subtle.timingSafeEqual(expected, new TextEncoder().encode(hash));
}

function failure(status) {
  // 不透出 OSS 错误体、签名或 Location。官网 302 前已确认资产存在，这里不回退 GitHub。
  return new Response(status === 404 ? "Not Found" : "Bad Gateway", {
    status,
    headers: { "cache-control": "no-store" },
  });
}

async function releaseBody(response) {
  try {
    await response.body?.cancel();
  } catch (error) {
    console.warn("Unable to cancel OSS response body", error);
  }
}

async function presign(env, key) {
  const stringToSign = `GET\n\n\n${SIGN_EXPIRES}\n/${env.OSS_BUCKET}/${key}`;
  const hmacKey = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(env.OSS_ACCESS_KEY_SECRET),
    { name: "HMAC", hash: "SHA-1" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign("HMAC", hmacKey, new TextEncoder().encode(stringToSign));
  const signature = btoa(String.fromCharCode(...new Uint8Array(mac)));
  const path = key.split("/").map(encodeURIComponent).join("/");
  const query = new URLSearchParams({
    OSSAccessKeyId: env.OSS_ACCESS_KEY_ID,
    Expires: String(SIGN_EXPIRES),
    Signature: signature,
  });
  return `https://${env.OSS_BUCKET}.${env.OSS_ENDPOINT}/${path}?${query}`;
}
