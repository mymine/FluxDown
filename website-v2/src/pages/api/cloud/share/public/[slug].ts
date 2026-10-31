/**
 * GET /api/cloud/share/public/<slug> — FluxMesh分享清单（同源代理）。
 *
 * 转发 FluxCloud `GET /api/v1/share/public/{slug}`（公开端点，按访客 IP 限频），校验并裁剪后返回；
 * 失败透传状态码与 `{ code, message }`。响应 no-store + `X-Robots-Tag: noindex`。
 * 契约与 IP 透传说明见 `src/lib/share-proxy.ts`。
 */

import type { APIRoute } from "astro";
import { getClientIp } from "@/lib/client-ip";
import { failureResponse, lookupShare, shareJson } from "@/lib/share-proxy";

export const prerender = false;

export const GET: APIRoute = async ({ params, request, clientAddress }) => {
  const lookup = await lookupShare(params.slug ?? "", getClientIp(request, clientAddress));
  return lookup.ok ? shareJson(lookup.manifest) : failureResponse(lookup);
};
