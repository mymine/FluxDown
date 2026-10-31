/**
 * 拦截放行状态：
 * - 绕过令牌：回退浏览器下载的 URL 在 15 s 内所有拦截层一致放行；
 * - 跳过接管修饰键（IDM 式 Alt+点击）：按住期间发起的请求与其重定向目标领取绕过令牌；
 * - App 可用性熔断器（#308 #346 弹窗风暴修复）。
 */
import { browser } from "wxt/browser";
import { checkFluxDownAvailableWithRetry } from "@/utils/download-dispatch";
import { logWarn } from "@/utils/error-log";

// 绕过令牌：URL → 过期时间戳，15 秒内有效
const bypassTokens = new Map<string, number>();
// Bug R3-2 修复：周期性清理 bypassTokens 中的过期条目，防止长期积累内存泄漏
setInterval(() => {
  const now = Date.now();
  for (const [tokenUrl, expiry] of bypassTokens) {
    if (expiry <= now) bypassTokens.delete(tokenUrl);
  }
  for (const [tabId, until] of bypassArmedTabs) {
    if (until <= now) bypassArmedTabs.delete(tabId);
  }
}, 60_000); // 每 60 秒清理一次

/**
 * 绕过令牌检查（基于时间，不消费）。
 *
 * 关键修复（#308 #346 弹窗风暴根因）：bypassTokens 必须按"过期时间"语义工作，
 * 读取时**不删除**。原实现"读取即删除"导致回退重发起的下载被第二层
 * (onDeterminingFilename) 放行并消费令牌后，第三层 (onCreated 兜底) 再次查不到
 * 令牌 → 重新拦截 → 再次发送失败 → 再次回退 → 无限弹窗循环。
 * 改为时间令牌后，同一次回退产生的下载在所有拦截层都被一致放行。
 */
export function hasActiveBypass(url: string): boolean {
  // 判定时修饰键仍按着（浏览器自建的下载、请求监听未覆盖的路径）：同样放行，并发令牌让后续拦截层一致
  if (bypassKeyHeld()) {
    allowBypass(url);
    return true;
  }
  const expiry = bypassTokens.get(url);
  return expiry !== undefined && expiry > Date.now();
}

// ── 跳过接管修饰键 ────────────────────────────────────────────────
// 两路信号，任一命中就把请求 URL 快照成令牌（响应头 / 下载项到达时用户往往已松手）：
// 1. 按住状态：全局而非按 tab（与 IDM 一致），覆盖浏览器自建的下载（Alt+点击另存）等无 tab 的请求。
//    只靠它会有两个洞：松手的 keyup 消息可能先于请求发起到达；页面在点击后先 await 再
//    程序化 a.click()（gopeed.com 先 HEAD 探测镜像，最长 3 s）时键早已松开。
// 2. 点击手势：带修饰键的真实点击立即为链接 href 发令牌，并让该 tab 在随后
//    BYPASS_CLICK_WINDOW_MS 内发起的请求都领令牌——意图绑定在点击上，与松手时机无关。
// sensor 在失焦 / 隐藏时上报松开；keyup 仍可能丢失，按住状态超过 30 s 未刷新视为已松开。
const BYPASS_KEY_STALE_MS = 30_000;
// 4 s：覆盖 gopeed.com 这类先探测镜像（单路超时 3 s）再跳转的页面，并留 1 s 余量。
const BYPASS_CLICK_WINDOW_MS = 4_000;
let bypassKeySince = 0;
/** tabId → 点击放行窗口截止时间戳。 */
const bypassArmedTabs = new Map<number, number>();

/** sensor 上报修饰键按下 / 松开（由 messages.bypassKey 调用）。 */
export function setBypassKeyHeld(held: boolean): void {
  bypassKeySince = held ? Date.now() : 0;
}

/** sensor 上报一次带修饰键的真实点击（由 messages.bypassClick 调用）。 */
export function armBypassClick(tabId: number | undefined, href: string | undefined): void {
  if (href) allowBypass(href);
  if (tabId !== undefined) bypassArmedTabs.set(tabId, Date.now() + BYPASS_CLICK_WINDOW_MS);
}

function bypassKeyHeld(): boolean {
  return bypassKeySince > 0 && Date.now() - bypassKeySince < BYPASS_KEY_STALE_MS;
}

function onBypassRequestStart(details: { url: string; tabId: number }): undefined {
  const armedUntil = bypassArmedTabs.get(details.tabId);
  if (bypassKeyHeld() || (armedUntil !== undefined && armedUntil > Date.now())) allowBypass(details.url);
  return undefined;
}

function onBypassRedirect(details: { url: string; redirectUrl: string }): void {
  const expiry = bypassTokens.get(details.url);
  if (expiry !== undefined && expiry > Date.now()) allowBypass(details.redirectUrl);
}

/**
 * 注册请求发起 / 重定向监听（所有资源类型：Alt+点击时浏览器自建的下载、XHR 触发的下载
 * 都不是 main_frame）；由 startBackground 调用一次。
 */
export function installBypassKey(): void {
  const filter = { urls: ["<all_urls>"] };
  try {
    browser.webRequest.onBeforeRequest.addListener(onBypassRequestStart, filter);
    browser.webRequest.onBeforeRedirect.addListener(onBypassRedirect, filter);
  } catch (e) {
    logWarn("[FluxDown] Failed to register bypass-key request listeners:", e);
  }
}

// ── App 可用性熔断器（#308 #346 弹窗风暴修复）────────────────────────
// 当发送失败且**重连重试 ping 仍确认 App 不可达**时，进入冷却期：冷却期内**跳过拦截**，
// 让浏览器原生下载，避免"拦截→发送失败→回退→再拦截"的反复弹窗。
// 恢复路径（缩短"App 已恢复却仍被旁路"的死区，review 发现 #1/#4/#6）：
//   1) 冷却期到期后下一次下载尝试发送，成功即解除（兜底）；
//   2) popup 打开时 ping 成功立即解除（用户启动 App 后常见操作）；
//   3) 待发队列 flush 成功时解除。
// 冷却期取 30s（短于 60s 以缩小误熔断/陈旧冷却的影响半径，review 发现 #3/#5）。
const APP_DOWN_COOLDOWN_MS = 30_000;
// storage.session 持久化键：让熔断状态在 MV3 Service Worker 重启后仍生效，
// 否则每次 SW 冷启动都会重新拦截一次造成跨生命周期的弹窗。
const APP_DOWN_STATE_KEY = "fluxdownAppDownUntil";
let _appDownUntil = 0;
// 熔断期内"未检测到 App"提示只弹一次，解除熔断时复位。
let _appDownNotified = false;
// 标记熔断状态是否已被一次实时的 markAppDown/markAppUp 改写过。
// 用于防止 SW 冷启动时异步恢复的旧持久值，覆盖掉启动期间刚到达的实时恢复信号
// （Chrome MV3：appConfirmedUp 消息可能正是唤醒 SW 的那条，markAppUp 先跑、
//  随后 storage.session.get 才 resolve 并把旧 until 写回，造成已恢复却又被熔断）。
// review round2 发现 #1。
let _breakerStateTouched = false;

export function isAppKnownDown(): boolean {
  return Date.now() < _appDownUntil;
}

function persistAppDownState(until: number): void {
  // storage.session 在部分旧版浏览器不可用；best-effort，失败不影响内存态熔断。
  try {
    (browser.storage as any).session
      ?.set({ [APP_DOWN_STATE_KEY]: until })
      ?.catch(() => {});
  } catch {
    /* ignore */
  }
}

export function markAppDown(): void {
  // 无条件置位：即使本次是 no-op，也代表"有过实时熔断/恢复判定"，
  // 启动恢复必须让位（见 _breakerStateTouched 说明）。
  _breakerStateTouched = true;
  _appDownUntil = Date.now() + APP_DOWN_COOLDOWN_MS;
  persistAppDownState(_appDownUntil);
}

export function markAppUp(): void {
  // 无条件置位：即便内存态已是"up"（guard 跳过），一条实时的"App 可达"信号
  // 也应阻止后续异步恢复把旧的 down 状态写回（review round2 发现 #1）。
  _breakerStateTouched = true;
  if (_appDownUntil !== 0 || _appDownNotified) {
    _appDownUntil = 0;
    _appDownNotified = false;
    persistAppDownState(0);
  }
}

// 熔断确认探测的单航班守卫：并发下载在 cooldown 到期同时失败时，
// 共享同一次重连重试 ping，避免各自独立 ping 造成的连接抖动（review round2 发现 #4）。
let _appProbe: Promise<boolean> | null = null;
export function probeAppReachable(): Promise<boolean> {
  if (!_appProbe) {
    _appProbe = checkFluxDownAvailableWithRetry()
      .catch(() => false)
      .finally(() => {
        _appProbe = null;
      });
  }
  return _appProbe;
}

/** 放行某个 URL 15 s（回退浏览器下载时调用）。 */
export function allowBypass(url: string): void {
  bypassTokens.set(url, Date.now() + 15_000);
}

/** 熔断期内「未检测到 App」提示每轮只弹一次：首次调用返回 true。 */
export function claimAppDownNotice(): boolean {
  if (_appDownNotified) return false;
  _appDownNotified = true;
  return true;
}

/** SW 启动时从 storage.session 恢复熔断状态（best-effort）。 */
export function restoreBreakerState(): void {
  void (async () => {
    try {
      const s = await (browser.storage as any).session?.get(APP_DOWN_STATE_KEY);
      const until = s?.[APP_DOWN_STATE_KEY];
      // 仅当启动期间没有任何实时熔断/恢复发生时才采用持久值，避免覆盖刚到达的
      // markAppUp/markAppDown（review round2 发现 #1）。
      if (
        !_breakerStateTouched &&
        typeof until === "number" &&
        until > Date.now()
      ) {
        _appDownUntil = until;
      }
    } catch {
      /* ignore */
    }
  })();
}
