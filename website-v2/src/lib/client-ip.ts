/**
 * 客户端 IP：服务器反代（1Panel OpenResty）统一算好后写入 `X-Client-IP` 并覆盖访客自带的同名头——
 * 经 Cloudflare 网段（fluxdown.com / www 与旧域名）取 `CF-Connecting-IP`，其余取 TCP 对端。
 * 规则在服务器 `conf.d/fluxdown-client-ip.conf`。
 * 不采用 X-Forwarded-For 首值（CDN 对已有 XFF 是追加，首值客户端可控）；缺头时回落到 clientAddress。
 */
export function getClientIp(request: Request, clientAddress?: string | null): string {
  const ip = request.headers.get("x-client-ip")?.trim();
  if (ip && ip.length <= 45 && /^[0-9a-fA-F:.]+$/.test(ip)) return ip;
  return clientAddress || "unknown";
}
