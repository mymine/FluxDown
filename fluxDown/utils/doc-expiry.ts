/** 距过期不足此天数时提示「即将过期」。 */
const EXPIRY_WARN_DAYS = 7;
const DAY_MS = 24 * 60 * 60 * 1000;

export type DocExpiry = { state: "none" } | { state: "expired" } | { state: "ok" | "soon"; days: number };

/** 规则文档过期状态：社区包 / 吊销列表过期会让社区规则整体停用，UI 据此提前提示。 */
export function docExpiry(expiresAt: string | undefined, now: number): DocExpiry {
  if (expiresAt === undefined) return { state: "none" };
  const at = Date.parse(expiresAt);
  if (Number.isNaN(at)) return { state: "none" };
  if (at <= now) return { state: "expired" };
  const days = Math.ceil((at - now) / DAY_MS);
  return { state: days <= EXPIRY_WARN_DAYS ? "soon" : "ok", days };
}
