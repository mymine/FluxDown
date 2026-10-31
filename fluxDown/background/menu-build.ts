/**
 * 右键菜单整体重建（清空后按清单逐项创建），与浏览器 API 解耦以便测试。
 *
 * 浏览器启动 / 启用扩展时，Chrome 会异步把上次持久化的菜单恢复回来（MenuManager::ReadFromStorage）；
 * 恢复若落在 removeAll 与 create 之间，create 报重复 id、恢复出的旧项残留。恢复每次加载只发生一次，
 * 所以有项创建失败就整体再重建一次即可收敛；仍失败则交回原因，不无限重试。
 */
import type { Browser } from "wxt/browser";

export type MenuItemSpec = Browser.contextMenus.CreateProperties & { id: string };

export interface MenuApi {
  removeAll(): Promise<void>;
  /** 创建一项；`done` 收到失败原因（null = 成功）。 */
  create(item: MenuItemSpec, done: (error: string | null) => void): void;
}

export const MAX_REBUILD_ATTEMPTS = 2;

/** 清空后建全部项（父项须排在子项前）；返回最后一轮仍失败的原因，空数组 = 菜单与清单一致。 */
export async function rebuildMenus(api: MenuApi, items: readonly MenuItemSpec[]): Promise<string[]> {
  let errors: string[] = [];
  for (let attempt = 0; attempt < MAX_REBUILD_ATTEMPTS; attempt++) {
    await api.removeAll();
    // 同步依次发出、一起等回调：不为每项多等一次往返。
    const results = await Promise.all(
      items.map((item) => new Promise<string | null>((resolve) => api.create(item, resolve))),
    );
    errors = results.filter((error): error is string => error !== null);
    if (errors.length === 0) break;
  }
  return errors;
}
