/** 候选 / 资源在 UI 中的展示文案辅助。 */
import type { MediaCandidateVariant } from "@/utils/media-candidates";
import { qualityFrameRateLabel, qualityResolutionLabel } from "@/utils/media-candidates";
import type { DetectedResource } from "@/utils/resource-types";
import type { t as TFn } from "@/utils/i18n";

/** 清晰度文案：auto / original 走 i18n，"<n>p" 规范为 "1080P"，其余（码率标识等）原样展示。 */
export function variantLabel(t: typeof TFn, variant: MediaCandidateVariant): string {
  if (variant.label === "auto") return t("panel.autoQuality");
  if (variant.label === "original") return t("panel.originalQuality");
  const resolution = qualityResolutionLabel(variant.label);
  if (!resolution) return variant.label || t("panel.qualityUnknown");
  const fps = qualityFrameRateLabel(variant.frameRate);
  return fps ? `${resolution} ${fps}` : resolution;
}

/** 资源展示名：filename → URL 末段 → URL。 */
export function resourceDisplayName(r: Pick<DetectedResource, "filename" | "url">, fallback: string): string {
  const filename = r.filename?.trim();
  if (filename) return filename;
  try {
    const last = new URL(r.url).pathname.split("/").filter(Boolean).pop();
    if (last) return decodeURIComponent(last);
  } catch {
    // 相对 / 畸形 URL：退回整串
  }
  return r.url || fallback;
}

export function hostOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}
