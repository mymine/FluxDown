/**
 * 交接标签页：把「唤起外部 App」的 URL（`fluxdown://` 协议、Android `intent:`）交给浏览器导航。
 * 复用单个标签页（连续下载不开一排标签），并在最后一次唤起 4s 后自动回收。
 */
import { browser } from "wxt/browser";
import { logWarn } from "@/utils/error-log";

let handoffTabId: number | null = null;
let handoffTabCloseTimer: ReturnType<typeof setTimeout> | undefined;

/** 在交接标签页中打开 [url]；浏览器拒绝创建 / 更新标签页时返回 false。 */
export async function openInHandoffTab(url: string): Promise<boolean> {
  try {
    if (handoffTabId !== null) {
      try {
        await browser.tabs.update(handoffTabId, { url, active: true });
      } catch {
        // 标签页已被用户关闭 — 重新创建
        handoffTabId = null;
      }
    }
    if (handoffTabId === null) {
      const tab = await browser.tabs.create({ url, active: true });
      handoffTabId = tab.id ?? null;
    }
    // 每次唤起重置回收定时器：4s 无新请求则关闭遗留的空白/错误标签页
    clearTimeout(handoffTabCloseTimer);
    handoffTabCloseTimer = setTimeout(() => {
      const id = handoffTabId;
      handoffTabId = null;
      if (id !== null) {
        browser.tabs.remove(id).catch(() => {});
      }
    }, 4000);
    return true;
  } catch (e) {
    logWarn("[FluxDown] failed to open handoff URL:", e);
    return false;
  }
}
