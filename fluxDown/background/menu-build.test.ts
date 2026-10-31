import { describe, expect, test } from "bun:test";
import { MAX_REBUILD_ATTEMPTS, rebuildMenus, type MenuApi, type MenuItemSpec } from "./menu-build";

/**
 * 模拟 Chrome 的 MenuManager：按调用顺序处理、重复 id 报错、子项要求父项已存在。
 * `restoreOnce`：浏览器启动时异步恢复上次持久化的菜单，落在第一次 removeAll 之后、create 之前。
 */
function fakeMenus(options: { restoreOnce?: MenuItemSpec[]; failIds?: string[] } = {}) {
  const items = new Map<string, MenuItemSpec>();
  let pendingRestore = options.restoreOnce;
  let removeAllCalls = 0;
  const api: MenuApi = {
    async removeAll() {
      removeAllCalls++;
      items.clear();
      for (const item of pendingRestore ?? []) items.set(item.id, item);
      pendingRestore = undefined;
    },
    create(item, done) {
      queueMicrotask(() => {
        if (options.failIds?.includes(item.id)) return done(`Invalid item ${item.id}`);
        if (items.has(item.id)) return done(`Cannot create item with duplicate id ${item.id}`);
        if (item.parentId !== undefined && !items.has(item.parentId)) {
          return done(`Cannot find menu item with id ${item.parentId}`);
        }
        items.set(item.id, item);
        done(null);
      });
    },
  };
  return { api, ids: () => [...items.keys()], removeAllCalls: () => removeAllCalls };
}

const item = (id: string, parentId?: string): MenuItemSpec => ({ id, parentId, title: id, contexts: ["link"] });

describe("rebuildMenus", () => {
  test("persisted menus restored between removeAll and create converge to the current list", async () => {
    // 上次会话开着远程通道（父项 + 本机 / 远程子项），这次只要父项。
    const menus = fakeMenus({
      restoreOnce: [item("send-link"), item("send-link-local", "send-link"), item("send-link-remote", "send-link"), item("send-page")],
    });

    const errors = await rebuildMenus(menus.api, [item("send-link"), item("send-page")]);

    expect(errors).toEqual([]);
    expect(menus.ids()).toEqual(["send-link", "send-page"]);
  });

  test("a failure that persists is reported after a bounded number of rebuilds", async () => {
    const menus = fakeMenus({ failIds: ["send-page"] });

    const errors = await rebuildMenus(menus.api, [item("send-link"), item("send-page")]);

    expect(errors).toEqual(["Invalid item send-page"]);
    expect(menus.removeAllCalls()).toBe(MAX_REBUILD_ATTEMPTS);
    expect(menus.ids()).toEqual(["send-link"]);
  });
});
