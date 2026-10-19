import { describe, expect, mock, test } from "bun:test";
import { compileBundled } from "@/rules/engine";
import type { ElementState } from "./protocol";

// storage.session 缺省：不持久化；tab-state 只在 installTabState 里注册浏览器监听（本测试不调用）。
mock.module("wxt/browser", () => ({ browser: { storage: {}, tabs: {} } }));
const engine = compileBundled();
mock.module("./rules-store", () => ({ getEngine: () => engine }));
// 必须先 mock 再加载被测模块（与 settings.test.ts 同一约定），故此处动态导入。
const { getElements, getRetiredElements, registerSession, setSessionHooks, unseenEvidence, updateElements } =
  await import("./tab-state");

let nextTab = 1;

function el(key: string, overrides: Partial<ElementState> = {}): Omit<ElementState, "frameId"> {
  return {
    key,
    tag: "video",
    playing: false,
    visibleRatio: 1,
    rect: { w: 800, h: 450 },
    videoWidth: 0,
    videoHeight: 0,
    currentSrc: "",
    lastPlayedAt: 0,
    lastInteractionAt: 0,
    bindings: [],
    title: "",
    encrypted: false,
    tracks: [],
    ...overrides,
  };
}

const mse = (msId: number, url: string, hid = "h1") => ({
  msId,
  hid,
  bindings: [{ sbId: msId, mime: "video/mp4", kind: "video" as const, exact: [url], temporal: [] }],
});

describe("元素播放会话归档", () => {
  test("复用元素换了 MediaSource：上一会话归档，标题取它播放时的标题而不是换片后的 DOM 文案", () => {
    const tab = nextTab++;
    registerSession(tab, 0, "s");
    updateElements(tab, 0, "s", [el("s:1", { ...mse(1, "https://cdn/a"), playing: true, title: "第7集" })], [], false);
    // 换片前 DOM 已经切到下一条的描述，随后才换上新的 MediaSource
    updateElements(tab, 0, "s", [el("s:1", { ...mse(1, "https://cdn/a"), title: "第8集" })], [], false);
    updateElements(tab, 0, "s", [el("s:1", { ...mse(2, "https://cdn/b"), playing: true, title: "第8集" })], [], false);
    const retired = getRetiredElements(tab);
    expect(retired.map((r) => [r.title, r.bindings[0].exact[0], r.playing])).toEqual([["第7集", "https://cdn/a", false]]);
    expect(retired[0].key).not.toBe("s:1");
    expect(getElements(tab).map((e) => e.msId)).toEqual([2]);
  });

  test("会话又回到元素上（重注入后重新上报同一会话）：不保留重复的历史项", () => {
    const tab = nextTab++;
    registerSession(tab, 0, "old");
    updateElements(tab, 0, "old", [el("old:1", mse(1, "https://cdn/a"))], [], false);
    registerSession(tab, 0, "new");
    expect(getRetiredElements(tab)).toHaveLength(1);
    updateElements(tab, 0, "new", [el("new:1", mse(1, "https://cdn/a"))], [], true);
    expect(getRetiredElements(tab)).toHaveLength(0);
    expect(getElements(tab).map((e) => e.key)).toEqual(["new:1"]);
  });

  test("没有来源证据的元素被移除时不归档", () => {
    const tab = nextTab++;
    updateElements(tab, 0, "s", [el("s:1")], [], false);
    updateElements(tab, 0, "s", [], ["s:1"], false);
    expect(getRetiredElements(tab)).toHaveLength(0);
    expect(getElements(tab)).toHaveLength(0);
  });

  test("帧换了文档（钩子 hid 变化）：上一文档的 MSE 历史会话丢弃；hid 不变（仅重注入）则保留", () => {
    const tab = nextTab++;
    registerSession(tab, 0, "a");
    updateElements(tab, 0, "a", [el("a:1", mse(1, "https://cdn/a", "h1"))], [], false);
    registerSession(tab, 0, "b");
    setSessionHooks(tab, 0, "b", { protocol: 1, hid: "h1", late: false });
    expect(getRetiredElements(tab)).toHaveLength(1);
    registerSession(tab, 0, "c");
    setSessionHooks(tab, 0, "c", { protocol: 1, hid: "h2", late: false });
    expect(getRetiredElements(tab)).toHaveLength(0);
  });
});

describe("sensor 会话登记", () => {
  test("不认识的 sid 发来增量：要求全量重发；全量本身不再要求", () => {
    const tab = nextTab++;
    expect(updateElements(tab, 0, "s", [el("s:1")], [], false)).toBe(true);
    expect(updateElements(tab, 0, "s", [el("s:1")], [], false)).toBe(false);
    const other = nextTab++;
    expect(updateElements(other, 0, "s", [el("s:1")], [], true)).toBe(false);
  });

  test("同帧出现新 sid：旧实例的元素被移除（它们不会再更新），其它帧不受影响", () => {
    const tab = nextTab++;
    updateElements(tab, 0, "old", [el("old:1"), el("old:2")], [], false);
    updateElements(tab, 3, "sub", [el("sub:1")], [], false);
    registerSession(tab, 0, "new");
    expect(getElements(tab).map((e) => e.key)).toEqual(["sub:1"]);
  });

  test("全量快照删掉本 sid 中缺席的元素，其它帧不受影响", () => {
    const tab = nextTab++;
    updateElements(tab, 0, "s", [el("s:1"), el("s:2")], [], false);
    updateElements(tab, 2, "t", [el("t:1")], [], false);
    updateElements(tab, 0, "s", [el("s:2")], [], true);
    expect(getElements(tab).map((e) => e.key).sort()).toEqual(["s:2", "t:1"]);
  });
});

describe("按元素证据补报资源", () => {
  test("只补资源表里没有的 http(s) 地址，按 SourceBuffer 的 MIME 给类型线索；同一地址只检查一次", () => {
    const tab = nextTab++;
    const elements = [
      el("s:1", {
        bindings: [
          { sbId: 1, mime: 'audio/mp4; codecs="mp4a.40.2"', kind: "audio", exact: ["https://cdn/x/a.mp4", "blob:https://p/1"], temporal: [] },
        ],
      }),
    ];
    updateElements(tab, 0, "s", elements, [], false);
    const missing = unseenEvidence(tab, elements);
    expect(missing.map((m) => [m.url, m.mimeType, m.detectedBy])).toEqual([["https://cdn/x/a.mp4", "audio/mp4", "fetch-intercept"]]);
    expect(unseenEvidence(tab, elements)).toEqual([]);
  });
});
