import { describe, expect, mock, test } from "bun:test";

mock.module("wxt/browser", () => ({ browser: {} }));
// 必须先 mock 再加载被测模块（与 probe.test.ts 同一约定），静态 import 会被提升到 mock 之前，故此处动态导入。
const { ALL_URLS_PERMISSION, hasHostAccess, requestHostAccess } = await import("./host-access");

function fakeApi(contains: () => Promise<boolean>, request: () => Promise<boolean> = async () => true) {
  const calls: { contains: unknown[]; request: unknown[] } = { contains: [], request: [] };
  return {
    calls,
    api: {
      contains: (p: unknown) => {
        calls.contains.push(p);
        return contains();
      },
      request: (p: unknown) => {
        calls.request.push(p);
        return request();
      },
    },
  };
}

describe("全站访问权限检测", () => {
  test("不适用的构建（Chrome）：视为已授权且不调用 API", async () => {
    const { api, calls } = fakeApi(async () => false);
    expect(await hasHostAccess(api, false)).toBe(true);
    expect(calls.contains).toHaveLength(0);
  });

  test("Firefox：按 <all_urls> 检查，被撤销时返回 false", async () => {
    const { api, calls } = fakeApi(async () => false);
    expect(await hasHostAccess(api, true)).toBe(false);
    expect(calls.contains).toEqual([ALL_URLS_PERMISSION]);
    expect(ALL_URLS_PERMISSION).toEqual({ origins: ["<all_urls>"] });
  });

  test("Firefox：已授权返回 true", async () => {
    const { api } = fakeApi(async () => true);
    expect(await hasHostAccess(api, true)).toBe(true);
  });

  test("API 调用失败按已授权处理，避免误报", async () => {
    const { api } = fakeApi(async () => {
      throw new Error("boom");
    });
    expect(await hasHostAccess(api, true)).toBe(true);
  });
});

describe("全站访问权限申请", () => {
  test("透传 <all_urls> 并返回用户决定", async () => {
    const granted = fakeApi(async () => false, async () => true);
    expect(await requestHostAccess(granted.api)).toBe(true);
    expect(granted.calls.request).toEqual([ALL_URLS_PERMISSION]);

    const refused = fakeApi(async () => false, async () => false);
    expect(await requestHostAccess(refused.api)).toBe(false);
  });

  test("申请被浏览器拒绝（reject）时返回 false", async () => {
    const { api } = fakeApi(
      async () => false,
      async () => {
        throw new Error("may only be called from a user input handler");
      },
    );
    expect(await requestHostAccess(api)).toBe(false);
  });
});
