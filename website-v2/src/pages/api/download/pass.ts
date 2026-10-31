/**
 * 官网浏览器下载的验证码通行证（见 src/lib/download-pass.ts）。
 *
 * - POST：ESA AI 验证码规则 `download-pass`（URI `fluxdown.com/api/download/pass`、仅 POST、无痕验证、
 *   拦截空 Token）在边缘验签，验签不过或不带 `captcha-verify-param` 直接 403，不回源；到这里的请求
 *   还须经 ESA 回源（viaEdge）且路径与规则完全一致（尾斜杠、大小写变体不在规则内），才签发 Cookie。
 * - GET：不在验证码规则内，告诉下载页点击前是否要先验证（没配置、非大陆、不经 ESA 或已有有效通行证时不要）。
 */
import type { APIRoute } from "astro";
import { DOWNLOAD_PASS_SECRET } from "astro:env/server";
import { getClientCountry, getClientIp } from "@/lib/client-ip";
import {
  DOWNLOAD_PASS_COOKIE,
  DOWNLOAD_PASS_TTL_SEC,
  issueDownloadPass,
  readDownloadPass,
  verifyDownloadPass,
  viaEdge,
} from "@/lib/download-pass";

export const prerender = false;

const PASS_PATH = "/api/download/pass";

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", "Cache-Control": "no-store", ...headers },
  });
}

export const GET: APIRoute = ({ request, clientAddress }) => {
  const secret = DOWNLOAD_PASS_SECRET;
  const required = !!secret && viaEdge(request) && getClientCountry(request) === "CN" &&
    !verifyDownloadPass(secret, readDownloadPass(request), getClientIp(request, clientAddress));
  return json(200, { required });
};

export const POST: APIRoute = ({ request, url, clientAddress }) => {
  const secret = DOWNLOAD_PASS_SECRET;
  if (!secret || !url.pathname.endsWith(PASS_PATH)) return json(404, { error: "Not found" });
  if (!viaEdge(request) || !request.headers.get("captcha-verify-param")) {
    return json(403, { error: "Captcha verification required" });
  }
  const token = issueDownloadPass(secret, getClientIp(request, clientAddress));
  const cookiePath = url.pathname.slice(0, -"/pass".length);
  const cookie = [
    `${DOWNLOAD_PASS_COOKIE}=${token}`,
    `Path=${cookiePath}`,
    `Max-Age=${DOWNLOAD_PASS_TTL_SEC}`,
    "HttpOnly",
    "SameSite=Lax",
    ...(import.meta.env.PROD ? ["Secure"] : []),
  ].join("; ");
  return json(200, { expires_in: DOWNLOAD_PASS_TTL_SEC }, { "Set-Cookie": cookie });
};
