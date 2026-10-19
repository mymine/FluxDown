/**
 * tab 状态恢复的纯合并逻辑（零 browser 依赖，可在 bun 下测试）。
 *
 * SW 被消息唤醒时，`getPageConfig` / `mediaElementState` / `contextTarget` 等会先通过
 * `stateOf()` 建出一个空状态，早于 `storage.session.get` 返回；此时不能因为「已有状态」就
 * 整体跳过恢复，否则随后的防抖持久化会用空状态覆盖原快照。这里把快照合并进实时状态：
 * 实时数据优先，空缺部分用快照补上。
 */
import type { DashManifest } from "@/utils/dash-manifest";
import { normalizeHlsPlaylist, type HlsPlaylistEntry, type HlsPlaylistInfo } from "@/utils/hls-playlist";
import type { DashManifestEntry } from "@/utils/media-candidates";
import { pageUrlKey } from "@/utils/page-nav";
import type { DetectedResource } from "@/utils/resource-types";
import type { ElementState, HooksInfo } from "./protocol";

/** `storage.session` 中每个 tab 的快照。 */
export interface PersistedTab {
  pageUrl: string;
  resources: DetectedResource[];
  manifests: DashManifestEntry[];
  /** 旧版本快照没有此字段。 */
  hlsPlaylists?: HlsPlaylistEntry[];
  elements: ElementState[];
  /** 历史播放会话；旧版本快照没有此字段。 */
  retired?: ElementState[];
  /** 各帧的 sensor 会话；旧版本快照没有此字段（sensor 下次上报时按 resync 补全量）。 */
  sessions?: { frameId: number; sid: string; hooks: HooksInfo | null }[];
}

/** 恢复只关心的 tab 状态字段（TabState 的子集）。 */
export interface RestorableState {
  pageUrl: string;
  resources: Map<string, DetectedResource>;
  manifests: Map<string, DashManifest>;
  carriedManifests: Set<string>;
  hlsPlaylists: Map<string, HlsPlaylistInfo>;
  elements: Map<string, ElementState>;
  sessions: Map<number, { sid: string; hooks: HooksInfo | null }>;
  retired: ElementState[];
}

/**
 * 把快照合并进实时状态；返回是否采用了快照（false = 快照属于另一个页面，已丢弃）。
 * - 实时页面 URL（`state.pageUrl`，其次 tab 当前 URL `currentUrl`）与快照换了页：丢弃快照；
 * - 同一页面：快照里的资源 / 清单 / 元素只补实时状态里没有的项。
 */
export function mergeRestoredTab(
  state: RestorableState,
  saved: PersistedTab,
  currentUrl: string | undefined,
  normalizeManifest: (raw: unknown) => DashManifest | null,
): boolean {
  const savedUrl = saved.pageUrl ?? "";
  const liveUrl = state.pageUrl || currentUrl || "";
  if (savedUrl && liveUrl && pageUrlKey(savedUrl) !== pageUrlKey(liveUrl)) return false;
  if (!state.pageUrl) state.pageUrl = savedUrl;
  for (const resource of saved.resources ?? []) {
    if (!state.resources.has(resource.id)) state.resources.set(resource.id, resource);
  }
  for (const entry of saved.manifests ?? []) {
    if (state.manifests.has(entry.url)) continue;
    const manifest = normalizeManifest(entry.manifest);
    if (!manifest) continue;
    state.manifests.set(entry.url, manifest);
    if (entry.carried) state.carriedManifests.add(entry.url);
  }
  for (const entry of saved.hlsPlaylists ?? []) {
    if (state.hlsPlaylists.has(entry.url)) continue;
    const playlist = normalizeHlsPlaylist(entry.playlist);
    if (playlist) state.hlsPlaylists.set(entry.url, playlist);
  }
  for (const entry of saved.sessions ?? []) {
    if (!state.sessions.has(entry.frameId)) state.sessions.set(entry.frameId, { sid: entry.sid, hooks: entry.hooks ?? null });
  }
  for (const element of saved.elements ?? []) {
    // 帧里已有别的 sensor 实例（快照之后被重注入 / 换了文档）：旧实例的元素已失效
    const live = state.sessions.get(element.frameId);
    if (live && !element.key.startsWith(`${live.sid}:`)) continue;
    if (!state.elements.has(element.key)) state.elements.set(element.key, element);
  }
  if (state.retired.length === 0 && saved.retired) state.retired = saved.retired;
  return true;
}
