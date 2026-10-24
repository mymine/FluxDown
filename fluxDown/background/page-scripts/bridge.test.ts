import { describe, expect, test } from "bun:test";
import { createStorageArea } from "@/utils/storage-mock.test-util";
import { createBridge, MAX_INVOKES_IN_FLIGHT, RATE_LIMIT_ALL, RATE_LIMIT_DOWNLOAD, RATE_WINDOW_MS, validateDownload, validateMediaItems } from "./bridge";
import type { AddMediaCall, BridgeDeps, BridgeSender, DownloadCall, InvokeOutcome } from "./bridge";
import { createPageScriptStore } from "./store";
import { MAX_MESSAGE_BYTES, MAX_STORAGE_BYTES } from "./wire";
import type { BridgeResponse } from "./wire";

const TOKEN = "t".repeat(40);
const PAGE = "https://www.bilibili.com/video/BV1";
const SENDER: BridgeSender = {
  id: "ext-id",
  url: PAGE,
  frameId: 0,
  tab: { id: 7, url: PAGE },
};

interface Harness {
  call(message: unknown, sender?: BridgeSender): Promise<BridgeResponse>;
  downloads: DownloadCall[];
  media: AddMediaCall[];
  invokes: Array<{ identity: string; name: string; args: unknown; pageUrl: string }>;
  clock: { now: number };
  invokeGate: { release: (() => void) | null; pending: Promise<InvokeOutcome> | null };
}

function harness(approved: Record<string, string> = { [TOKEN]: "fluxdown@bili" }): Harness {
  const store = createPageScriptStore(createStorageArea());
  const downloads: DownloadCall[] = [];
  const media: AddMediaCall[] = [];
  const invokes: Harness["invokes"] = [];
  const clock = { now: 1_000 };
  const invokeGate: Harness["invokeGate"] = { release: null, pending: null };
  const deps: BridgeDeps = {
    identityForToken: async (token) => approved[token] ?? null,
    now: () => clock.now,
    extensionId: "ext-id",
    download: async (call) => {
      downloads.push(call);
      return { ok: true };
    },
    addMedia: async (call) => {
      media.push(call);
      return call.items.length;
    },
    invoke: async (identity, name, args, pageUrl) => {
      invokes.push({ identity, name, args, pageUrl });
      return invokeGate.pending ?? { ok: true, value: { echo: args } };
    },
    storage: {
      get: (identity, key) => store.storageGet(identity, key),
      set: (identity, key, value) => store.storageSet(identity, key, value),
      remove: (identity, key) => store.storageRemove(identity, key),
    },
  };
  const bridge = createBridge(deps);
  return { call: (message, sender = SENDER) => bridge(message, sender), downloads, media, invokes, clock, invokeGate };
}

const msg = (op: string, rest: Record<string, unknown> = {}) => ({ v: 1, token: TOKEN, op, ...rest });

describe("authentication and envelope", () => {
  test("unknown token and unapproved plugins are rejected without side effects", async () => {
    const h = harness();
    const bad = await h.call({ ...msg("download", { request: { url: "https://x.test/a.zip" } }), token: "nope" });
    expect(bad).toEqual({ ok: false, error: "page scripts are not enabled for this plugin" });
    expect(h.downloads).toHaveLength(0);
  });

  test("revoked approval (token no longer known) is rejected", async () => {
    const h = harness({});
    const res = await h.call(msg("storage.get", { key: "k" }));
    expect(res.ok).toBe(false);
  });

  test("malformed envelopes", async () => {
    const h = harness();
    for (const bad of [null, 1, "x", [], { v: 2, token: TOKEN, op: "download" }, { v: 1, op: "download" }, { v: 1, token: TOKEN }]) {
      expect((await h.call(bad)).ok).toBe(false);
    }
    expect(await h.call(msg("eval", { code: "1" }))).toEqual({ ok: false, error: "unknown operation" });
  });

  test("messages over 256 KiB are rejected", async () => {
    const h = harness();
    const res = await h.call(msg("storage.set", { key: "k", value: "x".repeat(MAX_MESSAGE_BYTES) }));
    expect(res).toEqual({ ok: false, error: "message too large" });
  });

  test("sender must be this extension and an http(s) page; page url comes from the sender", async () => {
    const h = harness();
    expect((await h.call(msg("storage.get", { key: "k" }), { ...SENDER, id: "other" })).ok).toBe(false);
    expect((await h.call(msg("storage.get", { key: "k" }), { ...SENDER, url: "chrome://settings" })).ok).toBe(false);
    expect((await h.call(msg("storage.get", { key: "k" }), { ...SENDER, url: undefined })).ok).toBe(false);

    await h.call(msg("download", { request: { url: "https://cdn.test/v.mp4", pageUrl: "https://evil.test/" } }));
    expect(h.downloads[0].referrer).toBe(PAGE);
  });
});

describe("download", () => {
  test("accepts http(s), defaults referrer to the sender page, lowercases allowlisted headers", async () => {
    const h = harness();
    const res = await h.call(
      msg("download", {
        request: {
          url: "https://cdn.test/v.mp4",
          filename: "v.mp4",
          headers: { Referer: "https://www.bilibili.com/", "User-Agent": "UA", Accept: "*/*" },
        },
      }),
    );
    expect(res).toEqual({ ok: true, value: { ok: true } });
    expect(h.downloads[0]).toMatchObject({
      identity: "fluxdown@bili",
      url: "https://cdn.test/v.mp4",
      filename: "v.mp4",
      referrer: PAGE,
      headers: { referer: "https://www.bilibili.com/", "user-agent": "UA", accept: "*/*" },
      tabId: 7,
    });
  });

  test("explicit referrer must itself be http(s)", async () => {
    const h = harness();
    await h.call(msg("download", { request: { url: "https://a.test/f", referrer: "https://r.test/" } }));
    expect(h.downloads[0].referrer).toBe("https://r.test/");
    expect((await h.call(msg("download", { request: { url: "https://a.test/f", referrer: "javascript:1" } }))).ok).toBe(false);
  });

  test("illegal urls are rejected", () => {
    for (const url of ["file:///etc/passwd", "javascript:alert(1)", "data:text/plain,hi", "ftp://x/y", "magnet:?xt=1", "", 5, undefined, "https://" + "a".repeat(9000)]) {
      expect(validateDownload({ url }, PAGE).ok).toBe(false);
    }
  });

  test("headers outside the allowlist or with control characters are rejected", () => {
    expect(validateDownload({ url: "https://a.test/f", headers: { Cookie: "a=b" } }, PAGE)).toEqual({
      ok: false,
      error: "header not allowed: Cookie",
    });
    expect(validateDownload({ url: "https://a.test/f", headers: { Authorization: "x" } }, PAGE).ok).toBe(false);
    expect(validateDownload({ url: "https://a.test/f", headers: { accept: "a\r\nX: 1" } }, PAGE).ok).toBe(false);
    expect(validateDownload({ url: "https://a.test/f", headers: { accept: 1 } }, PAGE).ok).toBe(false);
    expect(validateDownload({ url: "https://a.test/f", headers: [] }, PAGE).ok).toBe(false);
    expect(validateDownload({ url: "https://a.test/f", filename: "a\u0000b" }, PAGE).ok).toBe(false);
    expect(validateDownload("https://a.test/f", PAGE).ok).toBe(false);
  });

  test("download is rate limited per plugin, separately from other operations", async () => {
    const h = harness();
    const req = { url: "https://a.test/f" };
    for (let i = 0; i < RATE_LIMIT_DOWNLOAD; i++) expect((await h.call(msg("download", { request: req }))).ok).toBe(true);
    expect(await h.call(msg("download", { request: req }))).toEqual({ ok: false, error: "rate limited" });
    expect((await h.call(msg("storage.get", { key: "k" }))).ok).toBe(true);
    h.clock.now += RATE_WINDOW_MS;
    expect((await h.call(msg("download", { request: req }))).ok).toBe(true);
  });

  test("overall per-plugin limit applies to every operation and is per plugin", async () => {
    const other = "o".repeat(40);
    const h = harness({ [TOKEN]: "a@x", [other]: "b@x" });
    for (let i = 0; i < RATE_LIMIT_ALL; i++) await h.call(msg("storage.get", { key: "k" }));
    expect(await h.call(msg("storage.get", { key: "k" }))).toEqual({ ok: false, error: "rate limited" });
    expect((await h.call({ ...msg("storage.get", { key: "k" }), token: other })).ok).toBe(true);
  });
});

describe("addMedia", () => {
  test("rejects non-array and more than 100 items; skips invalid entries; needs a tab", async () => {
    const h = harness();
    expect((await h.call(msg("addMedia", { items: "x" }))).ok).toBe(false);
    expect(await h.call(msg("addMedia", { items: Array.from({ length: 101 }, () => ({ url: "https://a.test/x" })) }))).toEqual({
      ok: false,
      error: "at most 100 items per call",
    });
    const res = await h.call(
      msg("addMedia", {
        items: [
          { url: "https://a.test/v.mp4", title: "T", mime: "video/mp4" },
          { url: "javascript:alert(1)" },
          { url: "https://a.test/b.mp4", mime: "not a mime" },
          null,
        ],
      }),
    );
    expect(res).toEqual({ ok: true, value: 2 });
    expect(h.media[0]).toMatchObject({ identity: "fluxdown@bili", tabId: 7, frameId: 0, pageUrl: PAGE });
    expect(h.media[0].items).toEqual([
      { url: "https://a.test/v.mp4", title: "T", mime: "video/mp4" },
      { url: "https://a.test/b.mp4" },
    ]);
    expect((await h.call(msg("addMedia", { items: [{ url: "https://a.test/x" }] }), { ...SENDER, tab: undefined })).ok).toBe(false);
  });

  test("validateMediaItems clips titles", () => {
    const res = validateMediaItems([{ url: "https://a.test/x", title: "t".repeat(1000) }]);
    expect(res.ok && res.value[0].title?.length).toBe(256);
  });
});

describe("invoke", () => {
  test("passes the plugin identity from the token and the sender page url (never script-reported)", async () => {
    const h = harness();
    const res = await h.call(msg("invoke", { name: "resolve", args: { id: 1 }, pageUrl: "https://evil.test/", identity: "x@y" }));
    expect(res).toEqual({ ok: true, value: { echo: { id: 1 } } });
    expect(h.invokes[0]).toEqual({ identity: "fluxdown@bili", name: "resolve", args: { id: 1 }, pageUrl: PAGE });
  });

  test("invalid handler names are rejected", async () => {
    const h = harness();
    for (const name of ["", 5, "a\nb", "x".repeat(200)]) expect((await h.call(msg("invoke", { name }))).ok).toBe(false);
  });

  test("concurrent invocations are capped per plugin", async () => {
    const h = harness();
    const gate = Promise.withResolvers<InvokeOutcome>();
    h.invokeGate.pending = gate.promise;
    const running = Array.from({ length: MAX_INVOKES_IN_FLIGHT }, () => h.call(msg("invoke", { name: "slow" })));
    // 等所有调用都进入 invoke（只推进微任务，不依赖真实时间）。
    for (let i = 0; i < 100 && h.invokes.length < MAX_INVOKES_IN_FLIGHT; i++) await Promise.resolve();
    expect(h.invokes).toHaveLength(MAX_INVOKES_IN_FLIGHT);
    expect(await h.call(msg("invoke", { name: "slow" }))).toEqual({ ok: false, error: "too many concurrent invocations" });
    gate.resolve({ ok: true, value: 1 });
    expect((await Promise.all(running)).every((r) => r.ok)).toBe(true);
    h.invokeGate.pending = null;
    expect((await h.call(msg("invoke", { name: "slow" }))).ok).toBe(true);
  });

  test("handler errors become readable rejections", async () => {
    const h = harness();
    h.invokeGate.pending = Promise.resolve({ ok: false, message: "rpc not exposed to page: x" });
    expect(await h.call(msg("invoke", { name: "x" }))).toEqual({ ok: false, error: "rpc not exposed to page: x" });
  });
});

describe("storage", () => {
  test("set / get / remove round-trip, isolated per plugin", async () => {
    const other = "o".repeat(40);
    const h = harness({ [TOKEN]: "a@x", [other]: "b@x" });
    await h.call(msg("storage.set", { key: "k", value: { n: 1 } }));
    expect(await h.call(msg("storage.get", { key: "k" }))).toEqual({ ok: true, value: { n: 1 } });
    expect(await h.call({ ...msg("storage.get", { key: "k" }), token: other })).toEqual({ ok: true, value: undefined });
    await h.call(msg("storage.remove", { key: "k" }));
    expect(await h.call(msg("storage.get", { key: "k" }))).toEqual({ ok: true, value: undefined });
  });

  test("a plugin cannot exceed 1 MiB in total", async () => {
    const h = harness();
    const chunk = "x".repeat(200 * 1024);
    for (let i = 0; i < 5; i++) expect((await h.call(msg("storage.set", { key: `k${i}`, value: chunk }))).ok).toBe(true);
    const res = await h.call(msg("storage.set", { key: "k5", value: chunk }));
    expect(res).toEqual({ ok: false, error: "storage quota exceeded" });
    expect(MAX_STORAGE_BYTES).toBe(1024 * 1024);
    // 写失败不污染已有数据
    expect((await h.call(msg("storage.get", { key: "k0" }))).ok).toBe(true);
  });

  test("prototype-polluting and oversized keys, and missing values, are rejected", async () => {
    const h = harness();
    expect((await h.call(msg("storage.set", { key: "__proto__", value: 1 }))).ok).toBe(false);
    expect((await h.call(msg("storage.set", { key: "", value: 1 }))).ok).toBe(false);
    expect((await h.call(msg("storage.set", { key: "k" }))).ok).toBe(false);
    expect(await h.call(msg("storage.get", { key: "constructor" }))).toEqual({ ok: true, value: undefined });
  });
});
