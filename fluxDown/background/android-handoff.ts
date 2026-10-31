/**
 * Android 本机通道：Android 版浏览器没有 Native Messaging，下载经 Intent URI 显式指定包名唤起
 * FluxDown App（构造见 utils/android-intent.ts），不依赖 `fluxdown://` 协议开关。
 * utils/download-dispatch.ts 在 Android 上用它代替 NMH 作为「本机」通道。
 *
 * Intent 是单向投递：浏览器不回报 App 是否收到，因此本通道只报告「已交给浏览器」；
 * 未安装 App 时 Chrome 不响应、Firefox 转去应用商店（Intent 带包名时生成的 market 意图）。
 */
import { browser } from "wxt/browser";
import { buildAndroidIntentUrl } from "@/utils/android-intent";
import { logWarn } from "@/utils/error-log";
import type { ApiResponse, BatchDownloadItem, DownloadRequest } from "@/utils/native-messaging";
import { openInHandoffTab } from "./handoff-tab";
import { sleep } from "./notify";

/**
 * Intent 承载不了的请求（音视频分轨对、POST 事务、App 不接收的 scheme）：App 什么也没收到，
 * 不属于「不可达」；fallback 模式据此改投远程，否则按业务拒绝回退浏览器下载。
 */
export const ANDROID_UNSUPPORTED_MESSAGES: ReadonlySet<string> = new Set([
  "android_audio_pair_unsupported",
  "android_post_unsupported",
  "android_scheme_unsupported",
]);

/** 批量逐条唤起的间隔：App 的「新建下载」弹窗把弹窗打开期间到达的后续唤起追加进同一表单。 */
const BATCH_INTERVAL_MS = 800;

function unsupportedReason(item: { audioUrl?: string; method?: string }): string | null {
  // 分轨对只发视频轨会得到无声视频，必须交给能 mux 的通道。
  if (item.audioUrl) return "android_audio_pair_unsupported";
  if (item.method && item.method.toUpperCase() !== "GET") return "android_post_unsupported";
  return null;
}

/**
 * 打开 Intent URI。优先在当前网页里导航：这是网页发起的导航，Chrome 与 Firefox 的外部应用拦截都会处理；
 * `intent:` 导航不会提交，页面保持原样。扩展注入的导航不一定带用户手势，此时 Chrome 弹「继续前往
 * FluxDown」提示而不是直接打开；Firefox 按「在应用中打开链接」设置直接打开或询问。
 * 当前页不可注入（浏览器内部页 / 无站点权限）时退回交接标签页（浏览器发起的导航，Chrome 同样只提示）。
 */
async function openIntent(intentUrl: string): Promise<boolean> {
  try {
    const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
    if (tab?.id !== undefined && /^https?:/i.test(tab.url ?? "")) {
      await browser.scripting.executeScript({
        target: { tabId: tab.id },
        func: (target: string) => {
          window.location.href = target;
        },
        args: [intentUrl],
      });
      return true;
    }
  } catch (e) {
    logWarn("[FluxDown] android handoff: page navigation failed, using a handoff tab:", e);
  }
  return openInHandoffTab(intentUrl);
}

async function handOff(item: DownloadRequest | BatchDownloadItem): Promise<ApiResponse> {
  const reason = unsupportedReason(item);
  if (reason) return { success: false, message: reason };
  const intentUrl = buildAndroidIntentUrl({
    url: item.url,
    filename: item.filename,
    cookies: item.cookies,
    referrer: item.referrer,
    headers: item.headers,
  });
  if (!intentUrl) return { success: false, message: "android_scheme_unsupported" };
  return (await openIntent(intentUrl)) ? { success: true } : { success: false, message: "android_handoff_failed" };
}

/** 单条下载交给 FluxDown Android App。 */
export function androidSendDownloadRequest(request: DownloadRequest): Promise<ApiResponse> {
  return handOff(request);
}

/** 批量下载逐条唤起；任何一条 Intent 承载不了时整批拒绝（交给远程或浏览器整体处理，不拆散一批）。 */
export async function androidSendBatchDownloadRequest(items: BatchDownloadItem[]): Promise<ApiResponse> {
  for (const item of items) {
    const reason = unsupportedReason(item);
    if (reason) return { success: false, message: reason };
  }
  let sent = 0;
  let last: ApiResponse = { success: false, message: "android_handoff_failed" };
  for (const [index, item] of items.entries()) {
    if (index > 0) await sleep(BATCH_INTERVAL_MS);
    last = await handOff(item);
    if (last.success) sent++;
  }
  return sent > 0 ? { success: true } : last;
}
