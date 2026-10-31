import { describe, expect, test } from "bun:test";
import {
  deepLink,
  downloadErrorKey,
  formatBytes,
  formatExpiry,
  isFileIndex,
  isValidSlug,
  landingState,
  landingStatus,
  parseDownloadRequest,
  parseGrant,
  parseManifest,
  parseShareInput,
  remainingParts,
  type ShareLookup,
  type ShareManifest,
} from "../src/lib/share-landing";

const SLUG = "Ab3-_xYz09Qw";
const NOW = Date.parse("2026-10-27T00:00:00Z");

const manifest = (patch: Partial<ShareManifest> = {}): ShareManifest => ({
  id: "sh_1",
  slug: SLUG,
  title: "Holiday photos",
  files: [{ index: 0, name: "a.zip", size: 1536 }],
  totalBytes: 1536,
  expiresAt: "2026-10-30T00:00:00Z",
  cloudReady: true,
  ...patch,
});
const ok = (patch: Partial<ShareManifest> = {}): ShareLookup => ({ ok: true, manifest: manifest(patch) });
const fail = (status: number, code = "x"): ShareLookup => ({ ok: false, status, code });

describe("slug and deep link", () => {
  test("slug shape follows FluxCloud (10–12 base64url chars)", () => {
    expect(isValidSlug(SLUG)).toBe(true);
    expect(isValidSlug("a".repeat(10))).toBe(true);
    expect(isValidSlug("a".repeat(9))).toBe(false);
    expect(isValidSlug("a".repeat(13))).toBe(false);
    expect(isValidSlug("abc def ghij")).toBe(false);
    expect(isValidSlug("../../etc/pw")).toBe(false);
    expect(isValidSlug(123456)).toBe(false);
    expect(deepLink(SLUG)).toBe(`fluxdown://share/${SLUG}`);
  });

  test("file index must be a bounded non-negative integer", () => {
    for (const good of [0, 1, 999]) expect(isFileIndex(good)).toBe(true);
    for (const bad of [-1, 1.5, NaN, Infinity, "1", null, 2_000_000]) expect(isFileIndex(bad)).toBe(false);
  });
});

describe("pickup input parsing", () => {
  test("accepts bare slugs, full links and deep links", () => {
    for (const text of [
      SLUG,
      `  ${SLUG}  `,
      `https://fluxdown.com/s/${SLUG}`,
      `https://fluxdown.com/zh/s/${SLUG}/`,
      `fluxdown.com/s/${SLUG}?x=1`,
      `http://localhost:4321/s/${SLUG}#top`,
      `fluxdown://share/${SLUG}`,
    ]) {
      expect(parseShareInput(text)).toEqual({ kind: "slug", slug: SLUG });
    }
  });

  test("6-digit pickup codes are never turned into a slug", () => {
    expect(parseShareInput("123456")).toEqual({ kind: "code" });
    expect(parseShareInput(" 000123 ")).toEqual({ kind: "code" });
    expect(parseShareInput("fluxdown://share/123456")).toEqual({ kind: "code" });
  });

  test("everything else is invalid", () => {
    for (const text of ["", "abc", "https://fluxdown.com/", `https://fluxdown.com/s/${"a".repeat(30)}`, "fluxdown://share/", "1234567"]) {
      expect(parseShareInput(text)).toEqual({ kind: "invalid" });
    }
  });
});

describe("manifest and grant validation", () => {
  const raw = {
    id: "sh_1", slug: SLUG, title: "t", files: [{ index: 0, name: "a", size: 1 }],
    totalBytes: 1, expiresAt: "2026-10-30T00:00:00Z", cloudReady: false,
  };

  test("valid manifests are trimmed to known fields", () => {
    expect(parseManifest({ ...raw, extra: "x" })).toEqual(raw);
    expect(parseManifest({ ...raw, turnstileSiteKey: "0x4AAA" })?.turnstileSiteKey).toBe("0x4AAA");
    expect(parseManifest({ ...raw, turnstileSiteKey: null })?.turnstileSiteKey).toBeUndefined();
    expect(parseManifest({ ...raw, turnstileSiteKey: "" })?.turnstileSiteKey).toBeUndefined();
    expect(parseManifest({ ...raw, pickupCode: "042917" })?.pickupCode).toBe("042917");
    expect(parseManifest({ ...raw, pickupCode: null })?.pickupCode).toBeUndefined();
  });

  test("malformed manifests are rejected", () => {
    for (const bad of [
      null, [], "x", { ...raw, cloudReady: "yes" }, { ...raw, files: "nope" }, { ...raw, totalBytes: -1 },
      { ...raw, expiresAt: "tomorrow" }, { ...raw, files: [{ index: -1, name: "a", size: 1 }] },
      { ...raw, files: [{ index: 0, name: 1, size: 1 }] }, { ...raw, turnstileSiteKey: 5 },
      { ...raw, pickupCode: 42917 }, { ...raw, pickupCode: "12345" }, { ...raw, pickupCode: "12a456" },
      { ...raw, title: undefined },
    ]) {
      expect(parseManifest(bad)).toBeNull();
    }
  });

  test("grants only accept http(s) urls", () => {
    const expiresAt = "2026-10-27T01:00:00Z";
    expect(parseGrant({ url: "https://cdn.example/f?sig=1", expiresAt })).toEqual({ url: "https://cdn.example/f?sig=1", expiresAt });
    for (const url of ["javascript:alert(1)", "data:text/html,x", "/relative", "ftp://x/y", "", 5]) {
      expect(parseGrant({ url, expiresAt })).toBeNull();
    }
    expect(parseGrant({ url: "https://cdn.example/f", expiresAt: "soon" })).toBeNull();
    expect(parseGrant(null)).toBeNull();
  });

  test("download request body: index required, token optional and bounded", () => {
    expect(parseDownloadRequest(JSON.stringify({ fileIndex: 2 }))).toEqual({ ok: true, fileIndex: 2 });
    expect(parseDownloadRequest(JSON.stringify({ fileIndex: 0, turnstileToken: "tok" }))).toEqual({ ok: true, fileIndex: 0, turnstileToken: "tok" });
    expect(parseDownloadRequest(JSON.stringify({ fileIndex: 0, turnstileToken: "" }))).toEqual({ ok: true, fileIndex: 0 });
    expect(parseDownloadRequest(JSON.stringify({ fileIndex: 0, turnstileToken: null }))).toEqual({ ok: true, fileIndex: 0 });
    for (const bad of [
      "", "not json", "[]", "null", JSON.stringify({}), JSON.stringify({ fileIndex: -1 }),
      JSON.stringify({ fileIndex: "1" }), JSON.stringify({ fileIndex: 1.5 }),
      JSON.stringify({ fileIndex: 0, turnstileToken: 7 }),
      JSON.stringify({ fileIndex: 0, turnstileToken: "x".repeat(2049) }),
      JSON.stringify({ fileIndex: 0, pad: "x".repeat(5000) }),
    ]) {
      expect(parseDownloadRequest(bad)).toEqual({ ok: false });
    }
  });
});

describe("state machine", () => {
  test("cloud-ready vs not-ready manifests", () => {
    expect(landingState(ok(), NOW)).toBe("ready");
    expect(landingState(ok({ cloudReady: false }), NOW)).toBe("notReady");
    expect(landingStatus(ok(), NOW)).toBe(200);
    expect(landingStatus(ok({ cloudReady: false }), NOW)).toBe(200);
  });

  test("ended / missing keep 410 / 404; expired manifests count as ended", () => {
    expect(landingState(fail(404), NOW)).toBe("ended");
    expect(landingStatus(fail(404), NOW)).toBe(404);
    expect(landingState(fail(410, "share_ended"), NOW)).toBe("ended");
    expect(landingStatus(fail(410, "share_takedown"), NOW)).toBe(410);
    expect(landingState(ok({ expiresAt: "2026-10-26T23:59:59Z" }), NOW)).toBe("ended");
    expect(landingStatus(ok({ expiresAt: "2026-10-26T23:59:59Z" }), NOW)).toBe(410);
  });

  test("upstream trouble is not reported as ended", () => {
    expect(landingState(fail(429), NOW)).toBe("rateLimited");
    expect(landingStatus(fail(429), NOW)).toBe(429);
    for (const status of [500, 502, 503]) {
      expect(landingState(fail(status), NOW)).toBe("unavailable");
      expect(landingStatus(fail(status), NOW)).toBe(503);
    }
  });

  test("download error mapping prefers the FluxCloud code", () => {
    expect(downloadErrorKey(403, "traffic_exhausted")).toBe("trafficExhausted");
    expect(downloadErrorKey(403, "turnstile_required")).toBe("turnstileRequired");
    expect(downloadErrorKey(400, "turnstile_failed")).toBe("turnstileFailed");
    expect(downloadErrorKey(409, "share_cloud_not_ready")).toBe("notReady");
    expect(downloadErrorKey(410, "share_takedown")).toBe("ended");
    expect(downloadErrorKey(404, undefined)).toBe("ended");
    expect(downloadErrorKey(409, undefined)).toBe("notReady");
    expect(downloadErrorKey(429, undefined)).toBe("rateLimited");
    expect(downloadErrorKey(502, "upstream_unreachable")).toBe("generic");
    expect(downloadErrorKey(500, undefined)).toBe("generic");
  });
});

describe("formatting", () => {
  test("sizes use 1024 units with at most one decimal", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1024 * 1024 * 100)).toBe("100 MB");
    expect(formatBytes(1024 ** 3 * 2.25)).toBe("2.3 GB");
    expect(formatBytes(1024 ** 2 - 1)).toBe("1 MB");
    expect(formatBytes(1024 ** 4 * 3)).toBe("3 TB");
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(NaN)).toBe("—");
  });

  test("expiry is rendered in UTC per language; invalid input is null", () => {
    expect(formatExpiry("2026-10-30T08:05:00Z", "en")).toMatch(/^Oct 30, 2026,? (at )?08:05 UTC$/);
    expect(formatExpiry("2026-10-30T08:05:00Z", "zh")).toContain("2026");
    expect(formatExpiry("2026-10-30T08:05:00Z", "zh")?.endsWith(" UTC")).toBe(true);
    expect(formatExpiry("garbage", "en")).toBeNull();
  });

  test("remaining time splits into days / hours / minutes", () => {
    expect(remainingParts("2026-10-29T03:30:00Z", NOW)).toEqual({ days: 2, hours: 3, minutes: 30 });
    expect(remainingParts("2026-10-27T00:05:00Z", NOW)).toEqual({ days: 0, hours: 0, minutes: 5 });
    expect(remainingParts("2026-10-26T00:00:00Z", NOW)).toBeNull();
    expect(remainingParts("2026-10-27T00:00:00Z", NOW)).toBeNull();
    expect(remainingParts("nope", NOW)).toBeNull();
  });
});

describe("crawler policy", () => {
  test("robots.txt disallows /s/ and /api/ in every group", async () => {
    const text = await Bun.file(new URL("../public/robots.txt", import.meta.url)).text();
    const groups = text.split(/\n\s*\n/).filter((block) => /^User-agent:/m.test(block));
    expect(groups.length).toBeGreaterThanOrEqual(2);
    for (const group of groups) {
      for (const rule of ["Disallow: /api/", "Disallow: /s/", "Disallow: /zh/s/"]) {
        expect(group).toContain(rule);
      }
    }
  });

  test("sitemap filter excludes the /s/ explainer in both languages", async () => {
    const config = await Bun.file(new URL("../astro.config.mjs", import.meta.url)).text();
    const set = config.match(/const noindexPathnames = new Set\(\[([\s\S]*?)\]\);/)?.[1] ?? "";
    for (const path of ['"/s"', '"/zh/s"']) expect(set).toContain(path);
  });
});
