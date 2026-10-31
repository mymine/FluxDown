/**
 * 官网浏览器下载的验证码通行证：浏览器先过 ESA AI 验证码（规则 `download-pass`，见 download-captcha.ts），
 * 再由 `POST /api/download/pass` 签发本 Cookie；`/api/download` 只给带有效通行证的浏览器 dl-cn 镜像
 * （download-gate.ts::needsDownloadPass），应用内更新不需要。
 *
 * 值为 `<到期 Unix 秒>.<HMAC-SHA256(secret, 到期 + 访客 IP) base64url>`：无状态、绑定访客 IP，
 * 脚本过一次验证码也不能把 Cookie 分发到别的出口使用。密钥为 `DOWNLOAD_PASS_SECRET`。
 */
import { createHmac, timingSafeEqual } from "node:crypto";

export const DOWNLOAD_PASS_COOKIE = "fd_dl_pass";
/** 通行证有效时长：覆盖一次下载页停留里连点几个文件与断点重试。 */
export const DOWNLOAD_PASS_TTL_SEC = 30 * 60;

const TOKEN = /^(\d{1,12})\.([A-Za-z0-9_-]{43})$/;

function sign(secret: string, expires: number, ip: string): string {
  return createHmac("sha256", secret).update(`${expires}\n${ip}`).digest("base64url");
}

/** 给访客 IP 签发通行证。 */
export function issueDownloadPass(secret: string, ip: string, now = Date.now()): string {
  const expires = Math.floor(now / 1000) + DOWNLOAD_PASS_TTL_SEC;
  return `${expires}.${sign(secret, expires, ip)}`;
}

/** 通行证未过期、签名正确且签发给同一访客 IP。 */
export function verifyDownloadPass(secret: string, token: string | null, ip: string, now = Date.now()): boolean {
  const match = token ? TOKEN.exec(token) : null;
  if (!match) return false;
  const expires = Number(match[1]);
  if (expires * 1000 <= now) return false;
  const expected = Buffer.from(sign(secret, expires, ip));
  const actual = Buffer.from(match[2]);
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}

/** 请求 Cookie 里的通行证（没有返回 null）。 */
export function readDownloadPass(request: Request): string | null {
  for (const part of request.headers.get("cookie")?.split(";") ?? []) {
    const eq = part.indexOf("=");
    if (eq > 0 && part.slice(0, eq).trim() === DOWNLOAD_PASS_COOKIE) return part.slice(eq + 1).trim();
  }
  return null;
}

/**
 * 请求是否经 ESA 回源（fluxdown.com）：反代按 `X-Fluxdown-Edge-Key` 鉴别结果写 `X-Fluxdown-Edge`
 * 并覆盖访客自带的同名头。只有经 ESA 的请求才过了边缘验签；旧域名（Cloudflare）与直连源站都不是。
 */
export function viaEdge(request: Request): boolean {
  return request.headers.get("x-fluxdown-edge") === "1";
}
