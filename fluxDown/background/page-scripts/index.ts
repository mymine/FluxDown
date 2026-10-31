/**
 * 页面脚本管理器的接线：把浏览器 API、桌面端请求、存储与现有下载 / 嗅探入口接到纯逻辑模块
 * （sync.ts / bridge.ts / store.ts）。
 *
 * `installPageScripts()` 必须在后台脚本顶层同步调用：事件监听器要在 SW / event page 首次执行时注册。
 * Firefox 未授予 `userScripts` 权限时 `runtime.onUserScriptMessage` 不存在，授权（`permissions.onAdded`）后再注册。
 */
import { browser } from "wxt/browser";
import { hostOf } from "@/rules/url";
import { effectiveRemoteMode } from "@/utils/download-dispatch";
import { profileFor } from "@/utils/site-profiles";
import { fetchPageScripts, fetchPageScriptSource, getCapabilities, invokePlugin, onCapabilities } from "../host";
import type { HostCapabilities } from "../protocol";
import { describeSendError, sendToFluxDown } from "../sender";
import { getCachedProfiles, getCachedSettings } from "../settings-cache";
import { getTabPageUrl, ingestResources } from "../tab-state";
import { browserAvailabilityEnv, probeAvailability } from "./availability";
import { createBridge } from "./bridge";
import { sha256Hex } from "./hash";
import { mediaPayloads, PAGE_SCRIPT_SOURCE_PREFIX } from "./media";
import type { AddMediaCall, BridgeHandler, DownloadCall } from "./bridge";
import type { PageScriptSourceResult, PageScriptsActionResult, PageScriptsStatus } from "./model";
import { createPageScriptStore } from "./store";
import type { PageScriptStore } from "./store";
import { buildStatus } from "./status";
import { createPageScriptSync } from "./sync";
import type { PageScriptSync, SyncCaps, SyncTrigger, UserScriptsApi } from "./sync";
import { logWarn } from "@/utils/error-log";

const SYNC_ALARM = "fluxdown-page-scripts-sync";
const SYNC_PERIOD_MIN = 10;
/** 状态请求在缓存新于该时长时不重新同步（`refresh: "stale"`）。 */
const STALE_AFTER_MS = 60_000;

interface Services {
  store: PageScriptStore;
  sync: PageScriptSync;
  bridge: BridgeHandler;
}

let services: Services | null = null;
let bridgeListening = false;

function toSyncCaps(caps: HostCapabilities): SyncCaps {
  return {
    protocol: caps.protocol,
    connected: caps.connected,
    actions: caps.actions,
    pageScriptsSequence: caps.pageScriptsSequence,
    pageScriptsEpoch: caps.pageScriptsEpoch,
  };
}

const browserScripts: UserScriptsApi = {
  async getIds() {
    return (await browser.userScripts.getScripts()).map((script) => script.id);
  },
  register: (scripts) => browser.userScripts.register(scripts),
  update: (scripts) => browser.userScripts.update(scripts),
  unregister: (ids) => browser.userScripts.unregister({ ids }),
  configureWorld: (properties) => browser.userScripts.configureWorld(properties),
  resetWorld: (worldId) => browser.userScripts.resetWorldConfiguration(worldId),
};

// ===== 桥能力 =====

async function download(call: DownloadCall): Promise<{ ok: boolean; message?: string }> {
  const outcome = await sendToFluxDown({
    url: call.url,
    filename: call.filename,
    referrer: call.referrer,
    // 页面脚本提供的头只在没有浏览器真实请求头缓存时使用（白名单已在 bridge 校验）。
    storedHeaders: Object.keys(call.headers).length > 0 ? call.headers : undefined,
    fileSize: 0,
    pageHost: hostOf(call.referrer),
  });
  return outcome.success ? { ok: true } : { ok: false, message: describeSendError(outcome.message) };
}

async function addMedia(call: AddMediaCall): Promise<number> {
  const topUrl = getTabPageUrl(call.tabId) || call.tabUrl || call.pageUrl;
  // 与 sensor 上报同口径：全局嗅探开关与站点配置 sniff 关闭时不入库。
  const [settings, profiles] = await Promise.all([getCachedSettings(), getCachedProfiles()]);
  if (!settings.resourceSniffing || !profileFor(profiles, hostOf(topUrl)).profile.sniff) return 0;
  return ingestResources(call.tabId, topUrl, mediaPayloads(call.items, topUrl), {
    frameId: call.frameId,
    ruleIds: [`${PAGE_SCRIPT_SOURCE_PREFIX}${call.identity}`],
  });
}

// ===== 安装 =====

function createServices(): Services {
  const store = createPageScriptStore(browser.storage.local);
  const sync = createPageScriptSync({
    store,
    availability: () => probeAvailability(browserAvailabilityEnv),
    capabilities: async (force) => toSyncCaps(await getCapabilities(force)),
    remoteMode: async () => (await effectiveRemoteMode()) === "always",
    fetchIndex: fetchPageScripts,
    fetchSource: fetchPageScriptSource,
    sha256: sha256Hex,
    scripts: browserScripts,
    now: () => Date.now(),
  });
  const bridge = createBridge({
    identityForToken: (token) => store.identityForToken(token),
    now: () => Date.now(),
    extensionId: browser.runtime.id,
    download,
    addMedia,
    invoke: (identity, name, args, pageUrl) => invokePlugin(identity, name, args, pageUrl),
    storage: {
      get: (identity, key) => store.storageGet(identity, key),
      set: (identity, key, value) => store.storageSet(identity, key, value),
      remove: (identity, key) => store.storageRemove(identity, key),
    },
  });
  return { store, sync, bridge };
}

function reconcileInBackground(trigger: SyncTrigger, caps?: SyncCaps): void {
  if (!services) return;
  services.sync.reconcile(trigger, caps).catch((e) => logWarn(`[FluxDown] page scripts sync (${trigger}) failed:`, e));
}

/** 注册 `runtime.onUserScriptMessage`（事件不存在时跳过，授权后再试）。 */
function registerBridgeListener(): void {
  const event = browser.runtime.onUserScriptMessage;
  if (bridgeListening || !event || !services) return;
  bridgeListening = true;
  const { bridge } = services;
  const errorReply = (e: unknown) => ({ ok: false as const, error: e instanceof Error ? e.message : String(e) });
  if (import.meta.env.FIREFOX) {
    event.addListener((message, sender) => {
      // 类型声明只允许 void 返回；Firefox 运行时接受 Promise。
      return bridge(message, sender).catch(errorReply) as unknown as void;
    });
  } else {
    event.addListener((message, sender, sendResponse) => {
      bridge(message, sender)
        .catch(errorReply)
        .then(sendResponse);
      return true as unknown as void;
    });
  }
}

async function syncAlarm(hasApprovals: boolean): Promise<void> {
  try {
    if (!hasApprovals) {
      await browser.alarms.clear(SYNC_ALARM);
      return;
    }
    // 同名 alarm 重复 create 会重置计时：已存在且周期一致时不重建。
    const existing = await browser.alarms.get(SYNC_ALARM);
    if (existing?.periodInMinutes === SYNC_PERIOD_MIN) return;
    await browser.alarms.create(SYNC_ALARM, { periodInMinutes: SYNC_PERIOD_MIN });
  } catch (e) {
    logWarn("[FluxDown] page scripts alarm setup failed:", e);
  }
}

/** 顶层同步调用：创建服务并注册全部监听器。 */
export function installPageScripts(): void {
  if (services) return;
  services = createServices();
  const { store } = services;

  registerBridgeListener();

  browser.runtime.onInstalled.addListener(() => reconcileInBackground("installed"));
  browser.runtime.onStartup.addListener(() => reconcileInBackground("startup"));
  browser.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === SYNC_ALARM) reconcileInBackground("alarm");
  });
  // `userScripts` 权限授予后：Firefox 此时才有 onUserScriptMessage；同时重新同步。
  browser.permissions?.onAdded?.addListener((permissions) => {
    if (!permissions.permissions?.includes("userScripts")) return;
    registerBridgeListener();
    reconcileInBackground("permission");
  });
  onCapabilities((caps) => {
    if (caps.connected) reconcileInBackground("capabilities", toSyncCaps(caps));
  });

  store
    .loadApprovals()
    .then((approvals) => syncAlarm(Object.keys(approvals).length > 0))
    .catch((e) => logWarn("[FluxDown] page scripts approvals load failed:", e));
}

// ===== 给 messages.ts 的 API =====

function requireServices(): Services {
  if (!services) throw new Error("page scripts not installed");
  return services;
}

export async function getPageScriptsStatus(refresh: "none" | "stale" | "force" = "none"): Promise<PageScriptsStatus> {
  const { store, sync } = requireServices();
  if (refresh === "force") {
    await sync.reconcile("user").catch((e) => logWarn("[FluxDown] page scripts sync failed:", e));
  } else if (refresh === "stale") {
    const state = await store.loadSync();
    if (state.lastAttemptAt === undefined || Date.now() - state.lastAttemptAt > STALE_AFTER_MS) {
      await sync.reconcile("view").catch((e) => logWarn("[FluxDown] page scripts sync failed:", e));
    }
  }
  const [state, approvals, availability] = await Promise.all([
    store.loadSync(),
    store.loadApprovals(),
    probeAvailability(browserAvailabilityEnv),
  ]);
  return buildStatus(state, approvals, availability);
}

/** 首次启用：记录批准（生成令牌）并立即同步。调用前选项页须已在用户手势里请求到 `userScripts` 权限。 */
export async function approvePageScripts(identity: string): Promise<PageScriptsActionResult> {
  const { store, sync } = requireServices();
  const state = await store.loadSync();
  const entry = state.index.find((item) => item.identity === identity);
  if (!entry) return { ok: false, message: "plugin_not_provided", status: await getPageScriptsStatus() };
  await store.approve(identity, entry.pluginName, Date.now());
  await syncAlarm(true);
  registerBridgeListener();
  await sync.reconcile("user").catch((e) => logWarn("[FluxDown] page scripts sync failed:", e));
  return { ok: true, status: await getPageScriptsStatus() };
}

/** 停用并移除：先删批准记录与令牌（桥立刻拒绝该插件），再注销全部脚本。 */
export async function revokePageScripts(identity: string): Promise<PageScriptsActionResult> {
  const { store, sync } = requireServices();
  await store.revoke(identity);
  try {
    await sync.removePlugin(identity);
  } catch (e) {
    logWarn("[FluxDown] page scripts unregister failed:", e);
    return { ok: false, message: e instanceof Error ? e.message : String(e), status: await getPageScriptsStatus() };
  }
  await syncAlarm(Object.keys(await store.loadApprovals()).length > 0);
  return { ok: true, status: await getPageScriptsStatus() };
}

/** 「查看源码」：按需从桌面端拉取，校验 sha256 后只读返回（不落 storage）。 */
export async function viewPageScriptSource(identity: string, id: string): Promise<PageScriptSourceResult> {
  const { store } = requireServices();
  const state = await store.loadSync();
  const entry = state.index.find((item) => item.identity === identity && item.id === id);
  if (!entry) return { ok: false, message: "script_not_found" };
  const reply = await fetchPageScriptSource(identity, id);
  if (!reply.ok) return { ok: false, message: reply.message ?? "fetch_failed" };
  const actual = await sha256Hex(reply.source.code);
  if (reply.source.sha256 !== entry.sha256 || actual !== entry.sha256) return { ok: false, message: "sha_mismatch" };
  return { ok: true, code: reply.source.code, sha256: actual };
}
