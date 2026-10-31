/**
 * 页面探测的存活检测与恢复。
 *
 * sensor 是声明式内容脚本：浏览器在扩展安装 / 更新 / 重载后不会给已打开的页面补注入，旧实例的
 * 扩展上下文失效后整页只剩 webRequest 嗅探，所有候选都失去元素证据（「未关联」、标题错位）。
 *
 * 检测：sensor 每次拉配置都登记会话 sid（tab-state）。顶层帧没有会话的 http(s) 页面加载完成后，
 * 先 `probe` 探测（SW 重启丢了登记时活着的 sensor 会应答并全量重发），无应答才补注入 MAIN 钩子
 * 与 sensor。全程事件驱动：不轮询、不保持长连接（长连接会让 MV3 Service Worker 反复被唤醒）。
 *
 * 每个文档最多补注入一次；失败（受限页 / 注入被拒）记为 unavailable，直到 tab 开始加载新文档。
 * 从不自动刷新页面：降级（钩子补注入前页面已在播放）由 UI 提示用户刷新。
 */
import { browser } from "wxt/browser";
import type { ContentMessage, HooksInfo, ProbeReply, ProbeState } from "./protocol";
import { getSession, markTabChanged, onTabChanged, registerSession, setSessionHooks, whenRestored } from "./tab-state";
import { logWarn } from "@/utils/error-log";

/** 触发后等待的时间：给正常加载的 sensor 留出登记窗口，并合并同一 tab 的连续触发。 */
const CHECK_DELAY_MS = 1_000;
const PING_TIMEOUT_MS = 1_000;
/** 补注入后等新 sensor 登记会话的上限。 */
const SESSION_WAIT_MS = 5_000;
/** WXT 内容脚本产物路径（entrypoints/page-hooks.content.ts、sensor.content.ts、panel.content/）。 */
const HOOKS_FILE = "/content-scripts/page-hooks.js";
const SENSOR_FILE = "/content-scripts/sensor.js";
const PANEL_FILE = "/content-scripts/panel.js";

/** 本文档的恢复结论；tab 开始加载新文档时清除。 */
const outcomes = new Map<number, "recovering" | "unavailable">();
const timers = new Map<number, ReturnType<typeof setTimeout>>();
const running = new Set<number>();
const sessionWaiters = new Map<number, () => void>();

/** 视图用的基础探测状态（degraded 由视图模型按当前播放元素判定）。 */
export function probeStateOf(tabId: number): ProbeState {
  if (getSession(tabId, 0)) return "live";
  return outcomes.get(tabId) ?? "unknown";
}

/** 安排一次检查：顶层帧已有会话、已在检查、本文档已有结论时直接返回（O(1)，可在热路径调用）。 */
export function scheduleProbeCheck(tabId: number, delay = CHECK_DELAY_MS): void {
  if (tabId < 0 || timers.has(tabId) || running.has(tabId) || outcomes.has(tabId) || getSession(tabId, 0)) return;
  timers.set(
    tabId,
    setTimeout(() => {
      timers.delete(tabId);
      probeTab(tabId).catch((e) => logWarn("[FluxDown] probe check failed:", e));
    }, delay),
  );
}

function reset(tabId: number): void {
  clearTimeout(timers.get(tabId));
  timers.delete(tabId);
  outcomes.delete(tabId);
}

function isProbeReply(value: unknown): value is ProbeReply {
  if (!value || typeof value !== "object") return false;
  const reply = value as { sid?: unknown; hooks?: unknown };
  return typeof reply.sid === "string" && reply.sid !== "" && (reply.hooks === null || typeof reply.hooks === "object");
}

async function ping(tabId: number): Promise<ProbeReply | null> {
  const message: ContentMessage = { action: "probe" };
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), PING_TIMEOUT_MS);
  });
  try {
    const reply: unknown = await Promise.race([browser.tabs.sendMessage(tabId, message, { frameId: 0 }), timeout]);
    return isProbeReply(reply) ? reply : null;
  } catch {
    // 没有接收端：页面里没有存活的 sensor
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * 补注入：先注入 MAIN 钩子（已有任何版本的钩子时它自己放弃安装），再注入 sensor，最后注入页内面板
 * （悬浮球，只在顶层帧；它失败不影响探测恢复）。Chrome 与 Firefox（MV3）同用 scripting.executeScript。
 */
async function inject(tabId: number): Promise<void> {
  const target = { tabId, allFrames: true };
  await browser.scripting.executeScript({ target, files: [HOOKS_FILE], world: "MAIN", injectImmediately: true });
  await browser.scripting.executeScript({ target, files: [SENSOR_FILE], injectImmediately: true });
  await browser.scripting
    .executeScript({ target: { tabId, frameIds: [0] }, files: [PANEL_FILE] })
    .catch((e) => logWarn("[FluxDown] panel reinjection failed:", e));
}

function waitForSession(tabId: number): Promise<boolean> {
  if (getSession(tabId, 0)) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      sessionWaiters.delete(tabId);
      resolve(false);
    }, SESSION_WAIT_MS);
    sessionWaiters.set(tabId, () => {
      clearTimeout(timer);
      sessionWaiters.delete(tabId);
      resolve(true);
    });
  });
}

/**
 * 立即检查一个 tab：顶层帧没有 sensor 会话时探测，无应答则补注入。同一 tab 并发调用只跑一次；
 * 本文档已有结论（恢复中 / 不可注入）时不重试。
 */
export async function probeTab(tabId: number): Promise<void> {
  if (running.has(tabId)) return;
  running.add(tabId);
  try {
    await whenRestored();
    if (getSession(tabId, 0) || outcomes.has(tabId)) return;
    const tab = await browser.tabs.get(tabId).catch(() => undefined);
    // 加载中的页面由声明式内容脚本负责；加载完成（status complete）后的下一次触发再检查
    if (!tab?.url || !/^https?:/i.test(tab.url) || tab.discarded || tab.status !== "complete") return;

    const reply = await ping(tabId);
    if (reply) {
      // 活着但登记丢了（SW 重启后快照缺失）：补登记，sensor 已在应答后全量重发元素
      registerSession(tabId, 0, reply.sid);
      setSessionHooks(tabId, 0, reply.sid, reply.hooks as HooksInfo | null);
      return;
    }

    outcomes.set(tabId, "recovering");
    markTabChanged(tabId);
    let recovered = false;
    try {
      await inject(tabId);
      recovered = await waitForSession(tabId);
    } catch (e) {
      logWarn("[FluxDown] probe reinjection failed:", e);
    }
    // 期间 tab 已开始加载新文档（reset 清了结论）：结论作废，交给新文档的 sensor
    if (outcomes.get(tabId) !== "recovering") return;
    if (recovered) outcomes.delete(tabId);
    else outcomes.set(tabId, "unavailable");
    markTabChanged(tabId);
  } finally {
    running.delete(tabId);
  }
}

/** 恢复各窗口的当前页；其余页面在被激活、产生媒体请求或打开面板时按需恢复。 */
function recoverActiveTabs(): void {
  browser.tabs
    .query({ active: true })
    .then((tabs) => {
      for (const tab of tabs) if (tab.id !== undefined) scheduleProbeCheck(tab.id, 0);
    })
    .catch((e) => logWarn("[FluxDown] probe tabs query failed:", e));
}

/** 同步注册触发点；由 startBackground 调用一次。 */
export function installProbe(): void {
  // 扩展安装 / 更新 / 重载：已打开的页面里没有本版本的 sensor。
  browser.runtime.onInstalled.addListener(recoverActiveTabs);
  // Firefox MV3 的全站访问权限可被用户撤销后再授予：授予前补注入被拒、记为 unavailable 的页面
  // 需要重新检查（声明式内容脚本只对之后加载的文档生效）。
  browser.permissions?.onAdded?.addListener((permissions) => {
    if (!permissions.origins?.includes("<all_urls>")) return;
    for (const [tabId, outcome] of outcomes) if (outcome === "unavailable") outcomes.delete(tabId);
    recoverActiveTabs();
  });
  browser.tabs.onActivated.addListener(({ tabId }) => scheduleProbeCheck(tabId));
  browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
    if (changeInfo.status === "loading") reset(tabId);
    else if (changeInfo.status === "complete") scheduleProbeCheck(tabId);
  });
  browser.tabs.onRemoved.addListener((tabId) => {
    reset(tabId);
    sessionWaiters.delete(tabId);
  });
  browser.tabs.onReplaced.addListener((addedTabId, removedTabId) => {
    reset(removedTabId);
    scheduleProbeCheck(addedTabId, 0);
  });
  // 状态变化（含 webRequest 嗅探到媒体）：会话出现时唤醒等待者；缺会话的页面安排检查
  onTabChanged((tabId) => {
    if (!getSession(tabId, 0)) {
      scheduleProbeCheck(tabId);
      return;
    }
    sessionWaiters.get(tabId)?.();
  });
}
