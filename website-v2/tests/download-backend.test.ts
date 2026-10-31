import { describe, expect, test } from "bun:test";
import { DownloadLookupCache } from "../src/lib/download-cache";
import { createOssProbe } from "../src/lib/download-probe";
import { createPan123Probe, signDirectLink, webdavContentLength } from "../src/lib/download-pan123";
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

describe("123pan direct link source", () => {
  const propfind = (size: string) => new Response(
    `<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response><D:prop>` +
      `<D:getcontentlength>${size}</D:getcontentlength></D:prop></D:response></D:multistatus>`,
    { status: 207 },
  );

  test("signature matches 123's documented example; expiry is issue time + TTL", () => {
    // 123「直链鉴权开发指引」示例：密钥 289ds32418bxdba，过期 1689220731，rand 123，UID 13。
    const url = signDirectLink(
      { base: "http://13.cdn.123clouddisk.com/", uid: "13", folder: "files", authKey: "289ds32418bxdba" },
      "1.txt", 60, (1689220731 - 60) * 1000, 123,
    );
    expect(url).toBe("http://13.cdn.123clouddisk.com/13/files/1.txt?auth_key=1689220731-123-13-3bdacc0e031fd67fe829152f37c8fbad");
    // URL 用按段编码的路径，签名用解码后的路径（123 按解码路径校验）。
    const spaced = new URL(signDirectLink({ base: "https://x", uid: "1", folder: "fd", authKey: "k" }, "v1/a b+c.zip", 1, 0, 1));
    expect(spaced.pathname).toBe("/1/fd/v1/a%20b%2Bc.zip");
    const hash = new Bun.CryptoHasher("md5").update("/1/fd/v1/a b+c.zip-1-1-1-k").digest("hex");
    expect(spaced.searchParams.get("auth_key")).toBe(`1-1-1-${hash}`);
    expect(webdavContentLength("<d:getcontentlength>42</d:getcontentlength>")).toBe(42);
    expect(webdavContentLength("<D:getcontentlength></D:getcontentlength>")).toBeNull();
  });

  test("probe checks size via PROPFIND and caches presence 1h, absence 60s", async () => {
    let now = 0;
    const calls: string[] = [];
    let size = "12";
    const request = (async (input, init) => {
      calls.push(`${init?.method} ${input}`);
      expect((init?.headers as Record<string, string>).Authorization).toBe("Basic x");
      return propfind(size);
    }) as typeof fetch;
    const probe = createPan123Probe("https://dav.example/webdav/", "Basic x", request, () => now, 4);
    expect(await Promise.all([probe("R/v1/a b.zip", 12), probe("R/v1/a b.zip", 12)])).toEqual([true, true]);
    expect(calls).toEqual(["PROPFIND https://dav.example/webdav/R/v1/a%20b.zip"]);
    expect(await probe("R/v1/a b.zip", 13)).toBe(false);
    now = 3_600_000 - 1;
    expect(await probe("R/v1/a b.zip", 12)).toBe(true);
    expect(await probe("R/v1/a b.zip", 13)).toBe(false);
    expect(calls.length).toBe(3);
    size = "13";
    now = 3_600_000;
    expect(await probe("R/v1/a b.zip", 12)).toBe(false);
    expect(calls.length).toBe(4);
  });

  test("missing assets and failures are unavailable; saturation never adds requests", async () => {
    for (const respond of [
      () => new Response("Not Found", { status: 404 }),
      () => new Response("<x/>", { status: 207 }),
      () => { throw new TypeError("network down"); },
    ]) {
      expect(await createPan123Probe("https://dav.example", "Basic x", (async () => respond()) as typeof fetch)("a", 12))
        .toBe(false);
    }
    let calls = 0;
    const saturated = createPan123Probe("https://dav.example", "Basic x",
      (() => { calls++; return new Promise<Response>(() => {}); }) as typeof fetch, () => 0, 1);
    void saturated("a", 12);
    await Promise.resolve();
    expect(await saturated("b", 12)).toBe(false);
    expect(await saturated("c", -1)).toBe(false);
    expect(calls).toBe(1);
  });
});
