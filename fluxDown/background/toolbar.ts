/** 工具栏图标与角标。 */
import { browser } from "wxt/browser";

/** MV3 为 action，Firefox MV2 构建只有 browserAction。 */
export function getActionApi(): typeof browser.action | undefined {
  // wxt 的 browser 类型只声明 MV3 的 action；browserAction 仅在 Firefox MV2 运行时存在。
  const mv2Browser = browser as unknown as {
    browserAction?: typeof browser.action;
  };
  return browser.action ?? mv2Browser.browserAction;
}

export async function updateBadgeForTab(tabId: number, count: number): Promise<void> {
  const text = count > 0 ? String(count) : "";
  const action = getActionApi();
  try {
    await action?.setBadgeText({ text, tabId });
    if (count > 0) {
      await action?.setBadgeBackgroundColor({ color: "#3B82F6", tabId });
    }
  } catch {
    // tab 可能已关闭
  }
}

export function updateIcon(enabled: boolean) {
  const suffix = enabled ? "" : "-disabled";
  const iconPath = {
    16: `/icon/16${suffix}.png`,
    32: `/icon/32${suffix}.png`,
    48: `/icon/48${suffix}.png`,
    128: `/icon/128${suffix}.png`,
  };
  getActionApi()
    ?.setIcon({ path: iconPath })
    ?.catch(() => {
      /* 权限不足时静默忽略 */
    });
}
