import { describe, expect, test } from "bun:test";
import { nextCaptureActions, PENDING_MAX_AGE_MS, reconcilePending } from "./capture-logic";
import type { CaptureSettings, PendingCapture } from "./capture-logic";

const NOW = 1_000_000_000_000;
const DAY = 24 * 60 * 60 * 1000;

const settings: CaptureSettings = {
  fallbackOnReject: true,
  fallbackOnCancel: false,
  askExcludeAfterCancels: true,
};

function capture(id: string, overrides: Partial<PendingCapture> = {}): PendingCapture {
  return { id, url: `https://example.com/${id}.zip`, host: "example.com", createdAt: NOW - 1000, ...overrides };
}

describe("nextCaptureActions", () => {
  test("rejected + fallbackOnCancel：记录被拒并回退", () => {
    const c = capture("a");
    const { actions, keep } = nextCaptureActions([c], { a: "rejected" }, { ...settings, fallbackOnCancel: true }, {}, NOW);
    expect(actions).toEqual([
      { kind: "recordReject", host: "example.com" },
      { kind: "fallback", capture: c },
    ]);
    expect(keep).toEqual([]);
  });

  test("rejected 且 fallbackOnCancel 关闭：只记录，不回退", () => {
    const { actions } = nextCaptureActions([capture("a")], { a: "rejected" }, settings, {}, NOW);
    expect(actions).toEqual([{ kind: "recordReject", host: "example.com" }]);
  });

  test("第 3 次被拒触发询问；前两次不触发", () => {
    const stats = { "example.com": [NOW - 2 * DAY, NOW - DAY] };
    const third = nextCaptureActions([capture("a")], { a: "rejected" }, settings, stats, NOW);
    expect(third.actions).toContainEqual({ kind: "askExclude", host: "example.com" });

    const second = nextCaptureActions([capture("a")], { a: "rejected" }, settings, { "example.com": [NOW - DAY] }, NOW);
    expect(second.actions.some((a) => a.kind === "askExclude")).toBe(false);
  });

  test("7 天前的被拒不计数；已有站点配置或关闭询问时不询问", () => {
    const old = { "example.com": [NOW - 8 * DAY, NOW - 9 * DAY] };
    expect(
      nextCaptureActions([capture("a")], { a: "rejected" }, settings, old, NOW).actions.some((a) => a.kind === "askExclude"),
    ).toBe(false);

    const recent = { "example.com": [NOW - DAY, NOW - 2 * DAY] };
    expect(
      nextCaptureActions([capture("a")], { a: "rejected" }, settings, recent, NOW, new Set(["example.com"])).actions.some(
        (a) => a.kind === "askExclude",
      ),
    ).toBe(false);
    expect(
      nextCaptureActions([capture("a")], { a: "rejected" }, { ...settings, askExcludeAfterCancels: false }, recent, NOW).actions.some(
        (a) => a.kind === "askExclude",
      ),
    ).toBe(false);
  });

  test("failed 按 fallbackOnReject 回退；accepted / unknown 直接移除", () => {
    const f = capture("f");
    expect(nextCaptureActions([f], { f: "failed" }, settings, {}, NOW).actions).toEqual([{ kind: "fallback", capture: f }]);
    expect(nextCaptureActions([f], { f: "failed" }, { ...settings, fallbackOnReject: false }, {}, NOW).actions).toEqual([]);
    const done = nextCaptureActions([capture("a"), capture("u")], { a: "accepted", u: "unknown" }, settings, {}, NOW);
    expect(done.actions).toEqual([]);
    expect(done.keep).toEqual([]);
  });

  test("pending 与无状态保留；超时丢弃", () => {
    const p = capture("p");
    const stale = capture("s", { createdAt: NOW - PENDING_MAX_AGE_MS - 1 });
    const { actions, keep } = nextCaptureActions([p, stale], { p: "pending", s: "pending" }, settings, {}, NOW);
    expect(actions).toEqual([]);
    expect(keep).toEqual([p]);
    expect(nextCaptureActions([p], {}, settings, {}, NOW).keep).toEqual([p]);
  });
});

describe("reconcilePending", () => {
  test("轮询往返期间新登记的事务不被快照结果覆盖", () => {
    const a = capture("a");
    const b = capture("b"); // 在 captureStatus 的 await 期间由 trackCaptures 追加
    const snapshotIds = new Set(["a"]);
    const { keep } = nextCaptureActions([a], { a: "pending" }, settings, {}, NOW);
    // 修复前：`pending = keep` 会把 b 丢掉。
    expect(reconcilePending([a, b], snapshotIds, keep)).toEqual([a, b]);
  });

  test("快照内已终结的事务被移除，新事务保留", () => {
    const a = capture("a");
    const b = capture("b");
    const { keep } = nextCaptureActions([a], { a: "accepted" }, settings, {}, NOW);
    expect(reconcilePending([a, b], new Set(["a"]), keep)).toEqual([b]);
  });
});
