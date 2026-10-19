/**
 * dl.fluxdown.com（旧域名 dl-fluxdown.zerx.dev 同指本 Worker）：Cloudflare 边缘缓存 + 私有阿里云 OSS 回源。
 *
 * 官网 source=auto 与旧客户端 / 应用内更新 302 到这里。
 * 命中边缘缓存不计 OSS 外网流出；auto 链在 OSS 失败时回退精确 GitHub release asset。
 *
 * - 只放行 GET / HEAD 与 `${PREFIX}/<version>/<component>/<asset>`；桶保持私有。
 * - 回源 URL 用固定过期时间签名（V1 query 签名）：同一对象的回源 URL 恒定，
 *   CF 缓存键稳定；签名 URL 只在 CF 与 OSS 之间出现，不下发给客户端。
 * - 发布资产按版本目录不可变，边缘缓存 30 天；Range 由 CF 缓存层切片。
 * - `.apk` / `.ipa` 在桶内追加 `.bin` 存储（OSS 默认域名禁止分发它们），对外仍是原名，
 *   响应覆盖 Content-Type / Content-Disposition，见 src/lib/oss-storage.ts。
 *
 * 部署：cd website-v2/cdn && bunx wrangler deploy（见同目录 wrangler.toml），密钥
 * OSS_ACCESS_KEY_ID / OSS_ACCESS_KEY_SECRET 用 `bunx wrangler secret put` 注入。
 */

import { ossStorageKey, renamedDownloadHeaders } from "../src/lib/oss-storage";

// 2100-01-01T00:00:00Z：回源签名恒定，保证缓存键稳定。
const SIGN_EXPIRES = 4102444800;
const EDGE_TTL = 30 * 24 * 3600;
const BROWSER_TTL = 24 * 3600;
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

    let fallback = null;
    if (url.searchParams.has("fallback")) {
      const tag = url.searchParams.get("tag") ?? "";
      const match = /^(?:([a-z]+)-)?(v[A-Za-z0-9][A-Za-z0-9._-]*)$/.exec(tag);
      if (url.searchParams.get("fallback") !== "github" || !match ||
        match[2] !== segments[1] || (match[1] ?? "app") !== segments[2]) {
        return new Response("Bad Request", { status: 400, headers: { "cache-control": "no-store" } });
      }
      // 仓库来自部署配置而非请求；tag 必须精确对应对象路径，禁止任意回退 URL。
      if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(env.GITHUB_REPO ?? "")) {
        return new Response("Bad Gateway", { status: 502, headers: { "cache-control": "no-store" } });
      }
      fallback = `https://github.com/${env.GITHUB_REPO}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(segments[3])}`;
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
    } catch {
      return originFailure(fallback, 502);
    } finally {
      clearTimeout(timeout);
    }

    if (!upstream.ok) {
      await releaseBody(upstream);
      // 请求范围越界不是源站故障，不能跳转 GitHub 掩盖 416。
      if (upstream.status === 416) {
        const headers = new Headers({ "cache-control": "no-store" });
        const range = upstream.headers.get("content-range");
        if (range) headers.set("content-range", range);
        return new Response(null, { status: 416, headers });
      }
      return originFailure(fallback, upstream.status === 403 || upstream.status === 404 ? 404 : 502);
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
    out.set("cache-control", `public, max-age=${BROWSER_TTL}, immutable`);
    const cacheStatus = upstream.headers.get("cf-cache-status");
    if (cacheStatus) out.set("x-edge-cache", cacheStatus);
    if (request.method === "HEAD") await releaseBody(upstream);
    return new Response(request.method === "HEAD" ? null : upstream.body, {
      status: upstream.status,
      headers: out,
    });
  },
};

function originFailure(fallback, status) {
  if (fallback) {
    return new Response(null, {
      status: 302,
      headers: { location: fallback, "cache-control": "no-store", "x-download-source": "github" },
    });
  }
  // 不透出 OSS 错误体、签名或 Location。
  return new Response(status === 404 ? "Not Found" : "Bad Gateway", {
    status,
    headers: { "cache-control": "no-store" },
  });
}

async function releaseBody(response) {
  try {
    await response.body?.cancel();
  } catch (error) {
    // 已断开的上游流仍必须走既定 HTTP 回退，而不能把取消失败返回给客户端。
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
