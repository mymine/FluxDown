/**
 * POST /api/cloud/share/public/<slug>/report — 分享落地页举报（同源代理）。
 *
 * 请求体 `{ reason, detail?, contact?, turnstileToken? }`：reason 必填 ≤ 64 字，detail ≤ 2000，contact ≤ 200，
 * turnstileToken 来自页面上的 Turnstile 小部件（清单带 turnstileSiteKey 时）。校验后转发 FluxCloud
 * `POST /api/v1/share/public/{slug}/report`（每 IP 5 次 / 小时），成功 204；失败透传状态码与
 * `{ code, message }`（404 / 410 / 403|400 turnstile_* / 429）。响应 no-store + `X-Robots-Tag: noindex`。
 * 契约与 IP 透传说明见 `src/lib/share-proxy.ts`。
 */

import type { APIRoute } from "astro";
import { getClientIp } from "@/lib/client-ip";
import { isValidSlug, MAX_REPORT_BODY, parseReportRequest } from "@/lib/share-landing";
import { failureResponse, reportShare, shareJson, shareNoContent } from "@/lib/share-proxy";

export const prerender = false;

export const POST: APIRoute = async ({ params, request, clientAddress }) => {
  const slug = params.slug ?? "";
  if (!isValidSlug(slug)) return shareJson({ code: "share_not_found" }, 404);

  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_REPORT_BODY) return shareJson({ code: "validation_error" }, 422);
  const parsed = parseReportRequest(await request.text());
  if (!parsed.ok) return shareJson({ code: "validation_error" }, 422);

  const { reason, detail, contact, turnstileToken } = parsed;
  const ip = getClientIp(request, clientAddress);
  const result = await reportShare(slug, { reason, detail, contact, turnstileToken }, ip);
  return result.ok ? shareNoContent() : failureResponse(result);
};
