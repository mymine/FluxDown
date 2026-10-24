import { describe, expect, test } from "bun:test";
import { createFluxdown, installRuntime } from "./runtime-core";
import type { Fluxdown } from "./runtime-core";
import { bootstrapCode } from "./registration";
import { BOOT_GLOBAL } from "./wire";
import type { BridgeRequest, BridgeResponse } from "./wire";

const TOKEN = "SECRET-TOKEN-0123456789abcdef0123456789abcdef";
const BOOT = { identity: "fluxdown@bili", scriptId: "button", version: "1.0.0", token: TOKEN };

interface FakeWorld extends Record<string, unknown> {
  sent: BridgeRequest[];
}

/** 模拟 USER_SCRIPT world：先执行 bootstrap 代码赋值，再装运行时。 */
function world(reply: (message: BridgeRequest) => BridgeResponse | undefined, withRuntime = true): FakeWorld {
  const sent: BridgeRequest[] = [];
  const g: FakeWorld = { sent };
  if (withRuntime) {
    g.chrome = {
      runtime: {
        sendMessage: async (message: BridgeRequest) => {
          sent.push(message);
          return reply(message);
        },
      },
    };
  }
  // bootstrapCode 的内容是 `globalThis["…"]={…};`：在测试里按其字面量形式还原。
  const code = bootstrapCode(BOOT);
  const literal = code.slice(code.indexOf("=") + 1, -1);
  g[BOOT_GLOBAL] = JSON.parse(literal);
  return g;
}

describe("installRuntime", () => {
  test("reads and deletes the boot global; installs a frozen fluxdown", () => {
    const g = world(() => ({ ok: true }));
    expect(installRuntime(g)).toBe(true);
    expect(BOOT_GLOBAL in g).toBe(false);
    const fluxdown = g.fluxdown as Record<string, unknown>;
    expect(Object.isFrozen(fluxdown)).toBe(true);
    expect(Object.keys(fluxdown).sort()).toEqual(["addMedia", "download", "info", "invoke", "storage"]);
    expect(Object.isFrozen(fluxdown.info)).toBe(true);
    expect(Object.isFrozen(fluxdown.storage)).toBe(true);
    expect(fluxdown.info).toEqual({ identity: "fluxdown@bili", scriptId: "button", version: "1.0.0" });
  });

  test("the token is not reachable from the exposed object, its info, or the global scope", () => {
    const g = world(() => ({ ok: true }));
    installRuntime(g);
    const fluxdown = g.fluxdown as { storage: object; info: object };
    const surfaces = [
      JSON.stringify(fluxdown),
      JSON.stringify(fluxdown.info),
      JSON.stringify(Object.getOwnPropertyNames(fluxdown)),
      Object.values(fluxdown).map(String).join("\n"),
      Object.values(fluxdown.storage).map(String).join("\n"),
      JSON.stringify(Object.keys(g)),
    ];
    for (const surface of surfaces) expect(surface).not.toContain(TOKEN);
    expect(Object.getOwnPropertySymbols(fluxdown)).toHaveLength(0);
  });

  test("the token is sent with every call, but only inside the message", async () => {
    const g = world((m) => (m.op === "storage.get" ? { ok: true, value: 5 } : { ok: true, value: { ok: true } }));
    installRuntime(g);
    const fluxdown = g.fluxdown as Fluxdown;
    expect(await fluxdown.storage.get("k")).toBe(5);
    await fluxdown.download({ url: "https://a.test/f" });
    expect(g.sent.every((m) => m.token === TOKEN && m.v === 1)).toBe(true);
    expect(g.sent.map((m) => m.op)).toEqual(["storage.get", "download"]);
  });

  test("does nothing without boot data or without a messaging runtime", () => {
    const bare: Record<string, unknown> = {};
    expect(installRuntime(bare)).toBe(false);
    expect(bare.fluxdown).toBeUndefined();

    const noMessaging = world(() => undefined, false);
    expect(installRuntime(noMessaging)).toBe(false);
    expect(noMessaging.fluxdown).toBeUndefined();
    expect(BOOT_GLOBAL in noMessaging).toBe(false);

    const broken: Record<string, unknown> = { [BOOT_GLOBAL]: { identity: "a@b", token: "" } };
    expect(installRuntime(broken)).toBe(false);
  });

  test("the first script's api cannot be re-assigned by plugin code, but a later script of the same plugin may redefine it", () => {
    const g = world(() => ({ ok: true }));
    installRuntime(g);
    const first = g.fluxdown;
    expect(() => {
      "use strict";
      g.fluxdown = {};
    }).toThrow();
    expect(g.fluxdown).toBe(first);
    g[BOOT_GLOBAL] = { ...BOOT, scriptId: "second" };
    expect(installRuntime(g)).toBe(true);
    expect((g.fluxdown as { info: { scriptId: string } }).info.scriptId).toBe("second");
  });
});

describe("bridge calls", () => {
  test("errors from the extension reject with the message", async () => {
    const g = world(() => ({ ok: false, error: "rpc not exposed to page: x" }));
    installRuntime(g);
    const fluxdown = g.fluxdown as Fluxdown;
    await expect(fluxdown.invoke("x")).rejects.toThrow("rpc not exposed to page: x");
  });

  test("missing or malformed responses reject", async () => {
    const g = world(() => undefined);
    installRuntime(g);
    const fluxdown = g.fluxdown as Fluxdown;
    await expect(fluxdown.addMedia([])).rejects.toThrow("no response");
  });

  test("a failing sendMessage rejects with a readable error", async () => {
    const fluxdown = createFluxdown(BOOT, async () => {
      throw new Error("Extension context invalidated");
    });
    await expect(fluxdown.storage.set("k", 1)).rejects.toThrow("Extension context invalidated");
  });

  test("download maps the reply and addMedia returns the accepted count", async () => {
    const fluxdown = createFluxdown(BOOT, async (m) =>
      m.op === "download" ? { ok: true, value: { ok: false, message: "nope" } } : { ok: true, value: 3 },
    );
    expect(await fluxdown.download({ url: "https://a.test/f" })).toEqual({ ok: false, message: "nope" });
    expect(await fluxdown.addMedia([{ url: "https://a.test/f" }])).toBe(3);
    await expect(fluxdown.addMedia("x" as never)).rejects.toThrow(TypeError);
  });
});
