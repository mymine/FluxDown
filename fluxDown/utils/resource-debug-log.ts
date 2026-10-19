/**
 * Resource sniffer diagnostics export.
 *
 * The export keeps URL paths and parsed media metadata, because those are the
 * facts needed to diagnose aggregation. Credentials in query strings and in
 * path-embedded signed tokens (Akamai-style `hdnts=exp=...~hmac=...` segments,
 * `/token/<jwt>/...` segments) are redacted, and cookies/header values are
 * never serialized.
 */

import type { DashManifest } from "./dash-manifest";
import type {
  DashManifestEntry,
  MediaCandidate,
  MediaCandidateVariant,
} from "./media-candidates";
import { countMediaCandidateRows } from "./media-candidates";
import type { HlsPlaylistEntry } from "./hls-playlist";
import type { DetectedResource } from "./resource-types";
import type { ElementState } from "@/background/protocol";
export type DebugUrlKey = (url: string) => string;

/** 候选与播放元素的关联（来自视图模型）。 */
export interface DebugCandidateBinding {
  elementKey: string | null;
  binding: "exact" | "temporal" | "declared" | "inferred" | "none";
  playingVariantId: string | null;
}

export interface ResourceDebugLogOptions {
  resources: DetectedResource[];
  manifests?: DashManifestEntry[];
  hlsPlaylists?: HlsPlaylistEntry[];
  candidates: MediaCandidate[];
  /** 页面媒体元素状态（含 MSE 归因证据）；浮钮与「当前播放」分组都依赖它。 */
  elements?: ElementState[];
  /** 候选 id → 关联到的元素；缺省视为未关联。 */
  bindings?: ReadonlyMap<string, DebugCandidateBinding>;
  tabId?: number;
  pageUrl?: string;
  pageTitle?: string;
  source: "popup" | "content" | "sidepanel";
  /** URL 去重键（`engine.dedupeKey` 的闭包），由调用方传入。 */
  key: DebugUrlKey;
}

const SENSITIVE_QUERY_KEYS = new Set([
  "token",
  "access_token",
  "auth",
  "authorization",
  "sign",
  "signature",
  "sig",
  "policy",
  "expires",
  "expires_at",
  "expire",
  "key-pair-id",
  "key_pair_id",
  "x-amz-signature",
  "x-amz-credential",
  "x-amz-security-token",
  "hdnts",
]);

function isSensitiveQueryKey(key: string): boolean {
  // hmac/sig：Akamai `hdnts=exp=…~acl=/*~hmac=…` 的 acl 若含 `/`，URL 解析会把
  // `hmac=…` 拆成独立路径段，此时子键本身必须能被识别，否则签名原样落盘。
  return SENSITIVE_QUERY_KEYS.has(key.toLowerCase()) ||
    /(^|[-_])(token|sign|sig|signature|hmac|credential|authorization|auth|key.?pair.?id|policy|expires?|access.?token)([-_]|$)/i.test(key);
}

/**
 * A base64/hex-style opaque path segment (bearer token, signed cookie, JWT
 * part) rather than a human filename: long, no filename-style separators
 * (space/underscore), and either pure hex or mixes case + digits the way
 * encoded binary does — natural titles this long are near-universally
 * lowercase-with-separators or carry a recognizable extension.
 */
function looksLikeOpaqueToken(segment: string): boolean {
  if (segment.length < 32 || /[\s_]/.test(segment)) return false;
  if (!/^[A-Za-z0-9+/-]+$/.test(segment)) return false;
  if (/^[0-9a-fA-F]+$/.test(segment)) return true;
  return /[0-9]/.test(segment) && /[a-z]/.test(segment) && /[A-Z]/.test(segment);
}

/**
 * Redact a signed-token path segment. Two shapes seen in the wild:
 * - Akamai-style `key=value[~key=value...]` blob (`hdnts=exp=...~hmac=...`).
 *   Any sensitive sub-key (leading `hdnts=` or inner `hmac=`) redacts the whole
 *   blob: the acl sub-value may contain `/`, in which case URL parsing splits
 *   the blob across several path segments and the `hmac=...` tail lands in a
 *   segment of its own, so the leading key alone cannot be relied upon.
 * - A bare JWT (three dot-joined base64url parts) or a long base64/hex run
 *   embedded directly in the path, e.g. `/token/<jwt>/seg.m4s`.
 */
function redactPathSegment(segment: string): string {
  if (!segment) return segment;
  if (segment.includes("=")) {
    const sensitive = segment.split("~").some((part) => {
      const eq = part.indexOf("=");
      return eq > 0 && isSensitiveQueryKey(part.slice(0, eq).replace(/^[^A-Za-z0-9]+/, ""));
    });
    return sensitive ? "[REDACTED]" : segment;
  }
  if (/^[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}$/.test(segment)) {
    return "[REDACTED]";
  }
  return looksLikeOpaqueToken(segment) ? "[REDACTED]" : segment;
}

export function redactUrl(url: string | undefined): string {
  if (!url) return "";
  try {
    const parsed = new URL(url);
    for (const key of [...parsed.searchParams.keys()]) {
      if (isSensitiveQueryKey(key)) parsed.searchParams.set(key, "[REDACTED]");
    }
    parsed.pathname = parsed.pathname.split("/").map(redactPathSegment).join("/");
    return parsed.toString();
  } catch {
    return url;
  }
}

function canonicalOf(key: DebugUrlKey, url: string | undefined): string {
  return url ? key(redactUrl(url)) : "";
}

function timestampIso(value: number): string | undefined {
  if (!Number.isFinite(value) || value <= 0) return undefined;
  try {
    return new Date(value).toISOString();
  } catch {
    return undefined;
  }
}

function debugTrack(track: DashManifest["video"][number], key: DebugUrlKey) {
  return {
    id: track.id,
    url: redactUrl(track.url),
    canonicalUrl: canonicalOf(key, track.url),
    mimeType: track.mimeType,
    codecs: track.codecs,
    bandwidth: track.bandwidth,
    width: track.width,
    height: track.height,
    downloadable: track.downloadable !== false,
  };
}

function debugVariant(variant: MediaCandidateVariant, key: DebugUrlKey) {
  return {
    id: variant.id,
    label: variant.label,
    videoUrl: redactUrl(variant.videoUrl),
    canonicalVideoUrl: canonicalOf(key, variant.videoUrl),
    audioUrl: redactUrl(variant.audioUrl),
    canonicalAudioUrl: canonicalOf(key, variant.audioUrl),
    mimeType: variant.mimeType,
    bandwidth: variant.bandwidth,
    codec: variant.codec,
    fileSize: variant.fileSize,
    resourceId: variant.resourceId,
    alternateUrls: (variant.alternateUrls ?? []).map(redactUrl),
  };
}

type DebugTrack = ReturnType<typeof debugTrack>;
type DebugVariant = ReturnType<typeof debugVariant>;

/** 资源调试日志快照（可 JSON 序列化；URL 已脱敏）。 */
export interface ResourceDebugLog {
  format: "fluxdown-resource-debug";
  schemaVersion: 1;
  exportedAt: string;
  source: ResourceDebugLogOptions["source"];
  context: { tabId: number | undefined; pageUrl: string; pageTitle: string };
  summary: {
    rawResourceCount: number;
    rawMediaResourceCount: number;
    manifestCount: number;
    candidateCount: number;
    downloadableCandidateCount: number;
    representedRawResourceCount: number;
    displayedRowCount: number;
  };
  resources: Array<{
    id: string;
    url: string;
    canonicalUrl: string;
    ruleIds: string[] | undefined;
    finalUrl: string;
    canonicalFinalUrl: string;
    filename: string;
    type: DetectedResource["type"];
    size: number;
    mimeType: string | undefined;
    quality: string | undefined;
    detectedBy: DetectedResource["detectedBy"];
    detectedAt: number;
    detectedAtIso: string | undefined;
    tabId: number;
    pageUrl: string;
    confidence: DetectedResource["confidence"];
    isAttachment: boolean | undefined;
    auth: { hasCookies: boolean; headerNames: string[] };
  }>;
  manifests: Array<{ url: string; canonicalUrl: string; carried: boolean; video: DebugTrack[]; audio: DebugTrack[] }>;
  hlsPlaylists: Array<{ url: string; children: string[]; segmentDirs: string[] }>;
  elements: Array<{
    key: string;
    frameId: number;
    tag: ElementState["tag"];
    playing: boolean;
    visibleRatio: number;
    rect: ElementState["rect"];
    videoHeight: number;
    currentSrc: string;
    sourceBuffers: Array<{
      kind: string;
      mime: string;
      exact: string[];
      temporal: Array<{ url: string; count: number }>;
    }>;
  }>;
  candidates: Array<{
    id: string;
    title: string;
    type: MediaCandidate["type"];
    source: MediaCandidate["source"];
    pageUrl: string;
    downloadable: boolean;
    fragmentCount: number;
    rawResourceIds: string[];
    binding: DebugCandidateBinding;
    variants: DebugVariant[];
  }>;
}

/** Build a versioned, JSON-serializable snapshot of the current sniffer state. */
export function buildResourceDebugLog(options: ResourceDebugLogOptions): ResourceDebugLog {
  const manifests = options.manifests || [];
  const representedIds = new Set(
    options.candidates.flatMap((candidate) => candidate.rawResourceIds),
  );
  const rawNonMediaCount = options.resources.filter(
    (resource) =>
      resource.type !== "video" &&
      resource.type !== "stream" &&
      !representedIds.has(resource.id),
  ).length;
  const mediaResourceCount = options.resources.filter(
    (resource) =>
      resource.type === "video" ||
      resource.type === "audio" ||
      resource.type === "stream",
  ).length;

  return {
    format: "fluxdown-resource-debug",
    schemaVersion: 1,
    exportedAt: new Date().toISOString(),
    source: options.source,
    context: {
      tabId: options.tabId,
      pageUrl: redactUrl(options.pageUrl),
      pageTitle: options.pageTitle || "",
    },
    summary: {
      rawResourceCount: options.resources.length,
      rawMediaResourceCount: mediaResourceCount,
      manifestCount: manifests.length,
      candidateCount: options.candidates.length,
      downloadableCandidateCount: options.candidates.filter(
        (candidate) => candidate.downloadable,
      ).length,
      representedRawResourceCount: representedIds.size,
      displayedRowCount: countMediaCandidateRows(options.candidates) + rawNonMediaCount,
    },
    resources: options.resources.map((resource) => ({
      id: resource.id,
      url: redactUrl(resource.url),
      canonicalUrl: canonicalOf(options.key, resource.url),
      ruleIds: resource.ruleIds,
      finalUrl: redactUrl(resource.finalUrl),
      canonicalFinalUrl: canonicalOf(options.key, resource.finalUrl),
      filename: resource.filename,
      type: resource.type,
      size: resource.size,
      mimeType: resource.mimeType,
      quality: resource.quality,
      detectedBy: resource.detectedBy,
      detectedAt: resource.detectedAt,
      detectedAtIso: timestampIso(resource.detectedAt),
      tabId: resource.tabId,
      pageUrl: redactUrl(resource.pageUrl),
      confidence: resource.confidence,
      isAttachment: resource.isAttachment,
      auth: {
        hasCookies: Boolean(resource.cookies),
        headerNames: Object.keys(resource.headers || {}).sort(),
      },
    })),
    manifests: manifests.map((entry) => ({
      url: redactUrl(entry.url),
      canonicalUrl: canonicalOf(options.key, entry.url),
      carried: entry.carried === true,
      video: entry.manifest.video.map((track) => debugTrack(track, options.key)),
      audio: entry.manifest.audio.map((track) => debugTrack(track, options.key)),
    })),
    hlsPlaylists: (options.hlsPlaylists || []).map((entry) => ({
      url: redactUrl(entry.url),
      children: entry.playlist.children.map(redactUrl),
      segmentDirs: entry.playlist.segmentDirs.map(redactUrl),
    })),
    elements: (options.elements || []).map((element) => ({
      key: element.key,
      frameId: element.frameId,
      tag: element.tag,
      playing: element.playing,
      visibleRatio: element.visibleRatio,
      rect: element.rect,
      videoHeight: element.videoHeight,
      currentSrc: redactUrl(element.currentSrc),
      sourceBuffers: element.bindings.map((sb) => ({
        kind: sb.kind,
        mime: sb.mime,
        exact: sb.exact.map(redactUrl),
        temporal: sb.temporal.map((t) => ({ url: redactUrl(t.url), count: t.count })),
      })),
    })),
    candidates: options.candidates.map((candidate) => ({
      id: candidate.id,
      title: candidate.title,
      type: candidate.type,
      source: candidate.source,
      pageUrl: redactUrl(candidate.pageUrl),
      downloadable: candidate.downloadable,
      fragmentCount: candidate.fragmentCount,
      rawResourceIds: candidate.rawResourceIds,
      binding: options.bindings?.get(candidate.id) ?? { elementKey: null, binding: "none", playingVariantId: null },
      variants: candidate.variants.map((variant) => debugVariant(variant, options.key)),
    })),
  };
}

export function stringifyResourceDebugLog(log: ResourceDebugLog): string {
  return `${JSON.stringify(log, null, 2)}\n`;
}
