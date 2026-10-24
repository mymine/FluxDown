/** `fluxdown.addMedia` 条目 → 现有嗅探列表入库格式（纯函数）。 */
import type { ResourceMessagePayload, ResourceType } from "@/utils/resource-types";
import type { BridgeMediaItem } from "./wire";

/** 来源标注：入库资源的 `ruleIds` 里放 `page-script:<identity>`（诊断面板与「规则声明」判定沿用现有路径）。 */
export const PAGE_SCRIPT_SOURCE_PREFIX = "page-script:";

/** 用 mime 推断资源类型；无法判断返回 "other"，由规则引擎按扩展名继续分类。 */
export function mediaTypeFromMime(mime: string | undefined): ResourceType {
  const base = mime?.split(";")[0].trim().toLowerCase() ?? "";
  if (base.startsWith("video/")) return "video";
  if (base.startsWith("audio/")) return "audio";
  if (base.startsWith("image/")) return "image";
  if (base === "application/vnd.apple.mpegurl" || base === "application/x-mpegurl" || base === "application/dash+xml") {
    return "stream";
  }
  return "other";
}

export function mediaPayloads(items: readonly BridgeMediaItem[], pageUrl: string): ResourceMessagePayload[] {
  return items.map((item) => ({
    url: item.url,
    type: mediaTypeFromMime(item.mime),
    ...(item.title ? { filename: item.title } : {}),
    ...(item.mime ? { mimeType: item.mime.split(";")[0].trim() } : {}),
    detectedBy: "dom-scan",
    pageUrl,
  }));
}
