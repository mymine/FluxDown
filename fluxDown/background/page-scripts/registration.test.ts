import { describe, expect, test } from "bun:test";
import type { PageScriptIndexEntry } from "./model";
import {
  bootstrapCode,
  buildRegistration,
  fingerprintOf,
  pluginKeyOfRegistration,
  registrationId,
  worldIdOf,
  worldProperties,
} from "./registration";
import { BOOT_GLOBAL, RUNTIME_FILE, WORLD_CSP } from "./wire";

const SHA = "a".repeat(64);

function entry(patch: Partial<PageScriptIndexEntry> = {}): PageScriptIndexEntry {
  return {
    identity: "fluxdown@bili",
    pluginName: "Bili",
    pluginVersion: "1.2.0",
    id: "bili-button",
    title: "Bili button",
    matches: ["https://*.bilibili.com/*"],
    excludeMatches: [],
    runAt: "document_idle",
    world: "isolated",
    allFrames: false,
    sha256: SHA,
    bytes: 10,
    ...patch,
  };
}

describe("naming", () => {
  test("worldId and registration id replace @ with .", () => {
    expect(worldIdOf("fluxdown@bili")).toBe("fd.fluxdown.bili");
    expect(registrationId("fluxdown@bili", "bili-button")).toBe("fd.fluxdown.bili.bili-button");
  });

  test("plugin key is recovered from a registration id; foreign ids are not ours", () => {
    expect(pluginKeyOfRegistration("fd.fluxdown.bili.bili-button")).toBe("fluxdown.bili");
    expect(pluginKeyOfRegistration("other-script")).toBeNull();
  });

  test("world configuration enables messaging and forbids eval-style loading", () => {
    expect(worldProperties("fd.fluxdown.bili")).toEqual({
      worldId: "fd.fluxdown.bili",
      messaging: true,
      csp: WORLD_CSP,
    });
    expect(WORLD_CSP).toBe("script-src 'self'; object-src 'none'");
  });
});

describe("buildRegistration", () => {
  test("isolated script: USER_SCRIPT world + three js parts (boot data, runtime file, plugin code)", () => {
    const reg = buildRegistration(entry(), "tok".repeat(12), "console.log(1);");
    expect(reg.id).toBe("fd.fluxdown.bili.bili-button");
    expect(reg.world).toBe("USER_SCRIPT");
    expect(reg.worldId).toBe("fd.fluxdown.bili");
    expect(reg.matches).toEqual(["https://*.bilibili.com/*"]);
    expect(reg.excludeMatches).toBeUndefined();
    expect(reg.runAt).toBe("document_idle");
    expect(reg.allFrames).toBe(false);
    expect(reg.js).toHaveLength(3);
    expect(reg.js[0].code).toContain(BOOT_GLOBAL);
    expect(reg.js[0].code).toContain('"scriptId":"bili-button"');
    expect(reg.js[0].code).toContain('"version":"1.2.0"');
    expect(reg.js[0].code).toContain("tok".repeat(12));
    expect(reg.js[1]).toEqual({ file: RUNTIME_FILE });
    expect(reg.js[2]).toEqual({ code: "console.log(1);" });
  });

  test("main script: single plugin-code part, no worldId, and no token anywhere", () => {
    const reg = buildRegistration(entry({ world: "main" }), "secret-token-secret-token-secret!", "run();");
    expect(reg.world).toBe("MAIN");
    expect(reg.worldId).toBeUndefined();
    expect(reg.js).toEqual([{ code: "run();" }]);
    expect(JSON.stringify(reg)).not.toContain("secret-token");
  });

  test("excludeMatches / allFrames / runAt are carried over", () => {
    const reg = buildRegistration(
      entry({ excludeMatches: ["https://www.bilibili.com/x/*"], allFrames: true, runAt: "document_start" }),
      "t".repeat(32),
      "",
    );
    expect(reg.excludeMatches).toEqual(["https://www.bilibili.com/x/*"]);
    expect(reg.allFrames).toBe(true);
    expect(reg.runAt).toBe("document_start");
  });

  test("boot code is a pure JSON assignment even for hostile field values", () => {
    const code = bootstrapCode({
      identity: 'a@b"; alert(1); //',
      scriptId: "x",
      version: "1\u2028</script>",
      token: "t".repeat(32),
    });
    const prefix = `globalThis[${JSON.stringify(BOOT_GLOBAL)}]=`;
    expect(code.startsWith(prefix)).toBe(true);
    const literal = code.slice(prefix.length, -1);
    expect(JSON.parse(literal).identity).toBe('a@b"; alert(1); //');
  });
});

describe("fingerprint", () => {
  test("changes with sha, version, matches, runAt, world, allFrames", () => {
    const base = fingerprintOf(entry());
    expect(fingerprintOf(entry())).toBe(base);
    for (const patch of [
      { sha256: "b".repeat(64) },
      { pluginVersion: "2.0.0" },
      { matches: ["https://example.com/*"] },
      { excludeMatches: ["https://example.com/a/*"] },
      { runAt: "document_start" as const },
      { world: "main" as const },
      { allFrames: true },
    ]) {
      expect(fingerprintOf(entry(patch))).not.toBe(base);
    }
  });
});
