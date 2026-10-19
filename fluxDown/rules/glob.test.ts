import { describe, expect, test } from "bun:test";
import { asciiLower, compileGlob, hostMatches, hostMatchesPattern, parseHostPattern, urlGlobMatch } from "./glob";
import { hostOf } from "./url";

/** 编译前的参考实现（逐行移植 manifest.rs，未做任何优化），用于对拍。 */
function referenceGlob(pattern: string, url: string): boolean {
  const lower = (s: string) => s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));
  const p = lower(pattern);
  const text = lower(url);
  if (!p.includes("*")) return p === text;
  const parts = p.split("*");
  const n = parts.length;
  let pos = 0;
  for (let i = 0; i < n; i++) {
    const part = parts[i];
    if (part === "") continue;
    if (i === 0) {
      if (!text.slice(pos).startsWith(part)) return false;
      pos += part.length;
    } else if (i === n - 1) {
      if (!text.slice(pos).endsWith(part)) return false;
    } else {
      const idx = text.slice(pos).indexOf(part);
      if (idx < 0) return false;
      pos += idx + part.length;
    }
  }
  return true;
}

describe("urlGlobMatch（对应 manifest.rs doctest）", () => {
  test("四个 doctest 断言", () => {
    expect(urlGlobMatch("*://www.youtube.com/watch*", "https://www.youtube.com/watch?v=abc")).toBe(true);
    expect(urlGlobMatch("*", "anything")).toBe(true);
    expect(urlGlobMatch("https://x.com/a", "https://x.com/a")).toBe(true);
    expect(urlGlobMatch("*://x.com/*", "https://y.com/a")).toBe(false);
  });

  test("分段顺序：*://x.com/*a*b", () => {
    expect(urlGlobMatch("*://x.com/*a*b", "https://x.com/xa_b")).toBe(true);
    expect(urlGlobMatch("*://x.com/*a*b", "https://x.com/b-a")).toBe(false);
  });

  test("大小写不敏感；无 * 时精确相等", () => {
    expect(urlGlobMatch("HTTPS://X.com/*", "https://x.COM/A")).toBe(true);
    expect(urlGlobMatch("https://x.com/a", "https://x.com/a/")).toBe(false);
  });
});

describe("hostMatches", () => {
  test("本域及子域，按标签边界", () => {
    expect(hostMatches("a.bilibili.com", "bilibili.com")).toBe(true);
    expect(hostMatches("bilibili.com", "bilibili.com")).toBe(true);
    expect(hostMatches("notbilibili.com", "bilibili.com")).toBe(false);
  });

  test("= 前缀只匹配本域；*. 前缀不支持", () => {
    expect(hostMatches("x.com", "=x.com")).toBe(true);
    expect(hostMatches("a.x.com", "=x.com")).toBe(false);
    expect(hostMatches("a.x.com", "*.x.com")).toBe(false);
  });
});

describe("compileGlob", () => {
  test("与参考实现逐例对拍（含重叠分段、空段、多个 *）", () => {
    const patterns = [
      "*", "**", "a", "a*", "*a", "*a*", "a*b", "*a*b*", "a*a", "ab*ba", "*ab*ba", "a**b", "*://x.com/*a*b",
      "https://*.cdn.com/*.mp4", "*foo*bar*", "*aa*aa", "aa*aa*aa",
    ];
    const alphabet = ["a", "b", "A", "/", ":", "x", "."];
    let seed = 12345;
    const next = (): number => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed;
    };
    for (const p of patterns) {
      for (let i = 0; i < 300; i++) {
        const len = next() % 12;
        let text = "";
        for (let k = 0; k < len; k++) text += alphabet[next() % alphabet.length];
        expect([p, text, urlGlobMatch(p, text)]).toEqual([p, text, referenceGlob(p, text)]);
      }
    }
  });

  test("纯 *foo* 走子串匹配，输入需预先小写", () => {
    const g = compileGlob("*Doubleclick.NET*");
    expect(g(asciiLower("https://AD.DoubleClick.net/x"))).toBe(true);
    expect(g("https://example.com/")).toBe(false);
  });

  test("首尾锚定不重叠：a*a 至少两个字符", () => {
    expect(compileGlob("a*a")("a")).toBe(false);
    expect(compileGlob("a*a")("aa")).toBe(true);
  });

  test("query / fragment 参与匹配；非 ASCII 字符原样比较", () => {
    expect(urlGlobMatch("*?token=*", "https://x.com/a?token=1")).toBe(true);
    expect(urlGlobMatch("*#frag", "https://x.com/a#frag")).toBe(true);
    expect(urlGlobMatch("https://x.com/例子*", "https://x.com/例子/a")).toBe(true);
    expect(urlGlobMatch("https://x.com/É*", "https://x.com/é/a")).toBe(false);
  });

  test("空 pattern 只匹配空串", () => {
    expect(urlGlobMatch("", "")).toBe(true);
    expect(urlGlobMatch("", "a")).toBe(false);
  });
});

describe("hostMatches 边界", () => {
  test("末尾点：URL host 经 hostOf 归一后可命中；模式末尾点同样被忽略", () => {
    expect(hostMatches(hostOf("https://www.google-analytics.com./collect"), "google-analytics.com")).toBe(true);
    expect(hostMatches("a.x.com", "x.com.")).toBe(true);
    expect(hostMatches("x.com", "=x.com.")).toBe(true);
  });

  test("端口不影响 host 匹配；模式里的端口被忽略", () => {
    expect(hostMatches(hostOf("https://example.com:8443/a"), "example.com")).toBe(true);
    expect(parseHostPattern("example.com:8080")?.domain).toBe("example.com");
  });

  test("IPv6 字面量：方括号形式的模式只命中该地址", () => {
    expect(hostMatches(hostOf("http://[::1]:8080/a"), "[::1]")).toBe(true);
    expect(hostMatches(hostOf("http://[::2]/a"), "[::1]")).toBe(false);
  });

  test("IDN 模式转为 punycode 后与 URL host 匹配", () => {
    expect(parseHostPattern("例子.com")?.domain).toBe("xn--fsqu00a.com");
    expect(hostMatches(hostOf("https://www.例子.com/"), "例子.com")).toBe(true);
    expect(hostMatches(hostOf("https://例子.com/"), "=xn--fsqu00a.com")).toBe(true);
  });

  test("hostMatchesPattern 对大小写不敏感；子域后缀不跨标签", () => {
    const p = parseHostPattern("example.com");
    expect(p && hostMatchesPattern("WWW.EXAMPLE.COM", p)).toBe(true);
    expect(p && hostMatchesPattern("badexample.com", p)).toBe(false);
    expect(p?.suffix).toBe(".example.com");
  });
});
