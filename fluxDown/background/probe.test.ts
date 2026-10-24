import { beforeEach, describe, expect, mock, test } from "bun:test";
import { compileBundled } from "@/rules/engine";

interface FakeTab {
  url: string;
  status: "loading" | "complete";
  discarded?: boolean;
}

const tabs = new Map<number, FakeTab>();
/** 页面里存活的 sensor：有则应答 probe。 */
const liveSensors = new Map<number, string>();
const injected: {
  tabId: number;
  files: string[];
  world?: string;
  allFrames?: boolean;
  frameIds?: number[];
  injectImmediately?: boolean;
}[] = [];
let injectFails = false;
/** 补注入 sensor 后它会拉配置登记会话：由测试在 tab-state 加载后接上。 */
let onSensorInjected: (tabId: number) => void = () => {};
const updatedListeners: ((tabId: number, info: { status?: string }) => void)[] = [];

mock.module("wxt/browser", () => ({
  browser: {
    storage: {},
    runtime: { onInstalled: { addListener: () => {} } },
    tabs: {
      get: async (tabId: number) => {
        const tab = tabs.get(tabId);
        if (!tab) throw new Error("No tab");
        return { id: tabId, ...tab };
      },
      sendMessage: async (tabId: number) => {
        const sid = liveSensors.get(tabId);
        if (!sid) throw new Error("Could not establish connection. Receiving end does not exist.");
        return { sid, hooks: { protocol: 1, hid: "h", late: false } };
      },
      query: async () => [],
      onActivated: { addListener: () => {} },
      onUpdated: { addListener: (l: (tabId: number, info: { status?: string }) => void) => updatedListeners.push(l) },
      onRemoved: { addListener: () => {} },
      onReplaced: { addListener: () => {} },
    },
    scripting: {
      executeScript: async (opts: {
        target: { tabId: number; allFrames?: boolean; frameIds?: number[] };
        files: string[];
        world?: string;
        injectImmediately?: boolean;
      }) => {
        injected.push({
          tabId: opts.target.tabId,
          files: opts.files,
          world: opts.world,
          allFrames: opts.target.allFrames,
          frameIds: opts.target.frameIds,
          injectImmediately: opts.injectImmediately,
        });
        if (injectFails) throw new Error("Cannot access contents of the page");
        if (opts.files[0]?.endsWith("/sensor.js")) queueMicrotask(() => onSensorInjected(opts.target.tabId));
        return [];
      },
    },
  },
}));
const engine = compileBundled();
mock.module("./rules-store", () => ({ getEngine: () => engine }));
// 必须先 mock 再加载被测模块（与 settings.test.ts 同一约定），故此处动态导入。
const { installProbe, probeStateOf, probeTab } = await import("./probe");
const { getSession, registerSession } = await import("./tab-state");

installProbe();
onSensorInjected = (tabId) => registerSession(tabId, 0, "reinjected");

let nextTab = 100;

beforeEach(() => {
  injected.length = 0;
  injectFails = false;
});

describe("页面探测恢复", () => {
  test("sensor 活着但登记丢失（SW 重启）：探测应答后补登记，不注入", async () => {
    const tab = nextTab++;
    tabs.set(tab, { url: "https://www.douyin.com/jingxuan", status: "complete" });
    liveSensors.set(tab, "alive");
    await probeTab(tab);
    expect(getSession(tab, 0)?.sid).toBe("alive");
    expect(injected).toHaveLength(0);
    expect(probeStateOf(tab)).toBe("live");
  });

  test("页面里没有 sensor（扩展更新后的旧页面）：先注入 MAIN 钩子再注入 sensor，登记后恢复为 live", async () => {
    const tab = nextTab++;
    tabs.set(tab, { url: "https://www.douyin.com/jingxuan", status: "complete" });
    await probeTab(tab);
    expect(injected.map((i) => [i.files[0], i.world])).toEqual([
      ["/content-scripts/page-hooks.js", "MAIN"],
      ["/content-scripts/sensor.js", undefined],
      ["/content-scripts/panel.js", undefined],
    ]);
    expect(getSession(tab, 0)?.sid).toBe("reinjected");
    // 钩子与 sensor 覆盖所有帧并立即注入；面板只进顶层帧
    expect(injected.map((i) => [i.allFrames, i.frameIds, i.injectImmediately])).toEqual([
      [true, undefined, true],
      [true, undefined, true],
      [undefined, [0], undefined],
    ]);
    expect(probeStateOf(tab)).toBe("live");
  });

  test("注入被拒：记为 unavailable，同一文档不再重试；开始加载新文档后重新检查", async () => {
    const tab = nextTab++;
    tabs.set(tab, { url: "https://chromewebstore.google.com/detail/x", status: "complete" });
    injectFails = true;
    await probeTab(tab);
    expect(probeStateOf(tab)).toBe("unavailable");
    await probeTab(tab);
    expect(injected).toHaveLength(1);

    for (const listener of updatedListeners) listener(tab, { status: "loading" });
    expect(probeStateOf(tab)).toBe("unknown");
    injectFails = false;
    await probeTab(tab);
    expect(probeStateOf(tab)).toBe("live");
  });

  test("加载中的页面与非 http(s) 页面不探测（交给声明式内容脚本 / 本就不能注入）", async () => {
    const loading = nextTab++;
    tabs.set(loading, { url: "https://example.com/", status: "loading" });
    const internal = nextTab++;
    tabs.set(internal, { url: "chrome://settings/", status: "complete" });
    await probeTab(loading);
    await probeTab(internal);
    expect(injected).toHaveLength(0);
    expect(probeStateOf(loading)).toBe("unknown");
    expect(probeStateOf(internal)).toBe("unknown");
  });
});
