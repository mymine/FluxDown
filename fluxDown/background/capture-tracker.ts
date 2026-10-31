/**
 * capture 跟踪运行时：登记发送成功后桌面端返回的事务、轮询状态、执行动作。
 * 仅本机 NMH 通道（远程通道没有 capture_status）。纯决策见 capture-logic.ts。
 */
import { browser } from "wxt/browser";
import type { Browser } from "wxt/browser";
import { t } from "@/utils/i18n";
import { DEFAULT_SITE_PROFILE, profileFor, saveSiteProfile } from "@/utils/site-profiles";
import { nextCaptureActions, pruneCancelStats, reconcilePending } from "./capture-logic";
import type { CancelStats, CaptureAction, PendingCapture } from "./capture-logic";
import { captureStatus } from "./host";
import { notify } from "./notify";
import { addSendObserver, fallbackToBrowserDownload } from "./sender";
import { getCachedProfiles, getCachedSettings } from "./settings-cache";
import { logWarn } from "@/utils/error-log";

const PENDING_KEY = "fluxdown_capture_pending";
const CANCEL_STATS_KEY = "fluxdown_cancel_stats";
const ASK_PREFIX = "fluxdown-ask-exclude-";
const POLL_INTERVAL_MS = 2_000;

let pending: PendingCapture[] = [];
let loaded: Promise<void> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let polling = false;
/** 串行化 pending 的读-改-写。 */
let chain: Promise<void> = Promise.resolve();

function hostOfUrl(url: string | undefined): string {
  if (!url) return "";
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

async function ensureLoaded(): Promise<void> {
  loaded ??= (async () => {
    try {
      const stored = (await browser.storage.session?.get(PENDING_KEY))?.[PENDING_KEY];
      if (Array.isArray(stored)) pending = [...(stored as PendingCapture[]), ...pending];
    } catch {
      /* session 不可用：只用内存态 */
    }
  })();
  await loaded;
}

function persistPending(): void {
  browser.storage.session?.set({ [PENDING_KEY]: pending }).catch(() => {});
}

function schedulePoll(): void {
  if (timer || pending.length === 0) return;
  timer = setTimeout(() => {
    timer = null;
    void poll();
  }, POLL_INTERVAL_MS);
}

async function loadStats(): Promise<CancelStats> {
  const raw = (await browser.storage.local.get(CANCEL_STATS_KEY))[CANCEL_STATS_KEY];
  return raw && typeof raw === "object" ? (raw as CancelStats) : {};
}

async function applyAction(action: CaptureAction): Promise<void> {
  if (action.kind === "fallback") {
    await fallbackToBrowserDownload(action.capture.url, action.capture.filename, true);
  } else if (action.kind === "recordReject") {
    const now = Date.now();
    const stats = pruneCancelStats(await loadStats(), now);
    stats[action.host] = [...(stats[action.host] ?? []), now];
    await browser.storage.local.set({ [CANCEL_STATS_KEY]: stats });
  } else {
    askExclude(action.host);
  }
}

function askExclude(host: string): void {
  if (!browser.notifications?.create) return;
  const opts: Browser.notifications.NotificationCreateOptions = {
    type: "basic",
    iconUrl: "/icon/128.png",
    title: t("site.askExclude.title"),
    message: t("site.askExclude.message", { host }),
  };
  if (!import.meta.env.FIREFOX) {
    opts.buttons = [{ title: t("site.askExclude.never") }, { title: t("site.askExclude.dismiss") }];
  }
  Promise.resolve(
    browser.notifications.create(`${ASK_PREFIX}${host}`, opts),
  ).catch(() => {});
}

async function poll(): Promise<void> {
  if (polling) return;
  polling = true;
  try {
    await ensureLoaded();
    if (pending.length === 0) return;
    const [settings, stats, profiles] = await Promise.all([
      getCachedSettings(),
      loadStats(),
      getCachedProfiles(),
    ]);
    const snapshot = pending;
    const snapshotIds = new Set(snapshot.map((p) => p.id));
    const statuses = await captureStatus(snapshot.map((p) => p.id));
    // 站点配置按后缀语义（profileFor）命中：子域名继承父域配置，不再重复询问。
    const profiledHosts = new Set(
      snapshot.map((p) => p.host).filter((host) => profileFor(profiles, host).key !== null),
    );
    const { actions, keep } = nextCaptureActions(
      snapshot,
      statuses,
      settings,
      stats,
      Date.now(),
      profiledHosts,
    );
    // 往返期间 trackCaptures 可能已追加新事务：与快照结果合并，不能整体覆盖。
    pending = reconcilePending(pending, snapshotIds, keep);
    persistPending();
    for (const action of actions) {
      await applyAction(action).catch((e) => logWarn("[FluxDown] capture action failed:", e));
    }
  } catch (e) {
    logWarn("[FluxDown] capture poll failed:", e);
  } finally {
    polling = false;
    schedulePoll();
  }
}

/** 登记一批事务（单条发送与批量发送共用）；`metas` 与 `ids` 按序对应，不足时复用首个。 */
export function trackCaptures(
  ids: string[],
  metas: { url: string; filename?: string; referrer?: string; pageHost?: string }[],
): void {
  if (ids.length === 0 || metas.length === 0) return;
  chain = chain.then(async () => {
    await ensureLoaded();
    const now = Date.now();
    ids.forEach((id, i) => {
      const meta = metas[i] ?? metas[0];
      pending.push({
        id,
        url: meta.url,
        filename: meta.filename,
        host: meta.pageHost || hostOfUrl(meta.referrer) || hostOfUrl(meta.url),
        createdAt: now,
      });
    });
    persistPending();
    schedulePoll();
  });
}

/** 从应答的 data 取 transactionIds。 */
export function transactionIdsOf(data: unknown): string[] {
  if (!data || typeof data !== "object" || !("transactionIds" in data)) return [];
  const ids = data.transactionIds;
  return Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string") : [];
}

/** 同步注册通知监听、接入 sender 的发送成功事件，并恢复未完成的跟踪。 */
export function installCaptureTracker(): void {
  addSendObserver((input, outcome) => {
    // 只跟踪拦截来源：手动发送后在桌面端取消不应触发回退或计入「排除此站」统计。
    if (outcome.channel !== "local" || input.origin !== "intercept") return;
    const ids = transactionIdsOf(outcome.data);
    trackCaptures(ids, [input]);
  });

  browser.notifications?.onButtonClicked?.addListener((notificationId, buttonIndex) => {
    if (!notificationId.startsWith(ASK_PREFIX)) return;
    const host = notificationId.slice(ASK_PREFIX.length);
    if (buttonIndex === 0) {
      saveSiteProfile(host, { ...DEFAULT_SITE_PROFILE, intercept: "never", updatedAt: Date.now() }).catch(() => {});
    } else {
      void loadStats()
        .then(async (stats) => {
          delete stats[host];
          await browser.storage.local.set({ [CANCEL_STATS_KEY]: stats });
        })
        .catch(() => {});
    }
    Promise.resolve(browser.notifications.clear(notificationId)).catch(() => {});
  });
  browser.notifications?.onClicked?.addListener((notificationId) => {
    // Firefox 通知不支持 buttons：点击正文打开站点配置。
    if (!import.meta.env.FIREFOX || !notificationId.startsWith(ASK_PREFIX)) return;
    const host = notificationId.slice(ASK_PREFIX.length);
    browser.tabs
      .create({ url: browser.runtime.getURL(`/options.html#sites?host=${encodeURIComponent(host)}` as "/options.html") })
      .catch(() => {});
    Promise.resolve(browser.notifications.clear(notificationId)).catch(() => {});
  });

  void ensureLoaded().then(schedulePoll);
}
