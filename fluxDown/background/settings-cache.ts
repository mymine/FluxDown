/**
 * background 的 settings / 站点配置内存缓存。
 *
 * 避免每次拦截都 await storage（热路径去异步化）：缓存永不主动过期，完全依赖
 * `storage.onChanged` 失效，这样同步路径（防止下载 UI 闪现的关键）覆盖率接近
 * 100%，仅在 Service Worker 冷启动的第一个下载时缓存为 null。
 *
 * 本模块只依赖 utils，不导入其它 background 模块（download-dispatch 依赖它，
 * 反向依赖会形成环）。
 */
import { browser } from "wxt/browser";
import { REMOTE_SETTINGS_KEY } from "@/utils/remote-settings";
import { loadSettings } from "@/utils/settings";
import type { FluxDownSettings } from "@/utils/settings";
import { SITE_PROFILES_KEY, loadSiteProfiles } from "@/utils/site-profiles";
import type { SiteProfile } from "@/utils/site-profiles";

let settingsCache: FluxDownSettings | null = null;
// 用 inflight Promise 防止并发调用时发起多次 loadSettings。
let settingsInflight: Promise<FluxDownSettings> | null = null;
// 版本序号防止 inflight 竞态：storage.onChanged 在 inflight 期间触发时，
// 旧值不得写回覆盖新设置。
let settingsVersion = 0;

let profilesCache: Record<string, SiteProfile> | null = null;
let profilesInflight: Promise<Record<string, SiteProfile>> | null = null;
let profilesVersion = 0;

const settingsListeners = new Set<() => void>();

export function peekCachedSettings(): FluxDownSettings | null {
  return settingsCache;
}

export function peekCachedProfiles(): Record<string, SiteProfile> | null {
  return profilesCache;
}

export function getCachedSettings(): Promise<FluxDownSettings> {
  if (settingsCache) return Promise.resolve(settingsCache);
  if (settingsInflight) return settingsInflight;
  const versionAtStart = settingsVersion;
  settingsInflight = loadSettings()
    .then((s) => {
      if (settingsVersion === versionAtStart) settingsCache = s;
      settingsInflight = null;
      return s;
    })
    .catch((e) => {
      settingsInflight = null;
      throw e;
    });
  return settingsInflight;
}

export function getCachedProfiles(): Promise<Record<string, SiteProfile>> {
  if (profilesCache) return Promise.resolve(profilesCache);
  if (profilesInflight) return profilesInflight;
  const versionAtStart = profilesVersion;
  profilesInflight = loadSiteProfiles()
    .then((p) => {
      if (profilesVersion === versionAtStart) profilesCache = p;
      profilesInflight = null;
      return p;
    })
    .catch((e) => {
      profilesInflight = null;
      throw e;
    });
  return profilesInflight;
}

/** settings 或站点配置变化后（缓存已失效）通知：图标、下载栏、PageConfig 等据此刷新。 */
export function onSettingsChanged(listener: () => void): void {
  settingsListeners.add(listener);
}

/** 同步注册 storage 监听；由 startBackground 调用一次。 */
export function installSettingsCacheListener(): void {
  browser.storage.onChanged.addListener((changes, area) => {
    let changed = false;
    if ((area === "sync" && changes.settings) || (area === "local" && changes[REMOTE_SETTINGS_KEY])) {
      settingsCache = null;
      settingsInflight = null;
      settingsVersion++;
      changed = true;
    }
    if (area === "local" && changes[SITE_PROFILES_KEY]) {
      profilesCache = null;
      profilesInflight = null;
      profilesVersion++;
      changed = true;
    }
    if (!changed) return;
    for (const listener of settingsListeners) {
      try {
        listener();
      } catch (e) {
        console.warn("[FluxDown] settings listener failed:", e);
      }
    }
  });
}
