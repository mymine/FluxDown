import { afterEach, describe, expect, test, vi } from "bun:test";
import { LinkCaptureRegistry } from "./link-capture";
import type { LinkCaptureDeps } from "./link-capture";

const URL_A = "https://github.com/cli/cli/releases/download/v2.62.0/gh.zip";

function harness() {
  const calls: string[] = [];
  let resolveDownload: (id: number) => void = () => {};
  let rejectDownload: (error: Error) => void = () => {};
  const deps: LinkCaptureDeps = {
    download: (url) => {
      calls.push(`download ${url}`);
      const { promise, resolve, reject } = Promise.withResolvers<number>();
      resolveDownload = resolve;
      rejectDownload = reject;
      return promise;
    },
    cancel: async (id) => {
      calls.push(`cancel ${id}`);
    },
    erase: async (id) => {
      calls.push(`erase ${id}`);
    },
  };
  const registry = new LinkCaptureRegistry(deps, { startTimeoutMs: 1_000, abandonedTtlMs: 5_000 });
  return {
    registry,
    calls,
    started: (id: number) => resolveDownload(id),
    failed: (error: Error) => rejectDownload(error),
  };
}

/** 让 download() 的 then 回调与 cancel → erase 链跑完（只排空微任务，不依赖真实时间）。 */
async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

afterEach(() => {
  vi.useRealTimers();
});

describe("LinkCaptureRegistry", () => {
  test("claim by the primary interceptor hands the send to it", async () => {
    const h = harness();
    const outcome = h.registry.capture({ url: URL_A, referrer: "https://github.com/cli/cli/releases" });
    h.started(7);
    await flush();
    expect(h.registry.owns({ id: 7, url: "https://other.example/" })).toBe(true);
    expect(h.registry.claim({ id: 7, url: URL_A }, true)).toEqual({
      kind: "claimed",
      request: { url: URL_A, referrer: "https://github.com/cli/cli/releases" },
    });
    expect(await outcome).toBe(true);
    // 认领后不再归右键事务所有：后续同 id 事件交回常规拦截。
    expect(h.registry.owns({ id: 7, url: URL_A })).toBe(false);
  });

  test("onDeterminingFilename may arrive before download() resolves: matched by URL", async () => {
    const h = harness();
    const outcome = h.registry.capture({ url: "https://Example.com/a b.zip", referrer: "" });
    // 浏览器下载项里的 url 是规范化后的形式。
    expect(h.registry.claim({ id: 3, url: "https://example.com/a%20b.zip" }, true)?.kind).toBe("claimed");
    expect(await outcome).toBe(true);
    h.started(3);
    await flush();
    expect(h.registry.owns({ id: 3, url: "https://example.com/a%20b.zip" })).toBe(false);
  });

  test("other downloads of the same URL are not claimed once the id is known", async () => {
    const h = harness();
    void h.registry.capture({ url: URL_A, referrer: "" });
    h.started(1);
    await flush();
    expect(h.registry.claim({ id: 2, url: URL_A }, true)).toBeNull();
    expect(h.registry.owns({ id: 2, url: URL_A })).toBe(false);
  });

  test("a page response is declined and the caller sends the link directly", async () => {
    const h = harness();
    const outcome = h.registry.capture({ url: URL_A, referrer: "" });
    h.started(4);
    await flush();
    expect(h.registry.claim({ id: 4, url: URL_A }, false)).toEqual({ kind: "declined" });
    expect(await outcome).toBe(false);
  });

  test("download() rejection falls back to a direct send", async () => {
    const h = harness();
    const outcome = h.registry.capture({ url: URL_A, referrer: "" });
    h.failed(new Error("Invalid URL"));
    expect(await outcome).toBe(false);
    expect(h.registry.owns({ id: 1, url: URL_A })).toBe(false);
  });

  test("interruption before the claim falls back and erases the browser record", async () => {
    const h = harness();
    const outcome = h.registry.capture({ url: URL_A, referrer: "" });
    h.started(5);
    await flush();
    h.registry.interrupted({ id: 5, url: URL_A });
    expect(await outcome).toBe(false);
    expect(h.calls).toContain("erase 5");
    expect(h.registry.claim({ id: 5, url: URL_A }, true)).toBeNull();
  });

  test("timeout falls back, cancels the started download, and a late claim only cancels", async () => {
    vi.useFakeTimers();
    const h = harness();
    const outcome = h.registry.capture({ url: URL_A, referrer: "" });
    h.started(6);
    await flush();
    vi.advanceTimersByTime(1_000);
    expect(await outcome).toBe(false);
    await flush();
    expect(h.calls).toEqual([`download ${URL_A}`, "cancel 6", "erase 6"]);
    // 保留期内迟到的浏览器下载事件仍归右键事务：兜底层让路，主拦截只取消。
    expect(h.registry.owns({ id: 6, url: URL_A })).toBe(true);
    expect(h.registry.claim({ id: 6, url: URL_A }, true)).toEqual({ kind: "abandoned" });
  });

  test("abandoned transactions expire after the retention period", async () => {
    vi.useFakeTimers();
    const h = harness();
    const outcome = h.registry.capture({ url: URL_A, referrer: "" });
    vi.advanceTimersByTime(1_000);
    expect(await outcome).toBe(false);
    expect(h.registry.owns({ id: 9, url: URL_A })).toBe(true);
    vi.advanceTimersByTime(5_000);
    expect(h.registry.owns({ id: 9, url: URL_A })).toBe(false);
  });

  test("timeout before download() resolves cancels the download once its id arrives", async () => {
    vi.useFakeTimers();
    const h = harness();
    const outcome = h.registry.capture({ url: URL_A, referrer: "" });
    vi.advanceTimersByTime(1_000);
    expect(await outcome).toBe(false);
    h.started(8);
    await flush();
    expect(h.calls).toEqual([`download ${URL_A}`, "cancel 8", "erase 8"]);
  });

  test("a second capture of a pending URL sends directly instead of starting another browser download", async () => {
    const h = harness();
    void h.registry.capture({ url: URL_A, referrer: "" });
    expect(await h.registry.capture({ url: URL_A, referrer: "" })).toBe(false);
    expect(h.calls).toEqual([`download ${URL_A}`]);
  });
});
