import { describe, expect, test } from "bun:test";
import { createStorageArea } from "@/utils/storage-mock.test-util";
import { APPROVALS_KEY, createPageScriptStore, pluginStorageKey, SYNC_KEY } from "./store";

function setup() {
  const area = createStorageArea();
  return { area, store: createPageScriptStore(area) };
}

describe("approvals", () => {
  test("approve generates a persistent random token; re-approving keeps it", async () => {
    const { store, area } = setup();
    const first = await store.approve("a@x", "A", 100);
    expect(first.token).toMatch(/^[0-9a-f]{64}$/);
    expect(first.approvedAt).toBe(100);
    const again = await store.approve("a@x", "A renamed", 200);
    expect(again.token).toBe(first.token);
    expect(again.approvedAt).toBe(100);
    expect(again.pluginName).toBe("A renamed");
    const other = await store.approve("b@x", "B", 100);
    expect(other.token).not.toBe(first.token);
    expect(Object.keys(area.data[APPROVALS_KEY] as object).sort()).toEqual(["a@x", "b@x"]);
  });

  test("token lookup only matches approved plugins", async () => {
    const { store } = setup();
    const { token } = await store.approve("a@x", "A", 1);
    expect(await store.identityForToken(token)).toBe("a@x");
    expect(await store.identityForToken("0".repeat(64))).toBeNull();
    expect(await store.identityForToken("")).toBeNull();
  });

  test("revoke deletes approval, token, plugin storage, and registered records", async () => {
    const { store, area } = setup();
    const { token } = await store.approve("a@x", "A", 1);
    await store.storageSet("a@x", "k", 1);
    await store.updateSync((s) => {
      s.registered["fd.a.x.one"] = { sha256: "s", fingerprint: "f", world: "isolated" };
      s.registered["fd.b.x.one"] = { sha256: "s", fingerprint: "f", world: "isolated" };
      s.errors["a@x"] = { code: "fetch_failed" };
    });
    expect(await store.revoke("a@x")).toBe(true);
    expect(await store.identityForToken(token)).toBeNull();
    expect(area.data[pluginStorageKey("a@x")]).toBeUndefined();
    const sync = await store.loadSync();
    expect(Object.keys(sync.registered)).toEqual(["fd.b.x.one"]);
    expect(sync.errors["a@x"]).toBeUndefined();
    expect(await store.revoke("a@x")).toBe(false);
  });

  test("corrupted approval entries are ignored", async () => {
    const { store, area } = setup();
    area.data[APPROVALS_KEY] = { "a@x": { approved: true, approvedAt: 1, token: "short", pluginName: "A" }, "b@x": 5 };
    expect(await store.loadApprovals()).toEqual({});
  });

  test("approved state never stores script source", async () => {
    const { store, area } = setup();
    await store.approve("a@x", "A", 1);
    await store.updateSync((s) => {
      s.registered["fd.a.x.one"] = { sha256: "s", fingerprint: "f", world: "isolated" };
    });
    expect(JSON.stringify(area.data[SYNC_KEY])).not.toContain("code");
  });
});

describe("plugin storage", () => {
  test("values are namespaced per plugin and reads ignore inherited properties", async () => {
    const { store } = setup();
    await store.storageSet("a@x", "k", { n: 1 });
    expect(await store.storageGet("a@x", "k")).toEqual({ n: 1 });
    expect(await store.storageGet("b@x", "k")).toBeUndefined();
    expect(await store.storageGet("a@x", "constructor")).toBeUndefined();
    await store.storageRemove("a@x", "k");
    expect(await store.storageGet("a@x", "k")).toBeUndefined();
  });

  test("non-serializable values and quota overflow are refused", async () => {
    const { store } = setup();
    await expect(store.storageSet("a@x", "k", undefined)).rejects.toThrow("JSON");
    await expect(store.storageSet("a@x", "k", () => 1)).rejects.toThrow("JSON");
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    await expect(store.storageSet("a@x", "k", cyclic)).rejects.toThrow("JSON");
    await expect(store.storageSet("a@x", "big", "x".repeat(1024 * 1024 + 1))).rejects.toThrow("quota");
    expect(await store.storageGet("a@x", "big")).toBeUndefined();
  });
});
