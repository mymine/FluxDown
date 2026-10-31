import { describe, expect, test } from "bun:test";
import { ANDROID_EXTRA, ANDROID_PACKAGE, buildAndroidIntentUrl } from "./android-intent";

/**
 * 按 Android `Intent.parseUri(uri, URI_INTENT_SCHEME)` 的规则还原：数据部分 = 最后一个 `#` 之前，
 * `intent:` 前缀换成 `scheme`；`S.` extra 的键与值经 `Uri.decode`（不把 `+` 当空格）。
 */
function parseIntentUri(uri: string) {
  const hash = uri.lastIndexOf("#");
  expect(uri.startsWith("#Intent;", hash)).toBe(true);
  expect(uri.endsWith(";end")).toBe(true);
  const fields = uri.slice(hash + "#Intent;".length, -";end".length).split(";");
  const props = new Map<string, string>();
  const extras = new Map<string, string>();
  for (const field of fields) {
    const eq = field.indexOf("=");
    const key = field.slice(0, eq);
    const value = field.slice(eq + 1);
    if (key.startsWith("S.")) extras.set(decodeURIComponent(key.slice(2)), decodeURIComponent(value));
    else props.set(key, value);
  }
  const data = uri.slice(0, hash);
  expect(data.startsWith("intent:")).toBe(true);
  return { data: `${props.get("scheme")}:${data.slice("intent:".length)}`, props, extras };
}

describe("buildAndroidIntentUrl", () => {
  test("http(s) link targets the FluxDown package and carries request context as extras", () => {
    const headers = { "User-Agent": "Mozilla/5.0 (Android 15; Mobile)", Authorization: "Bearer a+b=c" };
    const uri = buildAndroidIntentUrl({
      url: "https://cdn.example.com/files/a%20b.zip?sig=x+y&exp=1#section",
      filename: "  报告 v2;final.zip ",
      cookies: "sid=a=b; token=c;d",
      referrer: "https://example.com/page?x=1#top",
      headers,
    });
    expect(uri).not.toBeNull();
    const parsed = parseIntentUri(uri!);
    expect(parsed.data).toBe("https://cdn.example.com/files/a%20b.zip?sig=x+y&exp=1");
    expect(parsed.props.get("package")).toBe(ANDROID_PACKAGE);
    expect(parsed.props.get("action")).toBe("android.intent.action.VIEW");
    expect(parsed.extras.get(ANDROID_EXTRA.fileName)).toBe("报告 v2;final.zip");
    expect(parsed.extras.get(ANDROID_EXTRA.cookie)).toBe("sid=a=b; token=c;d");
    expect(parsed.extras.get(ANDROID_EXTRA.referer)).toBe("https://example.com/page?x=1#top");
    expect(parsed.extras.get(ANDROID_EXTRA.userAgent)).toBe(headers["User-Agent"]);
    expect(JSON.parse(parsed.extras.get(ANDROID_EXTRA.headers)!)).toEqual(headers);
    // 不带回退地址：有回退地址时浏览器在无手势 / 未选「总是在应用中打开」时会直接导航过去而不唤起 App。
    expect(parsed.extras.has("browser_fallback_url")).toBe(false);
  });

  test("empty context adds no request extras", () => {
    const parsed = parseIntentUri(buildAndroidIntentUrl({ url: "http://host/a.bin", filename: "  " })!);
    expect(parsed.data).toBe("http://host/a.bin");
    expect([...parsed.extras.keys()]).toEqual([]);
  });

  test("user agent extra is found case-insensitively", () => {
    const parsed = parseIntentUri(buildAndroidIntentUrl({ url: "https://h/x", headers: { "user-agent": "UA/1" } })!);
    expect(parsed.extras.get(ANDROID_EXTRA.userAgent)).toBe("UA/1");
  });

  test("magnet and ed2k keep their scheme and escape '#'", () => {
    const magnet = parseIntentUri(
      buildAndroidIntentUrl({ url: "MAGNET:?xt=urn:btih:abc&dn=C#%20Book" })!,
    );
    expect(magnet.data).toBe("magnet:?xt=urn:btih:abc&dn=C%23%20Book");
    const ed2k = parseIntentUri(buildAndroidIntentUrl({ url: "ed2k://|file|a.iso|100|0123456789ABCDEF0123456789ABCDEF|/" })!);
    expect(ed2k.data).toBe("ed2k://|file|a.iso|100|0123456789ABCDEF0123456789ABCDEF|/");
  });

  test("schemes without an app VIEW filter are rejected", () => {
    expect(buildAndroidIntentUrl({ url: "ftp://host/a.zip" })).toBeNull();
    expect(buildAndroidIntentUrl({ url: "blob:https://host/uuid" })).toBeNull();
    expect(buildAndroidIntentUrl({ url: "https:no-authority" })).toBeNull();
    expect(buildAndroidIntentUrl({ url: "https://" })).toBeNull();
  });
});
