import { afterAll, beforeEach, expect, mock, test } from "bun:test";
import { bustApiCaches } from "../src/lib/api-cache";

let configured = true;
let cdnConfigured = false;
let cnConfigured = false;
const signatures: string[] = [];
mock.module("astro:env/server", () => ({ GITHUB_TOKEN: "", GITHUB_REPO: "owner/repo" }));
const ossMock = () => ({
  ossConfigured: configured,
  cnCdnConfigured: cnConfigured,
  cdnDownloadUrl: (key: string, tag: string) => cdnConfigured
    ? `https://cdn.example/${key}?fallback=github&tag=${encodeURIComponent(tag)}` : null,
  cnCdnDownloadUrl: (key: string) => {
    signatures.push("CN");
    return `https://cn.example/${key}?auth_key=signed`;
  },
  releaseObjectKey: (tag: string, name: string) => `${tag}/${name}`,
  presignOssUrl: (method: string, key: string) => {
    signatures.push(method);
    return `https://oss.example/${key}`;
  },
  ossDownloadUrl: (key: string) => {
    signatures.push("GET");
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
  configured = true;
  cdnConfigured = false;
  cnConfigured = false;
  mock.module("../src/lib/oss", ossMock);
  draft = false;
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

async function get(source = "", tag = `v${sequence}`, filename = "app.zip", country = "") {
  const url = new URL(`https://site.example/api/download/${filename}`);
  if (source) url.searchParams.set("source", source);
  if (tag) url.searchParams.set("tag", tag);
  const request = new Request(url, { headers: country ? { "cf-ipcountry": country } : {} });
  return GET({ params: { filename }, url, request } as Parameters<typeof GET>[0]);
}

test("github never signs or probes OSS", async () => {
  const response = await get("github");
  expect(response.status).toBe(302);
  expect(response.headers.get("X-Download-Source")).toBe("github");
  expect(signatures).toEqual([]);
  expect(requests.length).toBe(1);
});

test("auto uses CDN without HEAD or signing even when website OSS credentials are absent", async () => {
  cdnConfigured = true;
  configured = false;
  mock.module("../src/lib/oss", ossMock);
  headSize = "0";
  const response = await get("auto");
  expect(response.status).toBe(302);
  expect(response.headers.get("X-Download-Source")).toBe("cdn");
  expect(response.headers.get("Location")).toBe(
    `https://cdn.example/v${sequence}/app.zip?fallback=github&tag=v${sequence}`,
  );
  expect(signatures).toEqual([]);
  expect(requests.length).toBe(1);
});

test("auto without CDN retains verified OSS then GitHub fallback", async () => {
  expect((await get("auto")).headers.get("X-Download-Source")).toBe("oss");
  expect(signatures).toEqual(["HEAD", "GET"]);
  sequence++;
  headSize = "11";
  expect((await get("auto")).headers.get("X-Download-Source")).toBe("github");
});

test("auto CDN still requires published exact GitHub metadata", async () => {
  cdnConfigured = true;
  draft = true;
  expect((await get("auto")).status).toBe(403);
  bustApiCaches();
  draft = false;
  expect((await get("auto", `v${sequence}`, "unknown.zip")).status).toBe(404);
  bustApiCaches();
  status = 503;
  expect((await get("auto")).status).toBe(503);
  expect(signatures).toEqual([]);
});

test("mainland visitors get a verified OSS asset through the signed Aliyun CDN, else GitHub", async () => {
  cdnConfigured = true;
  cnConfigured = true;
  mock.module("../src/lib/oss", ossMock);
  for (const source of ["auto", ""]) {
    const response = await get(source, `v${sequence}`, "app.zip", "CN");
    expect(response.headers.get("X-Download-Source")).toBe("cn-cdn");
    expect(response.headers.get("Location")).toBe(`https://cn.example/v${sequence}/app.zip?auth_key=signed`);
  }
  // HEAD 结果缓存：同一资产只探测一次。
  expect(signatures).toEqual(["HEAD", "CN", "CN"]);
  expect((await get("auto", `v${sequence}`, "app.zip", "HK")).headers.get("X-Download-Source")).toBe("cdn");
  sequence++;
  headSize = "11";
  signatures.length = 0;
  const missing = await get("auto", `v${sequence}`, "app.zip", "CN");
  expect(missing.headers.get("X-Download-Source")).toBe("github");
  expect(signatures).toEqual(["HEAD"]);
});

test("mainland routing is off until the Aliyun CDN key is configured", async () => {
  cdnConfigured = true;
  mock.module("../src/lib/oss", ossMock);
  const response = await get("auto", `v${sequence}`, "app.zip", "CN");
  expect(response.headers.get("X-Download-Source")).toBe("cdn");
  expect(signatures).toEqual([]);
});

test("legacy prefers verified OSS and forced OSS also works", async () => {
  expect((await get()).headers.get("X-Download-Source")).toBe("oss");
  expect((await get("oss")).headers.get("X-Download-Source")).toBe("oss");
  expect(signatures).toEqual(["HEAD", "GET", "GET"]);
});

test("size mismatch: legacy falls back, forced OSS returns 503 without Location", async () => {
  headSize = "11";
  expect((await get()).headers.get("X-Download-Source")).toBe("github");
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
