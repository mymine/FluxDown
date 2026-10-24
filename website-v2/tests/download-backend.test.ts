import { describe, expect, test } from "bun:test";
import { DownloadLookupCache } from "../src/lib/download-cache";
import { createOssProbe } from "../src/lib/download-probe";
import {
  DownloadLookupError, downloadSource, latestWithAsset, matchesAssetSize,
  ossUnavailable, downloadRedirect, transientGitHubStatus, isGitHubDownloadUrl,
  type DownloadRelease,
} from "../src/lib/download-policy";

const temporary = () => Promise.reject(new DownloadLookupError("offline", true));

describe("download policy", () => {
  test("explicit sources and legacy compatibility", () => {
    expect(downloadSource(null)).toBe("legacy");
    expect(downloadSource("unknown")).toBe("legacy");
    expect(downloadSource("oss")).toBe("oss");
    expect(downloadSource("github")).toBe("github");
    expect(downloadSource("auto")).toBe("auto");
    const unavailable = ossUnavailable();
    expect(unavailable.status).toBe(503);
    expect(unavailable.headers.has("Location")).toBe(false);
    expect(unavailable.headers.get("Cache-Control")).toBe("no-store");
    const redirect = downloadRedirect("https://github.com/file", "github", true);
    expect(redirect.status).toBe(302);
    expect(redirect.headers.get("X-Download-Metadata")).toBe("stale");
  });

  test("latest selection excludes drafts and previews, searches by exact asset", () => {
    const release = (tag: string, draft = false, prerelease = false): DownloadRelease => ({
      tag_name: tag, draft, prerelease,
      assets: [{ name: "app.zip", size: 12, browser_download_url: "https://github.com/file" }],
    });
    expect(latestWithAsset([release("draft", true), release("rc", false, true),
      release("stable")], "app.zip")?.tag_name).toBe("stable");
    expect(latestWithAsset([release("stable")], "other.zip")).toBeNull();
  });

  test("GitHub URL validation accepts repository casing but keeps strict origin and path boundaries", () => {
    expect(isGitHubDownloadUrl("https://github.com/Owner/Repo/releases/download/V1/App.ZIP", "owner/repo")).toBe(true);
    expect(isGitHubDownloadUrl("https://github.com/owner/repo/releases/download/Preview%2FV1/App%20ARM.zip", "Owner/Repo")).toBe(true);
    for (const url of [
      "http://github.com/owner/repo/releases/download/v1/app.zip",
      "https://github.com.evil.test/owner/repo/releases/download/v1/app.zip",
      "https://github.com:444/owner/repo/releases/download/v1/app.zip",
      "https://user:pass@github.com/owner/repo/releases/download/v1/app.zip",
      "https://github.com/other/repo/releases/download/v1/app.zip",
      "https://github.com/owner/repo-extra/releases/download/v1/app.zip",
      "https://github.com/owner/repo/Releases/download/v1/app.zip",
      "https://github.com/owner/repo/releases/tag/v1/app.zip",
      "https://github.com/owner/repo/releases/download/v1/",
      "https://github.com/owner/repo/releases/download//app.zip",
      "https://github.com/owner/repo/releases/download/v1/app.zip/extra",
      "not a URL",
    ]) expect(isGitHubDownloadUrl(url, "owner/repo")).toBe(false);
  });

  test("size validation never accepts absent, malformed or mismatched length", () => {
    for (const length of [null, "", "-1", "1e2", "12.0", "13"]) {
      expect(matchesAssetSize(new Response(null, { headers: length === null ? {} :
        { "content-length": length } }), 12)).toBe(false);
    }
    expect(matchesAssetSize(new Response(), 0)).toBe(false);
    expect(matchesAssetSize(new Response(null, { headers: { "content-length": "12" } }), 12)).toBe(true);
    expect(matchesAssetSize(new Response(null, { status: 404, headers: { "content-length": "12" } }), 12)).toBe(false);
  });

  test("only transient HTTP failures qualify for stale metadata", () => {
    for (const status of [429, 500, 502, 503, 504]) expect(transientGitHubStatus(status, new Headers())).toBe(true);
    for (const status of [400, 401, 403, 404, 422]) expect(transientGitHubStatus(status, new Headers())).toBe(false);
    expect(transientGitHubStatus(403, new Headers({ "x-ratelimit-remaining": "0" }))).toBe(true);
  });
});

describe("bounded metadata cache", () => {
  test("coalesces loads; bounded stale age is not renewed by outages", async () => {
    let now = 0;
    let calls = 0;
    const cache = new DownloadLookupCache<number>(10, 30, 2, () => now);
    const load = async () => ++calls;
    expect(await Promise.all([cache.lookup("a", load), cache.lookup("a", load)])).toEqual([
      { value: 1, stale: false }, { value: 1, stale: false },
    ]);
    now = 11;
    expect(await cache.lookup("a", temporary)).toEqual({ value: 1, stale: true });
    now = 29;
    expect((await cache.lookup("a", temporary)).stale).toBe(true);
    now = 30;
    await expect(cache.lookup("a", temporary)).rejects.toThrow("offline");
    expect(calls).toBe(1);
  });

  test("authoritative missing/draft data replaces old data; permanent failure evicts", async () => {
    let now = 0;
    const cache = new DownloadLookupCache<string>(10, 30, 2, () => now);
    await cache.lookup("a", async () => "published");
    now = 11;
    expect((await cache.lookup("a", async () => "draft")).value).toBe("draft");
    now = 22;
    await expect(cache.lookup("a", async () => { throw new DownloadLookupError("401", false); })).rejects.toThrow("401");
    await expect(cache.lookup("a", temporary)).rejects.toThrow("offline");
  });

  test("capacity eviction and independent invalidated instance", async () => {
    const cache = new DownloadLookupCache<number>(10, 30, 1, () => 0);
    await cache.lookup("a", async () => 1);
    await cache.lookup("b", async () => 2);
    await expect(cache.lookup("a", temporary)).rejects.toThrow();
    const fresh = new DownloadLookupCache<number>();
    await expect(fresh.lookup("b", temporary)).rejects.toThrow();
  });
});

describe("OSS HEAD cache", () => {
  test("coalesces concurrent HEAD; size is part of key; TTL and capacity bounded", async () => {
    let now = 0;
    let calls = 0;
    const request = (async (_input, init) => {
      calls++;
      expect(init?.method).toBe("HEAD");
      expect(init?.signal).toBeInstanceOf(AbortSignal);
      return new Response(null, { headers: { "content-length": "12" } });
    }) as typeof fetch;
    const probe = createOssProbe((key) => key, request, () => now, 2);
    expect(await Promise.all([probe("a", 12), probe("a", 12)])).toEqual([true, true]);
    expect(calls).toBe(1);
    expect(await probe("a", 13)).toBe(false);
    expect(calls).toBe(2);
    await probe("a", 13);
    expect(calls).toBe(2);
    now = 60_000;
    await probe("a", 13);
    expect(calls).toBe(3);
    await probe("b", 12);
    await probe("a", 12);
    expect(calls).toBe(5);
    now += 3_600_000;
    await probe("a", 12);
    expect(calls).toBe(6);
  });

  test("failure is negatively cached; saturation never launches extra HEAD", async () => {
    let calls = 0;
    let finish!: (value: Response) => void;
    const probe = createOssProbe((key) => key, (async () => {
      calls++;
      return new Promise<Response>((resolve) => { finish = resolve; });
    }) as typeof fetch, () => 0, 1);
    const first = probe("a", 12);
    await Promise.resolve();
    expect(await probe("b", 12)).toBe(false);
    finish(new Response(null, { status: 503 }));
    expect(await first).toBe(false);
    expect(await probe("a", 12)).toBe(false);
    expect(calls).toBe(1);
    const failed = createOssProbe(() => { throw new Error("signing failed"); });
    expect(await failed("a", 12)).toBe(false);
  });
});
