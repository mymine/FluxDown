import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  networkHostsOf,
  riskOf,
  validateManifest,
  validateSniffPack,
  VALID_PERMISSIONS,
  type PluginRisk,
} from "./manifest";

interface Case {
  name: string;
  manifest: unknown;
  files: string[];
  ok: boolean;
  risk?: PluginRisk;
  networkHosts?: string[];
}

const cases: Case[] = JSON.parse(
  readFileSync(new URL("../../../native/engine/src/plugin/fixtures/manifest_cases.json", import.meta.url), "utf8"),
);

describe("manifest fixtures (shared with Rust)", () => {
  test("fixture count", () => {
    expect(cases.length).toBeGreaterThanOrEqual(30);
  });
  for (const c of cases) {
    test(c.name, () => {
      const result = validateManifest(c.manifest, new Set(c.files));
      expect(result.ok, result.errors.join("; ")).toBe(c.ok);
      expect(result.ok).toBe(result.errors.length === 0);
      if (result.ok) {
        const m = c.manifest as Parameters<typeof riskOf>[0] & Parameters<typeof networkHostsOf>[0];
        if (c.risk) expect(riskOf(m)).toBe(c.risk);
        if (c.networkHosts) expect(networkHostsOf(m)).toEqual(c.networkHosts);
      }
    });
  }
});

describe("strict mode", () => {
  const base = { identity: "a@b", name: "N", version: "1.0.0" };
  test("requires entries to exist", () => {
    const m = { ...base, hooks: { entry: "h.js", events: ["onDone"] } };
    expect(validateManifest(m, new Set()).ok).toBe(true);
    expect(validateManifest(m, new Set(), { strict: true }).ok).toBe(false);
    expect(validateManifest(m, new Set(["h.js"]), { strict: true }).ok).toBe(true);
  });
  test("version must be plain three-part", () => {
    expect(validateManifest({ ...base, version: "v1.0.0-rc.1" }, new Set()).ok).toBe(true);
    expect(validateManifest({ ...base, version: "v1.0.0-rc.1" }, new Set(), { strict: true }).ok).toBe(false);
  });
  test("omitted files skip existence checks; strict without files is a clear error", () => {
    const m = { ...base, main: { entry: "main.js" }, hooks: { entry: "h.js", events: ["onDone"] } };
    expect(validateManifest(m).ok).toBe(true);
    expect(validateManifest(m, null).ok).toBe(true);
    expect(validateManifest({ ...base, version: "x" }).ok).toBe(false);
    const strict = validateManifest(m, undefined, { strict: true });
    expect(strict.ok).toBe(false);
    expect(strict.errors).toHaveLength(1);
    expect(strict.errors[0]).toContain("files");
  });
});

describe("misc rules", () => {
  const base = { identity: "a@b", name: "N", version: "1.0.0" };
  test("legacy window key is rejected", () => {
    expect(validateManifest({ ...base, window: { entry: "w.js" } }, new Set(["w.js"])).ok).toBe(false);
  });
  test("settings matrix", () => {
    const bad = { ...base, settings: [{ key: "k", title: "K", type: "number", widget: "text" }] };
    expect(validateManifest(bad, new Set()).ok).toBe(false);
    const good = { ...base, settings: [{ key: "k", title: "K", type: "number", min: 1, max: 5, default: "3" }] };
    expect(validateManifest(good, new Set()).ok).toBe(true);
  });
  test("eight permissions", () => {
    expect(VALID_PERMISSIONS).toHaveLength(8);
  });
  test("ipv6 network hosts", () => {
    const m = (host: string) => ({
      ...base,
      permissions: ["desktop"],
      contributes: { views: [{ id: "w", title: "T", location: "window", desktop: { entry: "w.js", networkHosts: [host] } }] },
    });
    const files = new Set(["w.js"]);
    expect(validateManifest(m("::1"), files).ok).toBe(true);
    expect(validateManifest(m("2001:db8::1"), files).ok).toBe(true);
    expect(validateManifest(m("2001:DB8::1"), files).ok).toBe(false);
    expect(validateManifest(m("1.2.3.4"), files).ok).toBe(true);
    // Not an IP literal, but numeric labels are valid DNS labels (same as the Rust validator).
    expect(validateManifest(m("1.2.3.256"), files).ok).toBe(true);
  });
});

describe("validateSniffPack", () => {
  test("accepts a valid pack", () => {
    expect(validateSniffPack('{"format":"fluxdown-sniff-pack","rules":[]}').ok).toBe(true);
  });
  test("rejects wrong format, missing rules, bad JSON", () => {
    expect(validateSniffPack('{"format":"other","rules":[]}').ok).toBe(false);
    expect(validateSniffPack('{"format":"fluxdown-sniff-pack"}').ok).toBe(false);
    expect(validateSniffPack("not json").ok).toBe(false);
  });
  test("rule count and size limits", () => {
    const many = `{"format":"fluxdown-sniff-pack","rules":[${Array(501).fill("{}").join(",")}]}`;
    expect(validateSniffPack(many).ok).toBe(false);
    expect(validateSniffPack(new Uint8Array(256 * 1024 + 1).fill(32)).ok).toBe(false);
  });
});
