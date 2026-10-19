import { describe, expect, test } from "bun:test";
import { DownloadLookupCache } from "../src/lib/download-cache";
import { createOssProbe } from "../src/lib/download-probe";
import worker from "../cdn/oss-proxy.js";
import { downloadSignature } from "../src/lib/download-signature";
import {
  DownloadLookupError, downloadSource, latestWithAsset, matchesAssetSize,
  mayProbeOss, ossUnavailable, downloadRedirect, transientGitHubStatus, isGitHubDownloadUrl,
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
    for (const stale of [false, true]) {
      expect(mayProbeOss("github", true, stale)).toBe(false);
    }
    expect(mayProbeOss("legacy", true, false)).toBe(true);
    expect(mayProbeOss("oss", false, false)).toBe(false);
    expect(mayProbeOss("oss", true, true)).toBe(false);
    expect(mayProbeOss("legacy", true, true)).toBe(false);
    expect(mayProbeOss("auto", true, false)).toBe(true);
    expect(mayProbeOss("auto", true, true)).toBe(false);
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

describe("CDN Worker download consumer", () => {
  const env = {
    OSS_ACCESS_KEY_ID: "test-id", OSS_ACCESS_KEY_SECRET: "test-secret",
    OSS_BUCKET: "test-bucket", OSS_ENDPOINT: "oss.example",
    OSS_RELEASE_PREFIX: "releases", GITHUB_REPO: "owner/repo",
  };
  const request = (path = "v1.2.3/app/App%20ARM.zip", query = "fallback=github&tag=v1.2.3", init?: RequestInit) =>
    new Request(`https://cdn.example/releases/${path}?${query}`, init);

  async function withOrigin(origin: typeof fetch, run: () => Promise<void>) {
    const previous = globalThis.fetch;
    globalThis.fetch = origin;
    try { await run(); } finally { globalThis.fetch = previous; }
  }

  test("cache hits stream bytes; Range and conditional Range reach a stable private origin URL", async () => {
    const origins: string[] = [];
    await withOrigin((async (input, init) => {
      origins.push(String(input));
      expect(init?.method).toBe("GET");
      expect(init?.redirect).toBe("manual");
      expect(new Headers(init?.headers).get("range")).toBe("bytes=0-2");
      expect(new Headers(init?.headers).get("if-range")).toBe('"asset"');
      expect(init?.signal).toBeInstanceOf(AbortSignal);
      expect((init as RequestInit & { cf: { cacheTtlByStatus: Record<string, number> } })
        .cf.cacheTtlByStatus["300-599"]).toBe(-1);
      return new Response("zip", { status: 206, headers: {
        "content-length": "3", "content-range": "bytes 0-2/12", "cf-cache-status": "HIT",
      } });
    }) as typeof fetch, async () => {
      for (const query of ["fallback=github&tag=v1.2.3", ""]) {
        const response = await worker.fetch(request(undefined, query, {
          headers: { range: "bytes=0-2", "if-range": '"asset"' },
        }), env);
        expect(response.status).toBe(206);
        expect(await response.text()).toBe("zip");
        expect(response.headers.get("x-edge-cache")).toBe("HIT");
        expect(response.headers.get("content-range")).toBe("bytes 0-2/12");
        expect(response.headers.has("location")).toBe(false);
        expect(response.headers.get("cache-control")).toContain("immutable");
      }
      expect(origins[0]).toBe(origins[1]);
      expect(new URL(origins[0]).searchParams.has("Signature")).toBe(true);
    });
  });

  test("APK is fetched from its .bin storage key but served under the original name and type", async () => {
    const origins: string[] = [];
    await withOrigin((async (input) => {
      origins.push(String(input));
      return new Response("apk", { status: 200, headers: {
        "content-type": "application/octet-stream",
        "content-disposition": 'attachment; filename="App.apk.bin"',
      } });
    }) as typeof fetch, async () => {
      const response = await worker.fetch(request("v1.2.3/app/App-arm64.apk"), env);
      expect(response.status).toBe(200);
      expect(await response.text()).toBe("apk");
      expect(response.headers.get("content-type")).toBe("application/vnd.android.package-archive");
      expect(response.headers.get("content-disposition")).toBe(
        `attachment; filename="App-arm64.apk"; filename*=UTF-8''App-arm64.apk`,
      );
      const zip = await worker.fetch(request(), env);
      expect(zip.headers.get("content-type")).toBe("application/octet-stream");
    });
    expect(new URL(origins[0]).pathname).toBe("/releases/v1.2.3/app/App-arm64.apk.bin");
    expect(new URL(origins[1]).pathname).toBe("/releases/v1.2.3/app/App%20ARM.zip");
  });

  for (const status of [301, 403, 404, 429, 500, 502]) {
    test(`HTTP ${status} cancels OSS body and redirects exact GitHub asset without caching`, async () => {
      let cancelled = false;
      await withOrigin((async () => new Response(new ReadableStream({
        cancel() { cancelled = true; },
      }), { status, headers: { location: "https://private.example/?Signature=secret" } })) as typeof fetch,
      async () => {
        const response = await worker.fetch(request(), env);
        expect(response.status).toBe(302);
        expect(response.headers.get("location")).toBe(
          "https://github.com/owner/repo/releases/download/v1.2.3/App%20ARM.zip",
        );
        expect(response.headers.get("cache-control")).toBe("no-store");
        expect(await response.text()).toBe("");
        expect(cancelled).toBe(true);
      });
    });
  }

  test("network failure maps unified and historical component tags exactly", async () => {
    await withOrigin((async () => { throw new TypeError("offline"); }) as typeof fetch, async () => {
      for (const [component, tag] of [
        ["app", "v1.2.3-rc.1"], ["app", "app-v1.2.3-rc.1"],
        ["extension", "extension-v1.2.3-rc.1"], ["server", "server-v1.2.3-rc.1"],
        ["cli", "cli-v1.2.3-rc.1"], ["mobile", "mobile-v1.2.3-rc.1"],
      ]) {
        const response = await worker.fetch(request(`v1.2.3-rc.1/${component}/file.zip`,
          `fallback=github&tag=${tag}`), env);
        expect(response.status).toBe(302);
        expect(response.headers.get("location")).toBe(
          `https://github.com/owner/repo/releases/download/${tag}/file.zip`,
        );
        expect(response.headers.get("cache-control")).toBe("no-store");
      }
      const legacy = await worker.fetch(request(undefined, ""), env);
      expect(legacy.status).toBe(502);
      expect(legacy.headers.has("location")).toBe(false);
    });
  });

  test("416 is not a source failure and HEAD releases upstream bodies", async () => {
    let cancelled = 0;
    let status = 416;
    await withOrigin((async () => new Response(new ReadableStream({
      cancel() { cancelled++; },
    }), { status, headers: { "content-range": "bytes */12", "content-length": "12" } })) as typeof fetch,
    async () => {
      const invalidRange = await worker.fetch(request(undefined, undefined, {
        headers: { range: "bytes=99-" },
      }), env);
      expect(invalidRange.status).toBe(416);
      expect(invalidRange.headers.get("content-range")).toBe("bytes */12");
      expect(invalidRange.headers.get("cache-control")).toBe("no-store");
      expect(invalidRange.headers.has("location")).toBe(false);
      status = 200;
      const head = await worker.fetch(request(undefined, undefined, { method: "HEAD" }), env);
      expect(head.status).toBe(200);
      expect(head.headers.get("content-length")).toBe("12");
      expect(await head.text()).toBe("");
      expect(cancelled).toBe(2);
    });
  });

  test("invalid paths, mismatched tags and external fallback targets never reach OSS or redirect", async () => {
    let calls = 0;
    await withOrigin((async () => { calls++; return new Response(null, { status: 404 }); }) as typeof fetch,
    async () => {
      for (const [path, query] of [
        ["v1.2.3/app/file.zip", "fallback=github&tag=extension-v1.2.3"],
        ["v1.2.3/extension/file.zip", "fallback=github&tag=v1.2.3"],
        ["v1.2.3/app/file.zip", "fallback=github&tag=v9.9.9"],
        ["v1.2.3/app/file.zip", "fallback=https://evil.example&tag=v1.2.3"],
        ["v1.2.3/app/file.zip", "fallback=github&tag=https://evil.example"],
        ["other/file.zip", ""], ["v1.2.3/app/sub/file.zip", ""],
        ["v1.2.3/app/file%5C.zip", ""], ["v1.2.3/app/file%00.zip", ""],
        ["v1.2.3/app/%ZZ", ""],
      ]) {
        const response = await worker.fetch(request(path, query), env);
        expect([400, 404]).toContain(response.status);
        expect(response.headers.has("location")).toBe(false);
      }
      expect((await worker.fetch(request(undefined, undefined, { method: "POST" }), env)).status).toBe(405);
      expect(calls).toBe(0);
    });
  });

  test("with a signing secret only unexpired links signed for the exact path reach OSS", async () => {
    const signed = { ...env, DOWNLOAD_SIGNING_SECRET: "s3cret" };
    const path = "/releases/v1.2.3/app/App%20ARM.zip";
    const now = Math.floor(Date.now() / 1000);
    let calls = 0;
    await withOrigin((async () => { calls++; return new Response("zip"); }) as typeof fetch, async () => {
      const sig = await downloadSignature("s3cret", path, now + 600);
      const ok = await worker.fetch(request(undefined, `exp=${now + 600}&sig=${sig}`), signed);
      expect(ok.status).toBe(200);
      expect(calls).toBe(1);
      const expired = await downloadSignature("s3cret", path, now - 1);
      const tooLong = await downloadSignature("s3cret", path, now + 25 * 3600);
      for (const [target, query] of [
        [undefined, ""],
        [undefined, "fallback=github&tag=v1.2.3"],
        [undefined, `exp=${now - 1}&sig=${expired}`],
        [undefined, `exp=${now + 25 * 3600}&sig=${tooLong}`],
        [undefined, `exp=${now + 601}&sig=${sig}`],
        ["v1.2.3/app/Other.zip", `exp=${now + 600}&sig=${sig}`],
      ] as const) {
        const response = await worker.fetch(request(target, query), signed);
        expect(response.status).toBe(403);
        expect(response.headers.has("location")).toBe(false);
      }
      expect(calls).toBe(1);
    });
  });
});
