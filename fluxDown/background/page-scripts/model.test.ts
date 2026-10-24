import { describe, expect, test } from "bun:test";
import { chromeMajorVersion, probeAvailability } from "./availability";
import type { AvailabilityEnv } from "./availability";
import { mediaPayloads, mediaTypeFromMime } from "./media";
import { emptySyncState, isAllowedMatchPattern, parsePageScriptSource, parsePageScriptsIndex } from "./model";
import type { PageScriptIndexEntry, PluginApprovals } from "./model";
import { buildStatus } from "./status";

const SHA = "c".repeat(64);

function raw(patch: Record<string, unknown> = {}) {
  return {
    identity: "a@x",
    pluginName: "A",
    pluginVersion: "1.0.0",
    id: "one",
    title: "One",
    matches: ["https://*.example.com/*"],
    excludeMatches: [],
    runAt: "document_idle",
    world: "isolated",
    allFrames: false,
    sha256: SHA,
    bytes: 3,
    ...patch,
  };
}

describe("parsePageScriptsIndex", () => {
  test("accepts a well-formed index and drops invalid or duplicate entries", () => {
    const index = parsePageScriptsIndex({
      sequence: 3,
      epoch: "e",
      scripts: [
        raw(),
        raw({ id: "Bad ID" }),
        raw({ id: "two", sha256: "zz" }),
        raw({ id: "three", matches: ["<all_urls>"] }),
        raw({ id: "four", matches: ["ftp://example.com/*"] }),
        raw({ id: "five", world: "other" }),
        raw({ id: "six", runAt: "later" }),
        raw({ id: "seven", matches: [] }),
        raw(),
      ],
    });
    expect(index?.sequence).toBe(3);
    expect(index?.scripts.map((s) => s.id)).toEqual(["one"]);
  });

  test("rejects non-index payloads", () => {
    for (const bad of [null, 1, {}, { sequence: -1, epoch: "e", scripts: [] }, { sequence: 1, epoch: 2, scripts: [] }, { sequence: 1, epoch: "e", scripts: "x" }]) {
      expect(parsePageScriptsIndex(bad)).toBeNull();
    }
  });

  test("match patterns: http(s)/* schemes only, never a bare wildcard host", () => {
    expect(isAllowedMatchPattern("https://*.bilibili.com/*")).toBe(true);
    expect(isAllowedMatchPattern("*://example.com/a")).toBe(true);
    expect(isAllowedMatchPattern("*://*/*")).toBe(false);
    expect(isAllowedMatchPattern("<all_urls>")).toBe(false);
    expect(isAllowedMatchPattern("file:///*")).toBe(false);
  });

  test("source reply shape", () => {
    expect(parsePageScriptSource({ identity: "a@x", id: "one", sha256: SHA, code: "x" })?.code).toBe("x");
    expect(parsePageScriptSource({ identity: "a@x", id: "one", sha256: SHA })).toBeNull();
  });
});

describe("availability", () => {
  const ok: AvailabilityEnv = {
    userAgent: "Mozilla/5.0 Chrome/140.0.0.0 Safari/537.36",
    hasPermission: async () => true,
    userScripts: () => ({ getScripts: async () => [], getWorldConfigurations: () => [] }),
  };

  test("chrome version parsing", () => {
    expect(chromeMajorVersion("Mozilla/5.0 (X11) Chrome/138.0.7204.50 Safari/537.36")).toBe(138);
    expect(chromeMajorVersion("Mozilla/5.0 (X11; rv:140.0) Gecko/20100101 Firefox/140.0")).toBeNull();
  });

  test("ok when permission, api, and worldId support are all present", async () => {
    expect(await probeAvailability(ok)).toBe("ok");
  });

  test("Chrome older than 133 is unsupported (no per-world worldId)", async () => {
    expect(await probeAvailability({ ...ok, userAgent: "Chrome/132.0.0.0" })).toBe("browser-unsupported");
  });

  test("optional permission not granted", async () => {
    expect(await probeAvailability({ ...ok, hasPermission: async () => false })).toBe("permission-missing");
  });

  test("api undefined (Chrome 138+ switch off) or throwing (developer mode off) means switch-off", async () => {
    expect(await probeAvailability({ ...ok, userScripts: () => undefined })).toBe("switch-off");
    expect(
      await probeAvailability({
        ...ok,
        userScripts: () => ({
          getScripts: async () => {
            throw new Error("Please enable developer mode");
          },
        }),
      }),
    ).toBe("switch-off");
  });

  test("an API without getWorldConfigurations cannot isolate plugins", async () => {
    expect(await probeAvailability({ ...ok, userScripts: () => ({ getScripts: async () => [] }) })).toBe("browser-unsupported");
  });

  test("Firefox: permission is the only gate", async () => {
    expect(await probeAvailability({ ...ok, userAgent: "Mozilla/5.0 Firefox/140.0" })).toBe("ok");
    expect(await probeAvailability({ ...ok, userAgent: "Mozilla/5.0 Firefox/140.0", hasPermission: async () => false })).toBe(
      "permission-missing",
    );
  });
});

describe("buildStatus", () => {
  const entries: PageScriptIndexEntry[] = [
    { ...(raw() as unknown as PageScriptIndexEntry) },
    { ...(raw({ id: "two" }) as unknown as PageScriptIndexEntry) },
    { ...(raw({ identity: "b@x", pluginName: "B" }) as unknown as PageScriptIndexEntry) },
  ];
  const approvals: PluginApprovals = {
    "a@x": { approved: true, approvedAt: 1, token: "t".repeat(32), pluginName: "A" },
    "gone@x": { approved: true, approvedAt: 1, token: "g".repeat(32), pluginName: "Gone" },
  };

  test("states: enabled / disabled, absent plugins still listed, pending count", () => {
    const sync = { ...emptySyncState(), index: entries, outcome: "ok" as const };
    sync.registered["fd.a.x.one"] = { sha256: SHA, fingerprint: "f", world: "isolated" };
    const status = buildStatus(sync, approvals, "ok");
    const byId = Object.fromEntries(status.plugins.map((p) => [p.identity, p]));
    expect(byId["a@x"].state).toBe("enabled");
    expect(byId["a@x"].scripts.map((s) => [s.id, s.registered])).toEqual([["one", true], ["two", false]]);
    expect(byId["b@x"].state).toBe("disabled");
    expect(byId["gone@x"]).toMatchObject({ approved: true, provided: false, name: "Gone" });
    expect(status.pendingCount).toBe(1);
    expect(status.attention).toBe("pending");
  });

  test("browser-side problems map onto the approved plugin state and raise the switch hint", () => {
    const sync = { ...emptySyncState(), index: entries, outcome: "browser-unavailable" as const };
    expect(buildStatus(sync, approvals, "switch-off").plugins.find((p) => p.identity === "a@x")?.state).toBe("switch-off");
    expect(buildStatus(sync, approvals, "switch-off").attention).toBe("switch");
    expect(buildStatus(sync, approvals, "permission-missing").plugins.find((p) => p.identity === "a@x")?.state).toBe("permission-missing");
    const unsupported = buildStatus(sync, approvals, "browser-unsupported");
    expect(unsupported.plugins.find((p) => p.identity === "a@x")?.state).toBe("browser-unsupported");
    expect(unsupported.attention).toBeNull();
  });

  test("error and offline states; remote mode blocks the feature and hides the popup hint", () => {
    const withError = { ...emptySyncState(), index: entries, outcome: "ok" as const, errors: { "a@x": { code: "sha_mismatch" as const } } };
    expect(buildStatus(withError, approvals, "ok").plugins.find((p) => p.identity === "a@x")?.state).toBe("error");
    const offline = { ...emptySyncState(), index: entries, outcome: "offline" as const };
    expect(buildStatus(offline, approvals, "ok").plugins.find((p) => p.identity === "a@x")?.state).toBe("desktop-offline");
    const remote = { ...emptySyncState(), index: entries, outcome: "remote-mode" as const };
    const status = buildStatus(remote, approvals, "ok");
    expect(status.blocked).toBe("remote-mode");
    expect(status.attention).toBeNull();
  });
});

describe("addMedia payloads", () => {
  test("mime decides type; title becomes filename; source is the sender page", () => {
    expect(mediaTypeFromMime("video/mp4; codecs=avc1")).toBe("video");
    expect(mediaTypeFromMime("audio/mpeg")).toBe("audio");
    expect(mediaTypeFromMime("application/x-mpegURL")).toBe("stream");
    expect(mediaTypeFromMime(undefined)).toBe("other");
    expect(mediaPayloads([{ url: "https://a.test/x", title: "T", mime: "video/mp4; x=y" }], "https://p.test/")).toEqual([
      { url: "https://a.test/x", type: "video", filename: "T", mimeType: "video/mp4", detectedBy: "dom-scan", pageUrl: "https://p.test/" },
    ]);
  });
});
