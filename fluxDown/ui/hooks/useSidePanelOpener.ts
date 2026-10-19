/**
 * 打开浏览器侧边栏（popup / options 共用）。
 *
 * - Chrome：`sidePanel.open({ windowId })`；
 * - Firefox：`sidebarAction.open()`。
 * 两者都要求在用户手势的同步调用栈内触发，因此 windowId 在挂载时先缓存好，
 * `open()` 内部不得 await 任何东西。API 不存在时 `available` 为 false，调用方应隐藏按钮。
 */
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";

/** Firefox 的 sidebarAction 不在 @wxt-dev/browser 的类型里，只声明用到的部分。 */
interface FirefoxExtras {
  sidebarAction?: { open(): Promise<void> };
}

// @wxt-dev/browser 只携带 Chromium 类型；Firefox 独有的 sidebarAction 运行时存在，需放宽类型读取。
const firefoxExtras = browser as unknown as FirefoxExtras;

export interface SidePanelOpener {
  /** 当前浏览器支持侧边栏。 */
  available: boolean;
  /** 在 click 回调中同步调用。 */
  open: () => void;
}

/**
 * @param closeAfterOpen 成功打开后关闭当前页面（popup 传 true；options 标签页不要关）。
 */
export function useSidePanelOpener(closeAfterOpen: boolean): SidePanelOpener {
  const sidebar =
    typeof firefoxExtras.sidebarAction?.open === "function" ? firefoxExtras.sidebarAction : null;
  const chromePanel = typeof browser.sidePanel?.open === "function";
  const [windowId, setWindowId] = useState<number | null>(null);

  useEffect(() => {
    if (!chromePanel || sidebar) return;
    let alive = true;
    browser.windows
      .getCurrent()
      .then((w) => {
        if (alive && typeof w.id === "number") setWindowId(w.id);
      })
      .catch(() => {
        // 取不到窗口时 open() 保持不可用
      });
    return () => {
      alive = false;
    };
  }, [chromePanel, sidebar]);

  const open = () => {
    let pending: Promise<void>;
    if (sidebar) {
      pending = sidebar.open();
    } else if (chromePanel && windowId !== null) {
      pending = browser.sidePanel.open({ windowId });
    } else {
      return;
    }
    pending
      .then(() => {
        if (closeAfterOpen) window.close();
      })
      .catch((e) => console.warn("[FluxDown] open side panel failed:", e));
  };

  return { available: sidebar !== null || chromePanel, open };
}
