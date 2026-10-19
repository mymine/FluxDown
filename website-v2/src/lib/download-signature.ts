/**
 * Cloudflare 下载 Worker 的链接签名：官网签发、Worker（cdn/oss-proxy.js）校验，两端共用本文件。
 *
 * `sig = base64url(HMAC-SHA256(secret, "<path>\n<exp>"))`。`path` 是 URL 里按段编码后的
 * 路径（含前导 `/`，与浏览器实际请求的 pathname 一致），`exp` 为 Unix 秒级过期时间。
 * 签名只保护「谁能拿到可用链接」，不参与 Worker 回源缓存键，签名参数不会打散边缘缓存。
 */

/** 签发方配置失误时的兜底：拒绝离现在超过 24 小时的 exp。 */
export const MAX_DOWNLOAD_URL_TTL_SEC = 24 * 3600;

const encoder = new TextEncoder();

export async function downloadSignature(secret: string, path: string, exp: number): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, encoder.encode(`${path}\n${exp}`)));
  return btoa(String.fromCharCode(...mac)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function verifyDownloadSignature(
  secret: string,
  path: string,
  exp: string | null,
  sig: string | null,
  nowSec = Math.floor(Date.now() / 1000),
): Promise<boolean> {
  if (!exp || !/^\d{1,12}$/.test(exp) || !sig) return false;
  const expires = Number(exp);
  if (expires < nowSec || expires - nowSec > MAX_DOWNLOAD_URL_TTL_SEC) return false;
  const expected = await downloadSignature(secret, path, expires);
  if (expected.length !== sig.length) return false;
  let diff = 0;
  for (let i = 0; i < expected.length; i++) diff |= expected.charCodeAt(i) ^ sig.charCodeAt(i);
  return diff === 0;
}
