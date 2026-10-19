/**
 * 资源数据模型与少量 URL 工具。
 *
 * 分类、噪音过滤、去重键、扩展名推断全部由声明式规则引擎（`rules/`）承担；
 * 大小阈值与可信度在 `background/resource-policy.ts`。本文件只保留跨模块共享的类型。
 */

// ===== 数据模型 =====

/** 资源分类 */
export type ResourceType =
  | "video"
  | "audio"
  | "document"
  | "archive"
  | "image"
  | "executable"
  | "torrent"
  | "stream"
  | "subtitle"
  | "magnet"
  | "other";

/** 资源检测来源 */
export type DetectionMethod =
  | "webRequest"
  | "dom-scan"
  | "mutation-observer"
  | "fetch-intercept"
  | "xhr-intercept"
  /** URL 只出现在页面 API 的 JSON 响应里，页面未必真的请求过（列表页 / 推荐流常见）。 */
  | "api-scan";

/**
 * 资源可信度等级
 *
 *  high   — 用户明确想要的（attachment、<a download>、大文件视频/音频）
 *  medium — 大概率有价值（HTTP 媒体 >阈值、文档、压缩包）
 *  low    — 可能有价值但噪音风险高（小 XHR、未知类型）
 */
export type ConfidenceLevel = "high" | "medium" | "low";

/** 检测到的可下载资源 */
export interface DetectedResource {
  id: string;
  url: string;
  finalUrl?: string;
  filename: string;
  type: ResourceType;
  size: number; // bytes, -1 = 未知
  mimeType?: string;
  quality?: string;
  detectedBy: DetectionMethod;
  detectedAt: number;
  tabId: number;
  pageUrl: string;
  /** 可信度等级 */
  confidence: ConfidenceLevel;
  /** 是否有 Content-Disposition: attachment */
  isAttachment?: boolean;
  /** 资源请求的 Cookie（webRequest 嗅探时捕获，下载时传递给 FluxDown） */
  cookies?: string;
  /** 资源请求的自定义头（Authorization 等，webRequest 嗅探时捕获） */
  headers?: Record<string, string>;
  /** 命中的嗅探规则 id（入库时由规则引擎记录，诊断面板展示）。 */
  ruleIds?: string[];
  /** 来源帧（sensor 上报时为 sender.frameId；webRequest 为请求的 frameId）。 */
  frameId?: number;
  /** 产出该资源的媒体元素 key（<track> / currentSrc 等）。 */
  elementKey?: string;
  /** 字幕语言（<track srclang>）。 */
  lang?: string;
  /** 字幕标签（<track label>）。 */
  label?: string;
  /** 换页前请求的播放清单（自动连播预取的下一个视频）：只在关联到播放元素后展示。 */
  carried?: boolean;
}

/** sensor → background（sensorReport）的资源上报格式 */
export interface ResourceMessagePayload {
  url: string;
  type: ResourceType;
  filename?: string;
  size?: number;
  mimeType?: string;
  quality?: string;
  detectedBy: DetectionMethod;
  pageUrl?: string;
  isAttachment?: boolean;
  /** 字幕语言（<track srclang>，#731）。 */
  lang?: string;
  /** 字幕标签（<track label>）。 */
  label?: string;
  elementKey?: string;
}

// ===== 工具函数 =====

/** URL 末段（百分号解码）。末段没有扩展名时原样返回，扩展名由调用方用 engine.fileExtension 补。 */
export function extractFilenameFromUrl(url: string): string {
  try {
    const pathname = new URL(url).pathname;
    const lastSegment = pathname.split("/").pop() || "";
    return decodeURIComponent(lastSegment);
  } catch {
    return "";
  }
}

/** URL 是否像流媒体清单（.m3u8 / .mpd / manifest / playlist）。 */
export function isStreamingUrl(url: string): boolean {
  const lower = url.toLowerCase();
  return (
    lower.includes(".m3u8") ||
    lower.includes(".mpd") ||
    lower.includes("/manifest") ||
    lower.includes("/playlist")
  );
}
