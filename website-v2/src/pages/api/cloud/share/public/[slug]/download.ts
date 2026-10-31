/**
 * POST /api/cloud/share/public/<slug>/download — 申请签名 CDN 下载链接（同源代理）。
 *
 * 请求体 `{ fileIndex, turnstileToken? }`：fileIndex 为非负整数，turnstileToken 来自页面上的
 * Cloudflare Turnstile 小部件（清单带 turnstileSiteKey 时）。校验后转发 FluxCloud
 * `POST /api/v1/share/public/{slug}/download`，成功返回 `{ url, expiresAt }`，失败透传状态码与
 * `{ code, message }`（404 / 410 / 409 / 403 traffic_exhausted / turnstile_* / 429）。
 * 响应 no-store + `X-Robots-Tag: noindex`。契约与 IP 透传说明见 `src/lib/share-proxy.ts`。
 */

import type { APIRoute } from "astro";
import { getClientIp } from "@/lib/client-ip";
import { isValidSlug, MAX_DOWNLOAD_BODY, parseDownloadRequest } from "@/lib/share-landing";
import { failureResponse, requestShareDownload, shareJson } from "@/lib/share-proxy";

export const prerender = false;

export const POST: APIRoute = async ({ params, request, clientAddress }) => {
  const slug = params.slug ?? "";
  if (!isValidSlug(slug)) return shareJson({ code: "share_not_found" }, 404);

  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_DOWNLOAD_BODY) return shareJson({ code: "validation_error" }, 422);
  const parsed = parseDownloadRequest(await request.text());
  if (!parsed.ok) return shareJson({ code: "validation_error" }, 422);

  const result = await requestShareDownload(
    slug,
    parsed.fileIndex,
    parsed.turnstileToken,
    getClientIp(request, clientAddress),
  );
  return result.ok ? shareJson(result.grant) : failureResponse(result);
};
