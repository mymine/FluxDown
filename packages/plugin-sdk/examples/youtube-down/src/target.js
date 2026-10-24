// 链接归一：把各种 YouTube 链接归成视频 / 播放列表（QuickJS 没有 URL 全局，用正则）。
import { parseQuery } from "./query.js";

export const VIDEO_ID_RE = /^[A-Za-z0-9_-]{11}$/;
const LIST_ID_RE = /^[A-Za-z0-9_-]{2,64}$/;
export const PRESETS = ["best", "2160", "1440", "1080", "720", "480", "360", "audio"];

/**
 * 解析 YouTube 链接：返回 `{ videoId }`、`{ playlistId }` 或 null。
 * `watch?v=…&list=…` 视为单个视频（用户正在看的那一个）；只有 `/playlist?list=…` 才是整个列表。
 */
export function parseTarget(url) {
  const m = /^https?:\/\/([^/?#]+)([^?#]*)(?:\?([^#]*))?/i.exec(url || "");
  if (!m) return null;
  const host = m[1].toLowerCase().replace(/:\d+$/, "");
  const path = m[2] || "/";
  const query = parseQuery(m[3] || "");
  const segments = path.split("/").filter(Boolean);
  const video = (id) => (VIDEO_ID_RE.test(id || "") ? { videoId: id } : null);

  if (host === "youtu.be") return video(segments[0]);
  if (!/(^|\.)youtube(-nocookie)?\.com$/.test(host)) return null;
  const head = (segments[0] || "").toLowerCase();
  if (head === "watch") return video(query.v);
  if (head === "shorts" || head === "live" || head === "embed") return video(segments[1]);
  if (head === "playlist") return LIST_ID_RE.test(query.list || "") ? { playlistId: query.list } : null;
  return null;
}

/** 二段解析的条目标识 `<videoId>@<preset>`（清单条目 id + 规格 id）。 */
export function parseItem(resolverItem) {
  if (!resolverItem) return null;
  const at = resolverItem.indexOf("@");
  const videoId = at >= 0 ? resolverItem.slice(0, at) : resolverItem;
  const preset = at >= 0 ? resolverItem.slice(at + 1) : "best";
  if (!VIDEO_ID_RE.test(videoId)) throw new Error(`无效的播放列表条目: ${resolverItem}`);
  return { videoId, preset: PRESETS.includes(preset) ? preset : "best" };
}
