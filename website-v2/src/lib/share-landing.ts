/**
 * FluxMesh分享落地页（/s/<slug>）的纯函数：输入解析、清单校验、状态选择、格式化。
 *
 * 不依赖 astro:*，页面、React 岛与 `src/lib/share-proxy.ts` 共用，tests/share-landing.test.ts 直接覆盖。
 * 契约：FluxCloud `GET /api/v1/share/public/{slug}` 与 `POST .../download`（见 share-proxy.ts）。
 */
import type { Lang } from "@/i18n/config";

/** FluxCloud slug：10–12 位 base64url 字符（server/src/share/codes.rs::is_valid_slug）。 */
const SLUG_RE = /^[A-Za-z0-9_-]{10,12}$/;
const PICKUP_CODE_RE = /^\d{6}$/;
/** 上游文件索引的合理上界，仅用于拒绝明显的垃圾输入。 */
const MAX_FILE_INDEX = 1_000_000;
/** Cloudflare Turnstile token 上限 2048 字符。 */
export const MAX_TURNSTILE_TOKEN = 2048;

export const SHARE_ROBOTS = "noindex, nofollow";

export function isValidSlug(value: unknown): value is string {
  return typeof value === "string" && SLUG_RE.test(value);
}

export function isFileIndex(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= MAX_FILE_INDEX;
}

/** 客户端深链：`fluxdown://share/<slug>`。 */
export function deepLink(slug: string): string {
  return `fluxdown://share/${slug}`;
}

export type ShareInput =
  | { kind: "slug"; slug: string }
  | { kind: "code" }
  | { kind: "invalid" };

/**
 * 解析 /s/ 入口输入框：裸 slug、`https://fluxdown.com/s/<slug>`（含 /zh 前缀、无协议写法）、
 * `fluxdown://share/<slug>`。6 位数字是取件码，只能在客户端输入，这里不解析成 slug。
 */
export function parseShareInput(raw: string): ShareInput {
  const text = raw.trim();
  if (PICKUP_CODE_RE.test(text)) return { kind: "code" };
  if (SLUG_RE.test(text)) return { kind: "slug", slug: text };
  const deep = text.match(/^fluxdown:\/\/share\/([A-Za-z0-9_-]+)\/?(?:[?#].*)?$/i);
  if (deep) {
    if (SLUG_RE.test(deep[1])) return { kind: "slug", slug: deep[1] };
    return PICKUP_CODE_RE.test(deep[1]) ? { kind: "code" } : { kind: "invalid" };
  }
  const link = text.match(/^(?:https?:\/\/)?[^/\s?#]+\/(?:zh\/)?s\/([A-Za-z0-9_-]{10,12})\/?(?:[?#].*)?$/i);
  if (link) return { kind: "slug", slug: link[1] };
  return { kind: "invalid" };
}

export interface ShareFile {
  index: number;
  name: string;
  size: number;
}

export interface ShareManifest {
  id: string;
  slug: string;
  title: string;
  files: ShareFile[];
  totalBytes: number;
  expiresAt: string;
  cloudReady: boolean;
  /** 当前 6 位取件码（分享有效时上游才给）。 */
  pickupCode?: string;
  turnstileSiteKey?: string;
}

export interface ShareGrant {
  url: string;
  expiresAt: string;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 下载代理请求体上限：`{ fileIndex, turnstileToken }` 远小于此。 */
export const MAX_DOWNLOAD_BODY = 4096;

export type DownloadRequest =
  | { ok: true; fileIndex: number; turnstileToken?: string }
  | { ok: false };

/** 校验 `POST …/download` 请求体：fileIndex 为合法非负整数，turnstileToken 可选且 ≤ 2048 字符。 */
export function parseDownloadRequest(text: string): DownloadRequest {
  if (text.length > MAX_DOWNLOAD_BODY) return { ok: false };
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return { ok: false };
  }
  if (!isRecord(body) || !isFileIndex(body.fileIndex)) return { ok: false };
  const token = body.turnstileToken;
  if (token === undefined || token === null || token === "") return { ok: true, fileIndex: body.fileIndex };
  if (typeof token !== "string" || token.length > MAX_TURNSTILE_TOKEN) return { ok: false };
  return { ok: true, fileIndex: body.fileIndex, turnstileToken: token };
}

/** 举报请求体上限：detail ≤ 2000 + contact ≤ 200 + token ≤ 2048 加 JSON 转义余量。 */
export const MAX_REPORT_BODY = 16384;
export const MAX_REPORT_REASON = 64;
export const MAX_REPORT_DETAIL = 2000;
export const MAX_REPORT_CONTACT = 200;

/** 举报原因的页面键；发给 FluxCloud 的是 `REPORT_REASON_VALUE` 里的固定中文文本（后台审核员阅读）。 */
export const REPORT_REASON_KEYS = ["copyright", "porn", "illegal", "malware", "other"] as const;
export type ReportReasonKey = (typeof REPORT_REASON_KEYS)[number];
export const REPORT_REASON_VALUE: Record<ReportReasonKey, string> = {
  copyright: "侵权",
  porn: "色情低俗",
  illegal: "违法违规",
  malware: "恶意软件",
  other: "其他",
};

export type ReportRequest =
  | { ok: true; reason: string; detail?: string; contact?: string; turnstileToken?: string }
  | { ok: false };

/** 可选文本：缺省 / null / 空白 → 省略；非字符串或超长 → 非法（undefined 表示非法，null 表示省略）。 */
function optionalText(value: unknown, max: number): string | null | undefined {
  if (value === undefined || value === null) return null;
  if (typeof value !== "string") return undefined;
  const text = value.trim();
  if (text === "") return null;
  return text.length > max ? undefined : text;
}

/** 校验 `POST …/report` 请求体：reason 必填 ≤ 64，detail ≤ 2000，contact ≤ 200，turnstileToken ≤ 2048。 */
export function parseReportRequest(text: string): ReportRequest {
  if (text.length > MAX_REPORT_BODY) return { ok: false };
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return { ok: false };
  }
  if (!isRecord(body) || typeof body.reason !== "string") return { ok: false };
  const reason = body.reason.trim();
  if (reason === "" || reason.length > MAX_REPORT_REASON) return { ok: false };
  const detail = optionalText(body.detail, MAX_REPORT_DETAIL);
  const contact = optionalText(body.contact, MAX_REPORT_CONTACT);
  const token = optionalText(body.turnstileToken, MAX_TURNSTILE_TOKEN);
  if (detail === undefined || contact === undefined || token === undefined) return { ok: false };
  return {
    ok: true,
    reason,
    ...(detail !== null && { detail }),
    ...(contact !== null && { contact }),
    ...(token !== null && { turnstileToken: token }),
  };
}

function isSize(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function isInstant(value: unknown): value is string {
  return typeof value === "string" && !Number.isNaN(Date.parse(value));
}

/** 校验并裁剪上游清单；任何字段形状不符返回 null（上游异常，不渲染半截页面）。 */
export function parseManifest(value: unknown): ShareManifest | null {
  if (!isRecord(value)) return null;
  const { id, slug, title, files, totalBytes, expiresAt, cloudReady, pickupCode, turnstileSiteKey } = value;
  if (typeof id !== "string" || typeof slug !== "string" || typeof title !== "string") return null;
  if (!Array.isArray(files) || !isSize(totalBytes) || !isInstant(expiresAt)) return null;
  if (typeof cloudReady !== "boolean") return null;
  const parsed: ShareFile[] = [];
  for (const file of files) {
    if (!isRecord(file) || !isFileIndex(file.index) || typeof file.name !== "string" || !isSize(file.size)) {
      return null;
    }
    parsed.push({ index: file.index, name: file.name, size: file.size });
  }
  const manifest: ShareManifest = { id, slug, title, files: parsed, totalBytes, expiresAt, cloudReady };
  if (typeof pickupCode === "string" && PICKUP_CODE_RE.test(pickupCode)) {
    manifest.pickupCode = pickupCode;
  } else if (pickupCode !== undefined && pickupCode !== null) {
    return null;
  }
  if (typeof turnstileSiteKey === "string" && turnstileSiteKey !== "") {
    manifest.turnstileSiteKey = turnstileSiteKey;
  } else if (turnstileSiteKey !== undefined && turnstileSiteKey !== null && turnstileSiteKey !== "") {
    return null;
  }
  return manifest;
}

/** 校验签名下载链接：只接受 http(s) URL，浏览器会直接导航到它。 */
export function parseGrant(value: unknown): ShareGrant | null {
  if (!isRecord(value) || typeof value.url !== "string" || !isInstant(value.expiresAt)) return null;
  try {
    const url = new URL(value.url);
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  } catch {
    return null;
  }
  return { url: value.url, expiresAt: value.expiresAt };
}

/** 清单查询结果：成功带清单，失败带上游状态码与稳定错误码。 */
export type ShareLookup =
  | { ok: true; manifest: ShareManifest }
  | { ok: false; status: number; code: string; message?: string; retryAfter?: string };

export type LandingState = "ready" | "notReady" | "ended" | "rateLimited" | "unavailable";

/** 页面状态机：ready / notReady 是 200；ended 覆盖不存在、已结束与已过期（404/410）。 */
export function landingState(lookup: ShareLookup, now = Date.now()): LandingState {
  if (lookup.ok) {
    if (Date.parse(lookup.manifest.expiresAt) <= now) return "ended";
    return lookup.manifest.cloudReady ? "ready" : "notReady";
  }
  if (lookup.status === 404 || lookup.status === 410) return "ended";
  if (lookup.status === 429) return "rateLimited";
  return "unavailable";
}

/** 页面响应状态码：不存在 404、已结束 / 过期 / 下架 410，其余与上游语义对齐。 */
export function landingStatus(lookup: ShareLookup, now = Date.now()): number {
  const state = landingState(lookup, now);
  if (state === "ended") return !lookup.ok && lookup.status === 404 ? 404 : 410;
  if (state === "rateLimited") return 429;
  if (state === "unavailable") return 503;
  return 200;
}

export type DownloadErrorKey =
  | "ended"
  | "notReady"
  | "trafficExhausted"
  | "turnstileRequired"
  | "turnstileFailed"
  | "rateLimited"
  | "generic";

/** 下载请求失败 → 文案键：先看 FluxCloud 错误码，再看状态码。 */
export function downloadErrorKey(status: number, code: string | undefined): DownloadErrorKey {
  switch (code) {
    case "share_ended":
    case "share_takedown":
    case "share_not_found":
      return "ended";
    case "share_cloud_not_ready":
      return "notReady";
    case "traffic_exhausted":
      return "trafficExhausted";
    case "turnstile_required":
      return "turnstileRequired";
    case "turnstile_failed":
      return "turnstileFailed";
  }
  if (status === 404 || status === 410) return "ended";
  if (status === 409) return "notReady";
  if (status === 429) return "rateLimited";
  return "generic";
}

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

/** 人类可读大小（1024 进制，最多 1 位小数）：`1536 → "1.5 KB"`。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  let value = bytes;
  let unit = 0;
  while (unit < BYTE_UNITS.length - 1 && (value >= 1024 || Number(value.toFixed(1)) >= 1024)) {
    value /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? value : Number(value.toFixed(1))} ${BYTE_UNITS[unit]}`;
}

const LOCALE: Record<Lang, string> = { en: "en-US", zh: "zh-CN" };

/**
 * 过期时刻的服务端文本（固定 UTC，避免 SSR 与访客时区不一致）；页面脚本会在浏览器里
 * 把 `<time>` 的文本换成访客本地时间。无效时间返回 null。
 */
export function formatExpiry(expiresAt: string, lang: Lang): string | null {
  const at = new Date(expiresAt);
  if (Number.isNaN(at.getTime())) return null;
  const text = new Intl.DateTimeFormat(LOCALE[lang], {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    timeZone: "UTC",
  }).format(at);
  return `${text} UTC`;
}

/** 浏览器端：按访客本地时区格式化同一时刻。 */
export function formatExpiryLocal(expiresAt: string, lang: Lang): string | null {
  const at = new Date(expiresAt);
  if (Number.isNaN(at.getTime())) return null;
  return new Intl.DateTimeFormat(LOCALE[lang], { dateStyle: "medium", timeStyle: "short" }).format(at);
}

export interface RemainingParts {
  days: number;
  hours: number;
  minutes: number;
}

/** 距过期的剩余时间；已过期返回 null。 */
export function remainingParts(expiresAt: string, now = Date.now()): RemainingParts | null {
  const ms = Date.parse(expiresAt) - now;
  if (Number.isNaN(ms) || ms <= 0) return null;
  const minutesTotal = Math.floor(ms / 60_000);
  return {
    days: Math.floor(minutesTotal / 1440),
    hours: Math.floor((minutesTotal % 1440) / 60),
    minutes: minutesTotal % 60,
  };
}
