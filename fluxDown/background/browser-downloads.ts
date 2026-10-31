/**
 * 浏览器自己下载的文件的可见反馈（Chromium）。
 *
 * 拦截开启时 `syncDownloadShelfState` 关闭了浏览器下载 UI（整个 profile 生效，Chromium
 * `ShouldShowDownloadBubble` 只要有扩展关闭 UI 就不显示气泡与工具栏图标）。插件放行给
 * 浏览器的下载（blob:/data:、SW 合成、类型 / 大小不符、站点不接管、绕过令牌、熔断回退）
 * 因此会静默落盘，用户以为没下载而反复点击。
 *
 * 反馈优先在发起页内显示（sensor 顶层帧的提示卡，不依赖系统通知权限）；页面收不到
 * （无内容脚本的页面、标签页已关）时退回系统通知。同一下载的「开始 / 完成」走同一通道。
 * 被接管的下载都已取消，到不了 in_progress 确认点与 complete。
 */
import { browser } from "wxt/browser";
import { t } from "@/utils/i18n";
import type { BrowserDownloadNotice } from "./protocol";
import { getCachedSettings } from "./settings-cache";
import { logWarn } from "@/utils/error-log";
import { hasDownloadsApi } from "@/utils/platform";

const NOTIFICATION_PREFIX = "fluxdown-browser-download-";

/** 文件名确定后再等这么久仍在下载才算浏览器接下了：留给拦截层的取消落定。 */
const START_CONFIRM_MS = 500;

interface Tracked {
  /** 发起下载时的活动标签页（DownloadItem 不带 tabId）。 */
  tabId: number | undefined;
  /** 已用的反馈通道；undefined = 尚未提示过。 */
  channel?: "page" | "system";
  startTimer?: ReturnType<typeof setTimeout>;
}

const tracked = new Map<number, Tracked>();

/** 当前浏览器是否支持（因而会被本扩展）隐藏下载 UI；Firefox（含没有 downloads API 的 Android 版）没有这些 API。 */
function canHideDownloadUi(): boolean {
  if (!hasDownloadsApi()) return false;
  const downloads = browser.downloads as unknown as Record<string, unknown>;
  return typeof downloads.setShelfEnabled === "function" || typeof downloads.setUiOptions === "function";
}

/** 路径末段（Windows / POSIX 分隔符都认）。 */
function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

async function activeTabId(): Promise<number | undefined> {
  const [tab] = await browser.tabs.query({ active: true, lastFocusedWindow: true });
  return tab?.id;
}

async function deliverToPage(tabId: number | undefined, notice: BrowserDownloadNotice): Promise<boolean> {
  if (tabId === undefined) return false;
  try {
    const reply: unknown = await browser.tabs.sendMessage(tabId, notice, { frameId: 0 });
    return (reply as { ok?: unknown } | undefined)?.ok === true;
  } catch {
    return false; // 页面没有存活的 sensor（chrome:// 页、PDF、已关闭）
  }
}

async function deliverToSystem(notice: BrowserDownloadNotice): Promise<void> {
  if (!browser.notifications?.create || notice.phase === "failed") return;
  const started = notice.phase === "started";
  await browser.notifications.create(`${NOTIFICATION_PREFIX}${notice.downloadId}`, {
    type: "basic",
    iconUrl: "/icon/128.png",
    title: t(started ? "notify.browserDownloadStartedTitle" : "notify.browserDownloadDoneTitle"),
    message: t(started ? "notify.browserDownloadStartedDetail" : "notify.browserDownloadDoneDetail", {
      name: notice.name,
    }),
  });
}

async function deliver(entry: Tracked, notice: BrowserDownloadNotice): Promise<void> {
  if (entry.channel !== "system" && (await deliverToPage(entry.tabId, notice))) {
    entry.channel = "page";
    if (entry.tabId !== undefined) rememberShown(notice.downloadId, entry.tabId);
    return;
  }
  if (entry.channel === "page" && notice.phase === "failed") return;
  entry.channel = "system";
  await deliverToSystem(notice);
}

/** 浏览器确实在下载（文件名已定、取消未发生）且 UI 被隐藏时提示「正在下载」。 */
async function confirmStarted(downloadId: number): Promise<void> {
  const entry = tracked.get(downloadId);
  if (!entry || entry.channel) return;
  // 与 syncDownloadShelfState 同一判据：拦截开启 = 下载 UI 被隐藏。
  if (!(await getCachedSettings()).enabled) return;
  const [item] = await browser.downloads.search({ id: downloadId });
  if (!item || item.state !== "in_progress" || !item.filename) return;
  await deliver(entry, { action: "browserDownload", phase: "started", downloadId, name: baseName(item.filename) });
}

function scheduleStart(downloadId: number): void {
  const entry = tracked.get(downloadId);
  if (!entry || entry.channel || entry.startTimer) return;
  entry.startTimer = setTimeout(() => {
    entry.startTimer = undefined;
    confirmStarted(downloadId).catch((e) => logWarn("[FluxDown] browser download start notice failed:", e));
  }, START_CONFIRM_MS);
}

async function onFinished(downloadId: number, state: "complete" | "interrupted"): Promise<void> {
  const entry = tracked.get(downloadId);
  tracked.delete(downloadId);
  clearTimeout(entry?.startTimer);
  if (state === "interrupted") {
    // 只收起已显示的「正在下载」；被接管的下载从未提示过
    if (entry?.channel) await deliver(entry, { action: "browserDownload", phase: "failed", downloadId, name: "" });
    return;
  }
  if (!(await getCachedSettings()).enabled) return;
  const [item] = await browser.downloads.search({ id: downloadId });
  if (!item || item.state !== "complete" || !item.exists) return;
  await deliver(entry ?? { tabId: await activeTabId() }, {
    action: "browserDownload",
    phase: "done",
    downloadId,
    name: baseName(item.filename),
  });
}

/** 页面提示卡「在文件夹中显示」：只放行本标签页收到过提示的下载。 */
export function showBrowserDownload(downloadId: number, tabId: number | undefined): boolean {
  if (tabId === undefined || !shownInTab.get(downloadId)?.has(tabId)) return false;
  browser.downloads.show(downloadId);
  return true;
}

/** 下载 id → 收到过页面提示的标签页（完成后仍需响应点击，独立于 tracked 的生命周期）。 */
const shownInTab = new Map<number, Set<number>>();
const SHOWN_LIMIT = 64;

function rememberShown(downloadId: number, tabId: number): void {
  const set = shownInTab.get(downloadId) ?? new Set<number>();
  set.add(tabId);
  shownInTab.set(downloadId, set);
  if (shownInTab.size > SHOWN_LIMIT) {
    const oldest = shownInTab.keys().next().value;
    if (oldest !== undefined) shownInTab.delete(oldest);
  }
}

/** 同步注册监听；由 startBackground 调用一次。 */
export function installBrowserDownloadNotices(): void {
  if (!canHideDownloadUi()) return;
  browser.downloads.onCreated.addListener((item) => {
    const entry: Tracked = { tabId: undefined };
    tracked.set(item.id, entry);
    activeTabId()
      .then((tabId) => {
        entry.tabId = tabId;
        if (item.filename && item.state === "in_progress") scheduleStart(item.id);
      })
      .catch((e) => logWarn("[FluxDown] active tab lookup failed:", e));
  });
  browser.downloads.onChanged.addListener((delta) => {
    const state = delta.state?.current;
    if (state === "complete" || state === "interrupted") {
      onFinished(delta.id, state).catch((e) => logWarn("[FluxDown] browser download notice failed:", e));
      return;
    }
    if (delta.filename?.current) scheduleStart(delta.id);
  });
  browser.notifications?.onClicked.addListener((notificationId) => {
    if (!notificationId.startsWith(NOTIFICATION_PREFIX)) return;
    const downloadId = Number(notificationId.slice(NOTIFICATION_PREFIX.length));
    if (Number.isInteger(downloadId)) {
      try {
        browser.downloads.show(downloadId);
      } catch (e) {
        logWarn("[FluxDown] downloads.show failed:", e);
      }
    }
    Promise.resolve(browser.notifications.clear(notificationId)).catch((e) => {
      logWarn("[FluxDown] notifications.clear failed:", e);
    });
  });
}
