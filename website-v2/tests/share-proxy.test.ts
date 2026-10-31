import { afterAll, beforeEach, expect, mock, test } from "bun:test";

let base = "http://cloud.test";
const configure = () => mock.module("astro:env/server", () => ({
  GITHUB_TOKEN: "", GITHUB_REPO: "owner/repo", FLUXCLOUD_API_BASE: base, DOWNLOAD_PASS_SECRET: undefined,
}));
configure();
// 动态 import：路由模块须在 astro:env/server 被 mock 之后才加载（静态 import 会先于 mock 求值）。
const { GET } = await import("../src/pages/api/cloud/share/public/[slug]");
const { POST } = await import("../src/pages/api/cloud/share/public/[slug]/download");
const originalFetch = globalThis.fetch;

const SLUG = "Ab3-_xYz09Qw";
const MANIFEST = {
  id: "sh_1", slug: SLUG, title: "Holiday photos",
  files: [{ index: 0, name: "a.zip", size: 1536 }, { index: 1, name: "b.txt", size: 12 }],
  totalBytes: 1548, expiresAt: "2026-10-30T00:00:00Z", cloudReady: true,
};

interface Seen { url: string; method: string; headers: Record<string, string>; body: string }
let seen: Seen[] = [];
let reply: () => Response | Promise<Response> = () => Response.json(MANIFEST);

beforeEach(() => {
  base = "http://cloud.test";
  configure();
  seen = [];
  reply = () => Response.json(MANIFEST);
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push({
      url: String(input),
      method: init?.method ?? "GET",
      headers: Object.fromEntries(new Headers(init?.headers).entries()),
      body: typeof init?.body === "string" ? init.body : "",
    });
    return reply();
  }) as typeof fetch;
});
afterAll(() => { globalThis.fetch = originalFetch; mock.restore(); });

const call = (slug: string, ip = "203.0.113.7") =>
  GET({
    params: { slug },
    request: new Request(`https://site.example/api/cloud/share/public/${slug}`, { headers: { "x-client-ip": ip } }),
    clientAddress: "10.0.0.1",
  } as unknown as Parameters<typeof GET>[0]);

const download = (slug: string, body: string, headers: Record<string, string> = {}) =>
  POST({
    params: { slug },
    request: new Request(`https://site.example/api/cloud/share/public/${slug}/download`, {
      method: "POST", body, headers: { "x-client-ip": "203.0.113.7", ...headers },
    }),
    clientAddress: "10.0.0.1",
  } as unknown as Parameters<typeof POST>[0]);

const expectPrivate = (response: Response) => {
  expect(response.headers.get("X-Robots-Tag")).toBe("noindex, nofollow");
  expect(response.headers.get("Cache-Control")).toBe("no-store");
  expect(response.headers.get("Content-Type")).toContain("application/json");
};

test("manifest: success is forwarded with the visitor ip and trimmed to known fields", async () => {
  reply = () => Response.json({ ...MANIFEST, internalNote: "secret", turnstileSiteKey: "0x4AAA" });
  const response = await call(SLUG);
  expect(response.status).toBe(200);
  expectPrivate(response);
  expect(await response.json()).toEqual({ ...MANIFEST, turnstileSiteKey: "0x4AAA" });
  expect(seen).toHaveLength(1);
  expect(seen[0].url).toBe(`http://cloud.test/api/v1/share/public/${SLUG}`);
  expect(seen[0].method).toBe("GET");
  expect(seen[0].headers["x-forwarded-for"]).toBe("203.0.113.7");
});

test("manifest: trailing slash on the configured base is ignored", async () => {
  base = "http://cloud.test///";
  configure();
  await call(SLUG);
  expect(seen[0].url).toBe(`http://cloud.test/api/v1/share/public/${SLUG}`);
});

test("manifest: FluxCloud errors pass through with the same status and code", async () => {
  for (const [status, code] of [[404, "share_not_found"], [410, "share_ended"], [410, "share_takedown"], [429, "rate_limited"]] as const) {
    reply = () => new Response(JSON.stringify({ code, message: "m" }), {
      status, headers: status === 429 ? { "Retry-After": "30" } : {},
    });
    const response = await call(SLUG);
    expect(response.status).toBe(status);
    expectPrivate(response);
    expect(await response.json()).toEqual({ code, message: "m" });
    if (status === 429) expect(response.headers.get("Retry-After")).toBe("30");
  }
});

test("manifest: non-JSON upstream errors keep their status with a stable code", async () => {
  reply = () => new Response("<html>bad gateway</html>", { status: 503 });
  const response = await call(SLUG);
  expect(response.status).toBe(503);
  expect(await response.json()).toEqual({ code: "upstream_error" });
});

test("manifest: malformed upstream success, unreachable upstream and missing config are 502", async () => {
  reply = () => Response.json({ ...MANIFEST, cloudReady: "yes" });
  let response = await call(SLUG);
  expect(response.status).toBe(502);
  expect(await response.json()).toEqual({ code: "upstream_invalid" });

  reply = () => { throw new TypeError("connect refused"); };
  response = await call(SLUG);
  expect(response.status).toBe(502);
  expect(await response.json()).toEqual({ code: "upstream_unreachable" });
  expectPrivate(response);

  base = "";
  configure();
  seen = [];
  response = await call(SLUG);
  expect(response.status).toBe(502);
  expect(await response.json()).toEqual({ code: "not_configured" });
  expect(seen).toHaveLength(0);
});

test("manifest: invalid slugs are 404 without touching FluxCloud", async () => {
  for (const slug of ["", "short", "a".repeat(13), "../../admin/x", "abc def ghij", "a%2Fb%2Fc%2Fd%2F"]) {
    const response = await call(slug);
    expect(response.status).toBe(404);
    expect(await response.json()).toEqual({ code: "share_not_found" });
    expectPrivate(response);
  }
  expect(seen).toHaveLength(0);
});

test("download: forwards fileIndex and the turnstile token, returns the signed url", async () => {
  reply = () => Response.json({ url: "https://cdn.example/f?sig=1", expiresAt: "2026-10-27T01:00:00Z", extra: 1 });
  const response = await download(SLUG, JSON.stringify({ fileIndex: 1, turnstileToken: "tok-123" }));
  expect(response.status).toBe(200);
  expectPrivate(response);
  expect(await response.json()).toEqual({ url: "https://cdn.example/f?sig=1", expiresAt: "2026-10-27T01:00:00Z" });
  expect(seen).toHaveLength(1);
  expect(seen[0].url).toBe(`http://cloud.test/api/v1/share/public/${SLUG}/download`);
  expect(seen[0].method).toBe("POST");
  expect(seen[0].headers["content-type"]).toBe("application/json");
  expect(seen[0].headers["x-forwarded-for"]).toBe("203.0.113.7");
  expect(JSON.parse(seen[0].body)).toEqual({ fileIndex: 1, turnstileToken: "tok-123" });
});

test("download: without a token only fileIndex is sent", async () => {
  reply = () => Response.json({ url: "https://cdn.example/f", expiresAt: "2026-10-27T01:00:00Z" });
  await download(SLUG, JSON.stringify({ fileIndex: 0, turnstileToken: "" }));
  expect(JSON.parse(seen[0].body)).toEqual({ fileIndex: 0 });
});

test("download: FluxCloud errors pass through (traffic, turnstile, not ready, ended, rate limit)", async () => {
  const cases = [
    [403, "traffic_exhausted"], [403, "turnstile_required"], [400, "turnstile_failed"],
    [409, "share_cloud_not_ready"], [410, "share_ended"], [404, "share_not_found"], [429, "rate_limited"],
  ] as const;
  for (const [status, code] of cases) {
    reply = () => new Response(JSON.stringify({ code, message: "m" }), { status });
    const response = await download(SLUG, JSON.stringify({ fileIndex: 0 }));
    expect(response.status).toBe(status);
    expectPrivate(response);
    expect(await response.json()).toEqual({ code, message: "m" });
  }
});

test("download: an unsafe or missing signed url from upstream is rejected", async () => {
  for (const body of [{ url: "javascript:alert(1)", expiresAt: "2026-10-27T01:00:00Z" }, { expiresAt: "2026-10-27T01:00:00Z" }]) {
    reply = () => Response.json(body);
    const response = await download(SLUG, JSON.stringify({ fileIndex: 0 }));
    expect(response.status).toBe(502);
    expect(await response.json()).toEqual({ code: "upstream_invalid" });
  }
});

test("download: bad input is rejected before FluxCloud is called", async () => {
  for (const body of [
    "", "nope", "[]", JSON.stringify({}), JSON.stringify({ fileIndex: -1 }), JSON.stringify({ fileIndex: "0" }),
    JSON.stringify({ fileIndex: 0.5 }), JSON.stringify({ fileIndex: 0, turnstileToken: { a: 1 } }),
    JSON.stringify({ fileIndex: 0, turnstileToken: "x".repeat(2049) }), "x".repeat(5000),
  ]) {
    const response = await download(SLUG, body);
    expect(response.status).toBe(422);
    expect(await response.json()).toEqual({ code: "validation_error" });
    expectPrivate(response);
  }
  const oversized = await download(SLUG, JSON.stringify({ fileIndex: 0 }), { "content-length": "9999" });
  expect(oversized.status).toBe(422);
  for (const slug of ["", "short", "../../x/y/z/abc"]) {
    const response = await download(slug, JSON.stringify({ fileIndex: 0 }));
    expect(response.status).toBe(404);
    expect(await response.json()).toEqual({ code: "share_not_found" });
  }
  expect(seen).toHaveLength(0);
});

const { POST: REPORT } = await import("../src/pages/api/cloud/share/public/[slug]/report");

const report = (slug: string, body: string, headers: Record<string, string> = {}) =>
  REPORT({
    params: { slug },
    request: new Request(`https://site.example/api/cloud/share/public/${slug}/report`, {
      method: "POST", body, headers: { "x-client-ip": "203.0.113.7", ...headers },
    }),
    clientAddress: "10.0.0.1",
  } as unknown as Parameters<typeof REPORT>[0]);

test("report: forwards reason, detail, contact and the turnstile token; 204 passes through", async () => {
  reply = () => new Response(null, { status: 204 });
  const response = await report(SLUG, JSON.stringify({
    reason: " 侵权 ", detail: "盗版内容", contact: "me@example.com", turnstileToken: "tok",
  }));
  expect(response.status).toBe(204);
  expect(await response.text()).toBe("");
  expect(response.headers.get("X-Robots-Tag")).toBe("noindex, nofollow");
  expect(response.headers.get("Cache-Control")).toBe("no-store");
  expect(seen).toHaveLength(1);
  expect(seen[0].url).toBe(`http://cloud.test/api/v1/share/public/${SLUG}/report`);
  expect(seen[0].method).toBe("POST");
  expect(seen[0].headers["x-forwarded-for"]).toBe("203.0.113.7");
  expect(JSON.parse(seen[0].body)).toEqual({
    reason: "侵权", detail: "盗版内容", contact: "me@example.com", turnstileToken: "tok",
  });
});

test("report: optional fields that are blank are not sent", async () => {
  reply = () => new Response(null, { status: 204 });
  await report(SLUG, JSON.stringify({ reason: "其他", detail: "  ", contact: "", turnstileToken: null }));
  expect(JSON.parse(seen[0].body)).toEqual({ reason: "其他" });
});

test("report: FluxCloud errors pass through (ended, turnstile, rate limit)", async () => {
  const cases = [
    [404, "share_not_found"], [410, "share_ended"], [403, "turnstile_required"], [400, "turnstile_failed"], [429, "rate_limited"],
  ] as const;
  for (const [status, code] of cases) {
    reply = () => new Response(JSON.stringify({ code, message: "m" }), {
      status, headers: status === 429 ? { "Retry-After": "600" } : {},
    });
    const response = await report(SLUG, JSON.stringify({ reason: "侵权" }));
    expect(response.status).toBe(status);
    expectPrivate(response);
    expect(await response.json()).toEqual({ code, message: "m" });
    if (status === 429) expect(response.headers.get("Retry-After")).toBe("600");
  }
});

test("report: field caps and bad input are rejected before FluxCloud is called", async () => {
  const ok = { reason: "侵权" };
  for (const body of [
    "", "nope", "[]", JSON.stringify({}), JSON.stringify({ reason: "" }), JSON.stringify({ reason: "  " }),
    JSON.stringify({ reason: 5 }), JSON.stringify({ reason: "x".repeat(65) }),
    JSON.stringify({ ...ok, detail: "x".repeat(2001) }), JSON.stringify({ ...ok, detail: 1 }),
    JSON.stringify({ ...ok, contact: "x".repeat(201) }), JSON.stringify({ ...ok, contact: {} }),
    JSON.stringify({ ...ok, turnstileToken: "x".repeat(2049) }), "x".repeat(20_000),
  ]) {
    const response = await report(SLUG, body);
    expect(response.status).toBe(422);
    expect(await response.json()).toEqual({ code: "validation_error" });
    expectPrivate(response);
  }
  expect((await report(SLUG, JSON.stringify(ok), { "content-length": "99999" })).status).toBe(422);
  for (const slug of ["", "short", "../../x/y/z/abc"]) {
    const response = await report(slug, JSON.stringify(ok));
    expect(response.status).toBe(404);
    expect(await response.json()).toEqual({ code: "share_not_found" });
  }
  expect(seen).toHaveLength(0);
});

test("report: boundary lengths are accepted", async () => {
  reply = () => new Response(null, { status: 204 });
  const response = await report(SLUG, JSON.stringify({
    reason: "x".repeat(64), detail: "d".repeat(2000), contact: "c".repeat(200),
  }));
  expect(response.status).toBe(204);
  expect(seen).toHaveLength(1);
});
