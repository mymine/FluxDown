import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { CookieJar, sidAuthorization } from "../src/cookies.js";
import { parseQuery, setUrlParam, urlParam } from "../src/query.js";
import { sha1Hex } from "../src/sha1.js";
import { parseTarget } from "../src/target.js";

const sha1 = (s: string) => createHash("sha1").update(s, "utf8").digest("hex");

describe("query", () => {
  test("decodes + and percent escapes, keeps the first duplicate and raw invalid escapes", () => {
    expect(parseQuery("s=a%2Bb%3D&sp=sig&url=https%3A%2F%2Fx%2Fy%3Fn%3D1&s=second&bad=%E0%A4&sp2=a+b")).toEqual(
      Object.assign(Object.create(null), { s: "a+b=", sp: "sig", url: "https://x/y?n=1", bad: "%E0%A4", sp2: "a b" }),
    );
  });

  test("replaces an existing parameter in place and appends a missing one", () => {
    const url = "https://r1.googlevideo.com/videoplayback?expire=1&n=abc&itag=18#frag";
    const withN = setUrlParam(url, "n", "x/y=");
    expect(withN).toBe("https://r1.googlevideo.com/videoplayback?expire=1&n=x%2Fy%3D&itag=18");
    expect(urlParam(withN, "n")).toBe("x/y=");
    expect(setUrlParam(withN, "sig", "AB-_")).toBe(`${withN}&sig=AB-_`);
  });
});

describe("SAPISIDHASH", () => {
  test("sha1 matches node:crypto, including non-ASCII input", () => {
    for (const s of ["", "abc", "1700000000 sid https://www.youtube.com", "中文 🎬 ".repeat(40)]) {
      expect(sha1Hex(s)).toBe(sha1(s));
    }
  });

  test("emits one hash per SAPISID cookie in yt-dlp's format, with the user session suffix", () => {
    const jar = CookieJar.parse("SAPISID=aaa; __Secure-1PAPISID=bbb; __Secure-3PAPISID=ccc; LOGIN_INFO=x");
    const origin = "https://www.youtube.com";
    expect(sidAuthorization(jar, origin, "", 1700000000)).toBe(
      [
        `SAPISIDHASH 1700000000_${sha1(`1700000000 aaa ${origin}`)}`,
        `SAPISID1PHASH 1700000000_${sha1(`1700000000 bbb ${origin}`)}`,
        `SAPISID3PHASH 1700000000_${sha1(`1700000000 ccc ${origin}`)}`,
      ].join(" "),
    );
    expect(sidAuthorization(jar, origin, "1234", 1700000000)?.split(" ")[1]).toBe(
      `1700000000_${sha1(`1234 1700000000 aaa ${origin}`)}_u`,
    );
  });

  test("falls back to __Secure-3PAPISID for SAPISID and returns null without any", () => {
    const only3p = CookieJar.parse("__Secure-3PAPISID=ccc");
    expect(sidAuthorization(only3p, "https://o", "", 1)).toBe(
      `SAPISIDHASH 1_${sha1("1 ccc https://o")} SAPISID3PHASH 1_${sha1("1 ccc https://o")}`,
    );
    expect(sidAuthorization(CookieJar.parse("YSC=1"), "https://o", "", 1)).toBeNull();
  });
});

describe("cookie jar", () => {
  test("reads Netscape files including #HttpOnly_ lines and only youtube.com domains", () => {
    const jar = CookieJar.parse(
      [
        "# Netscape HTTP Cookie File",
        "#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tLOGIN_INFO\tAFm=x:y",
        ".youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tabc/def",
        ".google.com\tTRUE\t/\tTRUE\t0\tSID\tnope",
      ].join("\n"),
    );
    expect(jar.get("LOGIN_INFO")).toBe("AFm=x:y");
    expect(jar.get("SAPISID")).toBe("abc/def");
    expect(jar.get("SID")).toBeUndefined();
    expect(jar.isAuthenticated()).toBe(true);
  });

  test("login needs LOGIN_INFO plus a SAPISID-family cookie", () => {
    expect(CookieJar.parse("SAPISID=a").isAuthenticated()).toBe(false);
    expect(CookieJar.parse("LOGIN_INFO=a").isAuthenticated()).toBe(false);
    expect(CookieJar.parse("LOGIN_INFO=a; __Secure-1PAPISID=b").isAuthenticated()).toBe(true);
  });

  test("defaults merge PREF and set SOCS only for anonymous sessions without a refusal", () => {
    const anon = CookieJar.parse("PREF=f6=40000000&hl=zh-CN");
    anon.applyDefaults();
    expect(parseQuery(anon.get("PREF") ?? "")).toEqual(Object.assign(Object.create(null), { f6: "40000000", hl: "en", tz: "UTC" }));
    expect(anon.get("SOCS")).toBe("CAI");

    const refused = CookieJar.parse("SOCS=CAE");
    refused.applyDefaults();
    expect(refused.get("SOCS")).toBe("CAE");

    const unanswered = CookieJar.parse("SOCS=CAAaBgi");
    unanswered.applyDefaults();
    expect(unanswered.get("SOCS")).toBe("CAI");

    const authed = CookieJar.parse("LOGIN_INFO=a; SAPISID=b");
    authed.applyDefaults();
    expect(authed.get("SOCS")).toBeUndefined();
  });

  test("absorbs Set-Cookie lines and drops expired cookies", () => {
    const jar = CookieJar.parse("YSC=old; GPS=1");
    jar.absorb(
      ["YSC=new; Domain=.youtube.com; Path=/; Secure", "GPS=1; Max-Age=0", "VISITOR_INFO1_LIVE=v; Expires=Thu, 01 Jan 1970 00:00:00 GMT"].join("\n"),
      Date.UTC(2026, 0, 1),
    );
    expect(jar.header()).toBe("YSC=new");
  });
});

describe("link targets", () => {
  test("maps every supported link shape", () => {
    expect(parseTarget("https://youtu.be/dQw4w9WgXcQ?t=3")).toEqual({ videoId: "dQw4w9WgXcQ" });
    expect(parseTarget("https://m.youtube.com/watch?feature=x&v=dQw4w9WgXcQ&list=PL123")).toEqual({ videoId: "dQw4w9WgXcQ" });
    expect(parseTarget("https://www.youtube.com/shorts/dQw4w9WgXcQ")).toEqual({ videoId: "dQw4w9WgXcQ" });
    expect(parseTarget("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ")).toEqual({ videoId: "dQw4w9WgXcQ" });
    expect(parseTarget("https://music.youtube.com/playlist?list=OLAK5uy_abc")).toEqual({ playlistId: "OLAK5uy_abc" });
    expect(parseTarget("https://www.youtube.com/watch?v=short")).toBeNull();
    expect(parseTarget("https://evil-youtube.com/watch?v=dQw4w9WgXcQ")).toBeNull();
  });
});
