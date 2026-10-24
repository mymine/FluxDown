import { afterAll, beforeEach, expect, mock, test } from "bun:test";
import { bustApiCaches } from "../src/lib/api-cache";
import { markReleaseViewer, resetReleaseViewers } from "../src/lib/download-gate";
import { resetDownloadBans } from "../src/lib/download-ban";

let configured = true;
let cdnConfigured = true;
const MAINLAND_IP = "114.114.114.114";
const signatures: string[] = [];
mock.module("astro:env/server", () => ({ GITHUB_TOKEN: "", GITHUB_REPO: "owner/repo" }));
// mock.module 在 bun test 进程内全局生效且不会被 mock.restore 撤销：必须保留真实导出，否则后运行的 geoip.test.ts 取不到 Ip2RegionV4。
const realGeoip = await import("../src/lib/geoip");
mock.module("../src/lib/geoip", () => ({ ...realGeoip, isMainlandChinaIp: (ip: string) => ip === MAINLAND_IP }));
const ossMock = () => ({
  ossConfigured: configured,
  cdnConfigured,
  cdnDownloadUrl: (key: string) => {
    signatures.push("CDN");
    return `https://cdn.example/${key}?auth_key=signed`;
  },
  releaseObjectKey: (tag: string, name: string) => `${tag}/${name}`,
  presignOssHeadUrl: (key: string) => {
    signatures.push("HEAD");
    return `https://oss.example/${key}`;
  },
});
mock.module("../src/lib/oss", ossMock);
const { GET } = await import("../src/pages/api/download/[filename]");
const originalFetch = globalThis.fetch;
let draft = false;
let prerelease = false;
let status = 200;
let headSize = "12";
let requests: string[] = [];
let sequence = 0;

beforeEach(() => {
  bustApiCaches();
  resetReleaseViewers();
  resetDownloadBans();
  markReleaseViewer(MAINLAND_IP);
  configured = true;
  cdnConfigured = true;
  mock.module("../src/lib/oss", ossMock);
  prerelease = false;
  status = 200;
  headSize = "12";
  signatures.length = 0;
  requests = [];
  sequence++;
  globalThis.fetch = (async (input, init) => {
    const url = String(input);
    requests.push(url);
    if (init?.method === "HEAD") return new Response(null, { headers: { "content-length": headSize } });
    if (status !== 200) return new Response(null, { status });
    const release = { tag_name: `v${sequence}`, draft, prerelease, assets: [{
      name: "app.zip", size: 12,
      browser_download_url: `https://github.com/owner/repo/releases/download/v${sequence}/app.zip`,
    }] };
    return Response.json(url.includes("/tags/") ? release : [release]);
  }) as typeof fetch;
});
afterAll(() => { globalThis.fetch = originalFetch; mock.restore(); });

/** 缺省模拟读过发布清单的大陆访客；clientIp 传空串时不带 X-Client-IP（按 TCP 对端 10.0.0.1）。 */
async function get(source = "", tag = `v${sequence}`, filename = "app.zip", clientIp = MAINLAND_IP, userAgent = "") {
  const url = new URL(`https://site.example/api/download/${filename}`);
  if (source) url.searchParams.set("source", source);
  if (tag) url.searchParams.set("tag", tag);
  const headers: Record<string, string> = {};
  if (clientIp) headers["x-client-ip"] = clientIp;
  if (userAgent) headers["user-agent"] = userAgent;
  const request = new Request(url, { headers });
  return GET({ params: { filename }, url, request, clientAddress: "10.0.0.1" } as Parameters<typeof GET>[0]);
}

test("github never signs or probes OSS, even for mainland visitors", async () => {
  const response = await get("github", `v${sequence}`, "app.zip", MAINLAND_IP);
  expect(response.status).toBe(302);
  expect(response.headers.get("X-Download-Source")).toBe("github");
  expect(signatures).toEqual([]);
  expect(requests.length).toBe(1);
});

test("visitors outside mainland China go straight to GitHub without probing", async () => {
  for (const source of ["auto", "", "unknown"]) {
    const response = await get(source, `v${sequence}`, "app.zip", "8.8.8.8");
    expect(response.headers.get("X-Download-Source")).toBe("github");
  }
  // 无 X-Client-IP 时按 TCP 对端判定，同样不是大陆。
  expect((await get("auto", `v${sequence}`, "app.zip", "")).headers.get("X-Download-Source")).toBe("github");
  expect(signatures).toEqual([]);
});

test("mainland visitors get a verified OSS asset through the signed CDN, else GitHub", async () => {
  for (const source of ["auto", ""]) {
    const response = await get(source, `v${sequence}`, "app.zip", MAINLAND_IP);
    expect(response.headers.get("X-Download-Source")).toBe("cdn");
    expect(response.headers.get("Location")).toBe(`https://cdn.example/v${sequence}/app.zip?auth_key=signed`);
  }
  // HEAD 结果缓存：同一资产只探测一次。
  expect(signatures).toEqual(["HEAD", "CDN", "CDN"]);
  sequence++;
  headSize = "11";
  signatures.length = 0;
  const missing = await get("auto", `v${sequence}`, "app.zip", MAINLAND_IP);
  expect(missing.headers.get("X-Download-Source")).toBe("github");
  expect(signatures).toEqual(["HEAD"]);
});

test("CDN routing is off until both the CDN key and OSS credentials are configured", async () => {
  for (const [cdn, oss] of [[false, true], [true, false]]) {
    cdnConfigured = cdn;
    configured = oss;
    mock.module("../src/lib/oss", ossMock);
    bustApiCaches();
    expect((await get("auto", `v${sequence}`, "app.zip", MAINLAND_IP)).headers.get("X-Download-Source")).toBe("github");
    expect((await get("oss")).status).toBe(503);
  }
  expect(signatures).toEqual([]);
});

test("forced OSS outside mainland China goes to GitHub instead of the paid CDN", async () => {
  const response = await get("oss", `v${sequence}`, "app.zip", "8.8.8.8");
  expect(response.headers.get("X-Download-Source")).toBe("github");
  expect(signatures).toEqual([]);
});

test("an IP over the download limit gets 429 without touching GitHub or OSS", async () => {
  for (let i = 0; i < 20; i++) {
    expect((await get("github", `v${sequence}`, `file-${i}.zip`)).status).not.toBe(429);
  }
  requests = [];
  const banned = await get("auto");
  expect(banned.status).toBe(429);
  expect(Number(banned.headers.get("Retry-After"))).toBe(12 * 3600);
  expect(requests).toEqual([]);
  expect(signatures).toEqual([]);
  // 其他 IP 不受影响。
  expect((await get("auto", `v${sequence}`, "app.zip", "8.8.8.8")).status).toBe(302);
});

test("mainland requests that never read the release manifest or come from scripts go to GitHub", async () => {
  resetReleaseViewers();
  for (const source of ["auto", "", "oss"]) {
    expect((await get(source)).headers.get("X-Download-Source")).toBe("github");
  }
  markReleaseViewer(MAINLAND_IP);
  for (const ua of ["curl/8.7.1", "python-requests/2.32", "Go-http-client/2.0", "Wget/1.21"]) {
    expect((await get("auto", `v${sequence}`, "app.zip", MAINLAND_IP, ua)).headers.get("X-Download-Source")).toBe("github");
  }
  expect(signatures).toEqual([]);
  // 应用内更新（含不带 User-Agent 的旧客户端）与浏览器照常走 CDN。
  for (const ua of ["fluxdown-agent/0.5.9", "fluxdown-android/0.5.9", "", "Mozilla/5.0 (Windows NT 10.0)"]) {
    expect((await get("", `v${sequence}`, "app.zip", MAINLAND_IP, ua)).headers.get("X-Download-Source")).toBe("cdn");
  }
});

test("reading the manifest admits the visitor for 24 hours", async () => {
  const originalNow = Date.now;
  let now = originalNow();
  Date.now = () => now;
  try {
    resetReleaseViewers();
    markReleaseViewer(MAINLAND_IP, now);
    now += 24 * 3600 * 1000 - 1;
    expect((await get("auto")).headers.get("X-Download-Source")).toBe("cdn");
    now += 2;
    bustApiCaches();
    expect((await get("auto")).headers.get("X-Download-Source")).toBe("github");
  } finally {
    Date.now = originalNow;
  }
});

test("size mismatch: mainland falls back to GitHub, forced OSS returns 503 without Location", async () => {
  headSize = "11";
  expect((await get("")).headers.get("X-Download-Source")).toBe("github");
  const response = await get("oss");
  expect(response.status).toBe(503);
  expect(response.headers.has("Location")).toBe(false);
  expect(signatures).toEqual(["HEAD"]);
});

test("unconfigured OSS is explicit 503", async () => {
  configured = false;
  mock.module("../src/lib/oss", ossMock);
  expect((await get("oss")).status).toBe(503);
  expect(signatures).toEqual([]);
});

test("draft and missing assets never sign; explicit previews remain supported", async () => {
  draft = true;
  expect((await get("oss")).status).toBe(403);
  expect(signatures).toEqual([]);
  bustApiCaches();
  draft = false;
  prerelease = true;
  expect((await get("github")).status).toBe(302);
  expect((await get("oss", `v${sequence}`, "unknown.zip")).status).toBe(404);
  expect((await get("github", "")).status).toBe(404);
  expect(signatures).toEqual([]);
});

test("stale metadata only redirects to GitHub; webhook invalidation removes fallback", async () => {
  const originalNow = Date.now;
  let now = originalNow();
  Date.now = () => now;
  try {
    expect((await get("github")).status).toBe(302);
    now += 120_001;
    cdnConfigured = true;
    status = 503;
    const fallback = await get();
    expect(fallback.status).toBe(302);
    expect(fallback.headers.get("X-Download-Source")).toBe("github");
    expect(fallback.headers.get("X-Download-Metadata")).toBe("stale");
    expect((await get("auto")).headers.get("X-Download-Source")).toBe("github");
    expect((await get("oss")).status).toBe(503);
    expect(signatures).toEqual([]);
    bustApiCaches();
    expect((await get("github")).status).toBe(503);
  } finally {
    Date.now = originalNow;
  }
});

for (const failure of [
  new DOMException("body timed out", "TimeoutError"),
  new DOMException("body aborted", "AbortError"),
  new TypeError("terminated"),
]) {
  test(`response body ${failure.name} preserves stale GitHub fallback without OSS signing`, async () => {
    const originalNow = Date.now;
    let now = originalNow();
    Date.now = () => now;
    try {
      expect((await get("github")).status).toBe(302);
      now += 120_001;
      globalThis.fetch = (async () => new Response(new ReadableStream({
        start(controller) { controller.error(failure); },
      }))) as typeof fetch;
      const fallback = await get();
      expect(fallback.status).toBe(302);
      expect(fallback.headers.get("X-Download-Source")).toBe("github");
      expect(fallback.headers.get("X-Download-Metadata")).toBe("stale");
      expect((await get("oss")).status).toBe(503);
      expect(signatures).toEqual([]);
      globalThis.fetch = (async () => { throw new TypeError("network down"); }) as typeof fetch;
      expect((await get("github")).status).toBe(302);
      now += 480_000;
      expect((await get("github")).status).toBe(503);
    } finally {
      Date.now = originalNow;
    }
  });
}

for (const body of ["{broken JSON", JSON.stringify({ tag_name: "invalid" })]) {
  test(`complete invalid response does not use stale metadata: ${body}`, async () => {
    const originalNow = Date.now;
    let now = originalNow();
    Date.now = () => now;
    try {
      expect((await get("github")).status).toBe(302);
      now += 120_001;
      globalThis.fetch = (async () => new Response(body)) as typeof fetch;
      expect((await get("github")).status).toBe(500);
      globalThis.fetch = (async () => { throw new TypeError("network down"); }) as typeof fetch;
      expect((await get("github")).status).toBe(503);
      expect(signatures).toEqual([]);
    } finally {
      Date.now = originalNow;
    }
  });
}

test("canonical repository casing works for tag and latest without changing tag or filename casing", async () => {
  const tag = `V${sequence}`;
  const filename = "App.ZIP";
  const location = `https://github.com/Owner/Repo/releases/download/${tag}/${filename}`;
  globalThis.fetch = (async (input) => {
    const release = { tag_name: tag, draft: false, prerelease: false, assets: [{
      name: filename, size: 12, browser_download_url: location,
    }] };
    return Response.json(String(input).includes("/tags/") ? release : [release]);
  }) as typeof fetch;
  for (const requestedTag of [tag, ""]) {
    const response = await get("github", requestedTag, filename);
    expect(response.status).toBe(302);
    expect(response.headers.get("Location")).toBe(location);
  }
  expect((await get("github", tag, "app.zip")).status).toBe(404);
  expect((await get("github", tag.toLowerCase(), filename)).status).toBe(500);
  expect(signatures).toEqual([]);
});

test("404 remains authoritative and cold API outages cannot produce signatures", async () => {
  status = 404;
  expect((await get("oss")).status).toBe(404);
  bustApiCaches();
  status = 503;
  expect((await get("github")).status).toBe(503);
  expect((await get("oss")).status).toBe(503);
  expect(signatures).toEqual([]);
});
