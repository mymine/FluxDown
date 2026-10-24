/** 工具栏图标与角标（Chrome / Firefox 均为 MV3 action）。 */
import { browser } from "wxt/browser";

export async function updateBadgeForTab(tabId: number, count: number): Promise<void> {
  const text = count > 0 ? String(count) : "";
  const action = browser.action;
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
  browser.action
    ?.setIcon({ path: iconPath })
    ?.catch(() => {
      /* 权限不足时静默忽略 */
    });
}
