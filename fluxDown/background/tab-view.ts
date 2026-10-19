/** 从 tab-state / settings / host 取数据，交给纯函数 composeTabView 生成 TabView。 */
import { browser } from "wxt/browser";
import { effectiveRemoteMode } from "@/utils/download-dispatch";
import { profileFor } from "@/utils/site-profiles";
import { getCapabilities } from "./host";
import type { TabView } from "./protocol";
import { sawNetworkRequest } from "./request-cache";
import { getEngine } from "./rules-store";
import { getCachedProfiles, getCachedSettings } from "./settings-cache";
import { loadServiceWorkerScopes } from "./sw-scopes";
import {
  getElements,
  getRetiredElements,
  lateHookFrames,
  getHlsPlaylists,
  getManifests,
  getResources,
  getTabPageUrl,
  getTabVersion,
  whenRestored,
} from "./tab-state";
import { probeStateOf } from "./probe";
import { composeTabView, handoffFor } from "./view-model";

function hostOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

async function tabTitle(tabId: number): Promise<{ title: string; url: string }> {
  try {
    const tab = await browser.tabs.get(tabId);
    return { title: tab.title ?? "", url: tab.url ?? "" };
  } catch {
    return { title: "", url: "" };
  }
}

/** `withHandoff: false`：不探测桌面端能力（徽标刷新、下载取候选等不需要 handoff 的路径）；handoff 填占位值。 */
export async function buildTabView(tabId: number, opts: { withHandoff?: boolean } = {}): Promise<TabView> {
  const withHandoff = opts.withHandoff ?? true;
  const [settings, profiles, caps, remoteMode, tab, , serviceWorkerScopes] = await Promise.all([
    getCachedSettings(),
    getCachedProfiles(),
    withHandoff ? getCapabilities() : Promise.resolve(null),
    withHandoff ? effectiveRemoteMode() : Promise.resolve("off" as const),
    tabTitle(tabId),
    // SW 被本次请求唤醒时，会话快照恢复尚未完成；先等它，否则返回空视图（浮钮随之消失）。
    whenRestored(),
    loadServiceWorkerScopes(),
  ]);
  const engine = getEngine();
  const resources = getResources(tabId);
  const pageUrl = getTabPageUrl(tabId) || resources.find((r) => r.pageUrl)?.pageUrl || tab.url;
  const host = hostOf(pageUrl);
  const { key: profileKey, profile } = profileFor(profiles, host);
  const site: TabView["site"] = {
    host,
    profileKey,
    profile,
  };
  return composeTabView({
    tabId,
    pageUrl,
    pageTitle: tab.title,
    version: getTabVersion(tabId),
    resources,
    manifests: getManifests(tabId),
    hlsPlaylists: getHlsPlaylists(tabId),
    mirrorGroups: engine.mirrorGroups(),
    elements: getElements(tabId),
    retiredElements: getRetiredElements(tabId),
    engine,
    showLowConfidence: settings.showLowConfidence,
    handoff: caps ? handoffFor({ remoteMode, caps, pageUrl }) : { kind: "unavailable", reason: "disconnected" },
    site,
    probe: probeStateOf(tabId),
    lateFrames: lateHookFrames(tabId),
    serviceWorkerScopes,
    sawNetworkRequest,
  });
}
