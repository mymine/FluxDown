import { describe, expect, test } from "bun:test";
import { createStorageArea } from "@/utils/storage-mock.test-util";
import { sha256Hex } from "./hash";
import type { PageScriptIndexEntry, PageScriptsAvailability, PageScriptsIndex } from "./model";
import { planSync } from "./plan";
import { fingerprintOf, registrationId } from "./registration";
import { createPageScriptStore } from "./store";
import type { PageScriptStore } from "./store";
import { createPageScriptSync } from "./sync";
import type { SourceResult, SyncCaps, SyncDeps } from "./sync";
import type { Browser } from "wxt/browser";

type Registered = Browser.userScripts.RegisteredUserScript;

const CAPS: SyncCaps = {
  protocol: 2,
  connected: true,
  actions: ["page_scripts", "page_script_source"],
  pageScriptsSequence: 1,
  pageScriptsEpoch: "e1",
};

async function entryFor(identity: string, id: string, code: string, patch: Partial<PageScriptIndexEntry> = {}) {
  const entry: PageScriptIndexEntry = {
    identity,
    pluginName: identity,
    pluginVersion: "1.0.0",
    id,
    title: id,
    matches: ["https://example.com/*"],
    excludeMatches: [],
    runAt: "document_idle",
    world: "isolated",
    allFrames: false,
    sha256: await sha256Hex(code),
    bytes: code.length,
    ...patch,
  };
  return entry;
}

interface Harness {
  deps: SyncDeps;
  store: PageScriptStore;
  registered: Map<string, Registered>;
  calls: string[];
  sources: Map<string, string>;
  state: {
    index: PageScriptsIndex | null;
    caps: SyncCaps;
    availability: PageScriptsAvailability;
    remote: boolean;
    sourceFail: Set<string>;
    sourceOverride: Map<string, SourceResult>;
    registerFail: boolean;
  };
}

function harness(): Harness {
  const store = createPageScriptStore(createStorageArea());
  const registered = new Map<string, Registered>();
  const calls: string[] = [];
  const sources = new Map<string, string>();
  const state: Harness["state"] = {
    index: null,
    caps: CAPS,
    availability: "ok",
    remote: false,
    sourceFail: new Set(),
    sourceOverride: new Map(),
    registerFail: false,
  };
  const deps: SyncDeps = {
    store,
    availability: async () => state.availability,
    capabilities: async () => state.caps,
    remoteMode: async () => state.remote,
    fetchIndex: async () => state.index,
    fetchSource: async (identity, id): Promise<SourceResult> => {
      const key = `${identity}/${id}`;
      calls.push(`fetch ${key}`);
      const override = state.sourceOverride.get(key);
      if (override) return override;
      if (state.sourceFail.has(key)) return { ok: false, message: "timeout" };
      const code = sources.get(key) ?? "";
      return { ok: true, source: { identity, id, sha256: await sha256Hex(code), code } };
    },
    sha256: sha256Hex,
    scripts: {
      getIds: async () => [...registered.keys()],
      register: async (list) => {
        if (state.registerFail) throw new Error("bad match pattern");
        for (const s of list) {
          if (registered.has(s.id)) throw new Error(`duplicate ${s.id}`);
        }
        for (const s of list) {
          registered.set(s.id, s);
          calls.push(`register ${s.id}`);
        }
      },
      update: async (list) => {
        for (const s of list) {
          if (!registered.has(s.id)) throw new Error(`missing ${s.id}`);
        }
        for (const s of list) {
          registered.set(s.id, s);
          calls.push(`update ${s.id}`);
        }
      },
      unregister: async (ids) => {
        for (const id of ids) {
          registered.delete(id);
          calls.push(`unregister ${id}`);
        }
      },
      configureWorld: async (props) => void calls.push(`world ${props.worldId}`),
      resetWorld: async (id) => void calls.push(`resetWorld ${id}`),
    },
    now: () => 1_000,
  };
  return { deps, store, registered, calls, sources, state };
}

async function setIndex(h: Harness, ...entries: PageScriptIndexEntry[]) {
  h.state.index = { sequence: 1, epoch: "e1", scripts: entries };
}

describe("planSync", () => {
  test("only approved plugins are wanted; missing or changed scripts are upserted; foreign ids are ignored", async () => {
    const a = await entryFor("a@x", "one", "A1");
    const b = await entryFor("b@x", "one", "B1");
    const plan = planSync(
      [a, b],
      new Set(["a@x"]),
      ["fd.a.x.one", "fd.a.x.gone", "fd.b.x.one", "foreign"],
      { "fd.a.x.one": { sha256: a.sha256, fingerprint: fingerprintOf(a), world: "isolated" } },
    );
    expect(plan.plugins.map((p) => [p.identity, p.upsert.length])).toEqual([["a@x", 0]]);
    expect(plan.orphans.sort()).toEqual(["fd.a.x.gone", "fd.b.x.one"]);
  });
});

describe("reconcile", () => {
  test("approved plugin: new script is fetched and registered; unapproved plugin is not", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "console.log('a')");
    const b = await entryFor("b@x", "one", "console.log('b')");
    h.sources.set("a@x/one", "console.log('a')");
    h.sources.set("b@x/one", "console.log('b')");
    await setIndex(h, a, b);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);

    expect(await sync.reconcile("user")).toBe("ok");
    expect([...h.registered.keys()]).toEqual([registrationId("a@x", "one")]);
    expect(h.calls).toContain("world fd.a.x");
    expect(h.calls.some((c) => c.includes("b@x"))).toBe(false);
    const state = await h.store.loadSync();
    expect(state.registered[registrationId("a@x", "one")].sha256).toBe(a.sha256);
    expect(state.applied).toEqual({ sequence: 1, epoch: "e1" });
    expect(state.index).toHaveLength(2);
  });

  test("unchanged scripts are neither fetched nor re-registered", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "X");
    h.sources.set("a@x/one", "X");
    await setIndex(h, a);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");
    h.calls.length = 0;
    await sync.reconcile("user");
    expect(h.calls).toEqual([]);
  });

  test("changed sha256 fetches and updates in place; changed matches alone also re-registers", async () => {
    const h = harness();
    const v1 = await entryFor("a@x", "one", "V1");
    h.sources.set("a@x/one", "V1");
    await setIndex(h, v1);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");

    const v2 = await entryFor("a@x", "one", "V2");
    h.sources.set("a@x/one", "V2");
    await setIndex(h, v2);
    h.calls.length = 0;
    await sync.reconcile("user");
    expect(h.calls).toEqual(["fetch a@x/one", "world fd.a.x", "update fd.a.x.one"]);
    expect(h.registered.get("fd.a.x.one")?.js.at(-1)).toEqual({ code: "V2" });

    const v2b = await entryFor("a@x", "one", "V2", { matches: ["https://other.example/*"] });
    await setIndex(h, v2b);
    h.calls.length = 0;
    await sync.reconcile("user");
    expect(h.calls).toContain("update fd.a.x.one");
    expect(h.registered.get("fd.a.x.one")?.matches).toEqual(["https://other.example/*"]);
  });

  test("world change replaces the registration (unregister then register)", async () => {
    const h = harness();
    const iso = await entryFor("a@x", "one", "X");
    h.sources.set("a@x/one", "X");
    await setIndex(h, iso);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");
    await setIndex(h, await entryFor("a@x", "one", "X", { world: "main" }));
    h.calls.length = 0;
    await sync.reconcile("user");
    expect(h.calls).toEqual(["fetch a@x/one", "unregister fd.a.x.one", "register fd.a.x.one"]);
    expect(h.registered.get("fd.a.x.one")?.world).toBe("MAIN");
  });

  test("scripts that vanish from the index, and plugins that are no longer provided, are unregistered", async () => {
    const h = harness();
    const one = await entryFor("a@x", "one", "1");
    const two = await entryFor("a@x", "two", "2");
    h.sources.set("a@x/one", "1");
    h.sources.set("a@x/two", "2");
    await setIndex(h, one, two);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");
    expect(h.registered.size).toBe(2);

    await setIndex(h, one);
    await sync.reconcile("user");
    expect([...h.registered.keys()]).toEqual(["fd.a.x.one"]);

    await setIndex(h);
    await sync.reconcile("user");
    expect(h.registered.size).toBe(0);
    expect(Object.keys((await h.store.loadSync()).registered)).toEqual([]);
  });

  test("source whose sha256 does not match the index is not registered and the error is recorded", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "GOOD");
    await setIndex(h, a);
    // 桌面端应答的 sha256 自洽但与索引不符。
    h.state.sourceOverride.set("a@x/one", {
      ok: true,
      source: { identity: "a@x", id: "one", sha256: await sha256Hex("EVIL"), code: "EVIL" },
    });
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");
    expect(h.registered.size).toBe(0);
    expect((await h.store.loadSync()).errors["a@x"]).toEqual({ code: "sha_mismatch", scriptId: "one" });

    // 应答声称的 sha256 与索引一致但内容被换：同样拒绝。
    h.state.sourceOverride.set("a@x/one", {
      ok: true,
      source: { identity: "a@x", id: "one", sha256: a.sha256, code: "EVIL" },
    });
    await sync.reconcile("user");
    expect(h.registered.size).toBe(0);
  });

  test("a failing plugin does not affect others, and is atomic (none of its scripts change)", async () => {
    const h = harness();
    const a1 = await entryFor("a@x", "one", "A1");
    const a2 = await entryFor("a@x", "two", "A2");
    const b1 = await entryFor("b@x", "one", "B1");
    h.sources.set("a@x/one", "A1");
    h.sources.set("a@x/two", "A2");
    h.sources.set("b@x/one", "B1");
    h.state.sourceFail.add("a@x/two");
    await setIndex(h, a1, a2, b1);
    await h.store.approve("a@x", "A", 1);
    await h.store.approve("b@x", "B", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");
    expect([...h.registered.keys()]).toEqual(["fd.b.x.one"]);
    const state = await h.store.loadSync();
    expect(state.errors["a@x"]?.code).toBe("fetch_failed");
    expect(state.errors["b@x"]).toBeUndefined();
    // 暂时性失败：不标记已应用，下次仍会重试。
    expect(state.applied).toBeUndefined();

    h.state.sourceFail.clear();
    await sync.reconcile("user");
    expect(h.registered.size).toBe(3);
    expect((await h.store.loadSync()).errors["a@x"]).toBeUndefined();
  });

  test("browser refusing registration records register_failed for that plugin only", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "A");
    h.sources.set("a@x/one", "A");
    await setIndex(h, a);
    await h.store.approve("a@x", "A", 1);
    h.state.registerFail = true;
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");
    expect((await h.store.loadSync()).errors["a@x"]).toEqual({ code: "register_failed", detail: "bad match pattern" });
  });

  test("desktop unreachable keeps existing registrations untouched", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "A");
    h.sources.set("a@x/one", "A");
    await setIndex(h, a);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");

    h.calls.length = 0;
    h.state.index = null; // fetchIndex 失败
    expect(await sync.reconcile("user")).toBe("offline");
    expect(h.registered.size).toBe(1);
    expect(h.calls).toEqual([]);

    h.state.caps = { ...CAPS, connected: false };
    expect(await sync.reconcile("user")).toBe("offline");
    expect(h.registered.size).toBe(1);
  });

  test("remote 'always' mode and an old desktop never touch registrations", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "A");
    h.sources.set("a@x/one", "A");
    await setIndex(h, a);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");

    h.state.remote = true;
    expect(await sync.reconcile("user")).toBe("remote-mode");
    h.state.remote = false;
    h.state.caps = { ...CAPS, actions: [] };
    expect(await sync.reconcile("user")).toBe("host-unsupported");
    expect(h.registered.size).toBe(1);
  });

  test("browser-side unavailability caches the index but registers nothing", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "A");
    await setIndex(h, a);
    await h.store.approve("a@x", "A", 1);
    h.state.availability = "switch-off";
    const sync = createPageScriptSync(h.deps);
    expect(await sync.reconcile("user")).toBe("browser-unavailable");
    expect(h.registered.size).toBe(0);
    expect((await h.store.loadSync()).index).toHaveLength(1);
  });

  test("capabilities trigger is a no-op when sequence/epoch are unchanged, and runs when they change", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "A");
    h.sources.set("a@x/one", "A");
    await setIndex(h, a);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");

    let indexFetches = 0;
    const originalFetch = h.deps.fetchIndex;
    h.deps.fetchIndex = async () => {
      indexFetches++;
      return originalFetch();
    };
    await sync.reconcile("capabilities", CAPS);
    expect(indexFetches).toBe(0);
    await sync.reconcile("capabilities", { ...CAPS, pageScriptsSequence: 2 });
    expect(indexFetches).toBe(1);
    await sync.reconcile("capabilities", { ...CAPS, pageScriptsSequence: 2, pageScriptsEpoch: "e2" });
    expect(indexFetches).toBe(2);
    await sync.reconcile("capabilities", { ...CAPS, pageScriptsSequence: null, pageScriptsEpoch: null });
    expect(indexFetches).toBe(2);
  });

  test("removePlugin unregisters every script of that plugin and resets its world, without the desktop", async () => {
    const h = harness();
    const a1 = await entryFor("a@x", "one", "1");
    const b1 = await entryFor("b@x", "one", "2");
    h.sources.set("a@x/one", "1");
    h.sources.set("b@x/one", "2");
    await setIndex(h, a1, b1);
    await h.store.approve("a@x", "A", 1);
    await h.store.approve("b@x", "B", 1);
    const sync = createPageScriptSync(h.deps);
    await sync.reconcile("user");
    await sync.removePlugin("a@x");
    expect([...h.registered.keys()]).toEqual(["fd.b.x.one"]);
    expect(h.calls).toContain("resetWorld fd.a.x");
  });

  test("concurrent reconciles are serialized and coalesced", async () => {
    const h = harness();
    const a = await entryFor("a@x", "one", "A");
    h.sources.set("a@x/one", "A");
    await setIndex(h, a);
    await h.store.approve("a@x", "A", 1);
    const sync = createPageScriptSync(h.deps);
    await Promise.all([sync.reconcile("user"), sync.reconcile("user"), sync.reconcile("user")]);
    expect(h.calls.filter((c) => c.startsWith("register")).length).toBe(1);
  });
});
