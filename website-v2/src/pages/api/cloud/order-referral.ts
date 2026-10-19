/**
 * POST /api/cloud/order-referral — 网页购买推荐码即时校验（同源代理，只读不下单）。
 * 转发 FluxCloud `POST /api/v1/orders/web/referral/validate`（公开端点，per-IP 限频 +
 * 推荐码尝试配额）；IP 透传说明见 order-lookup.ts。
 */

import type { APIRoute } from "astro";
import { cloudBase, forwardJson } from "@/lib/cloud-proxy";
import { getClientIp } from "@/lib/client-ip";

export const prerender = false;

export const POST: APIRoute = async ({ request, clientAddress }) => {
  const base = cloudBase();
  if (!base) return new Response("Not Configured", { status: 502 });
  const body = await request.text();
  return forwardJson(`${base}/api/v1/orders/web/referral/validate`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Forwarded-For": getClientIp(request, clientAddress),
    },
    body,
  });
};
