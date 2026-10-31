/**
 * 客户端 IP：服务器反代（1Panel OpenResty）统一算好后写入 `X-Client-IP` 并覆盖访客自带的同名头——
 * 经阿里云 ESA（fluxdown.com / www）且回源带对的鉴别头时取 ESA 注入的 `Ali-Real-Client-Ip`，
 * 经 Cloudflare 网段（旧域名 fluxdown.zerx.dev）取 `CF-Connecting-IP`，其余取 TCP 对端。
 * 规则在服务器 `conf.d/fluxdown-client-ip.conf` + `conf.d/fluxdown-edge.conf`。
 * 不采用 X-Forwarded-For 首值（CDN 对已有 XFF 是追加，首值客户端可控）；缺头时回落到 clientAddress。
 */
export function getClientIp(request: Request, clientAddress?: string | null): string {
  const ip = request.headers.get("x-client-ip")?.trim();
  if (ip && ip.length <= 45 && /^[0-9a-fA-F:.]+$/.test(ip)) return ip;
  return clientAddress || "unknown";
}

/**
 * 访客国家/地区（大写 ISO 3166-1 两位码）：反代只采信边缘注入的国家码写进 `X-Client-Country`
 * （覆盖访客自带的同名头）——经 ESA 鉴别时取 `Ali-Ip-Country`（小写，这里转大写），经 Cloudflare 网段
 * 时取 `CF-IPCountry`，其余为空。均按访客真实 IP（IPv4 / IPv6）判定，港澳台分别为 HK / MO / TW；
 * 未知（XX）、Tor（T1）与缺头返回 null。规则在服务器 `conf.d/fluxdown-client-ip.conf` + 各站点 `proxy/download.conf`。
 */
export function getClientCountry(request: Request): string | null {
  const country = request.headers.get("x-client-country")?.trim().toUpperCase();
  return country && /^[A-Z]{2}$/.test(country) && country !== "XX" ? country : null;
}
