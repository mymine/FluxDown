import { describe, expect, test } from "bun:test";
import { extractExtension, hostOf, parseUrlParts, pathOf } from "./url";

describe("hostOf", () => {
  test("小写并去掉 FQDN 末尾的点", () => {
    expect(hostOf("https://WWW.Example.COM/a")).toBe("www.example.com");
    expect(hostOf("https://www.google-analytics.com./collect")).toBe("www.google-analytics.com");
  });

  test("端口不进入 host；IPv6 保留方括号", () => {
    expect(hostOf("https://example.com:8443/a")).toBe("example.com");
    expect(hostOf("http://[::1]:8080/a.mp4")).toBe("[::1]");
    expect(hostOf("http://[2001:db8::1]/a")).toBe("[2001:db8::1]");
  });

  test("IDN 转为 punycode", () => {
    expect(hostOf("https://例子.com/a")).toBe("xn--fsqu00a.com");
  });

  test("无法解析返回空串", () => {
    expect(hostOf("not a url")).toBe("");
    expect(hostOf("")).toBe("");
  });
});

describe("pathOf", () => {
  test("pathname 不含 query / fragment；非法 URL 为空串", () => {
    expect(pathOf("https://x.com/a/b.mp4?q=1#f")).toBe("/a/b.mp4");
    expect(pathOf("nope")).toBe("");
  });
});

describe("parseUrlParts", () => {
  test("一次解析得到 host / path / ext", () => {
    expect(parseUrlParts("https://EXAMPLE.com:8443/x/y.MP4?a=b#c")).toEqual({
      host: "example.com",
      path: "/x/y.MP4",
      ext: "mp4",
    });
  });

  test("剥离路径参数 ;jsessionid=…", () => {
    expect(parseUrlParts("https://x.com/a.mp4;jsessionid=ABC?y=1").ext).toBe("mp4");
    expect(parseUrlParts("https://x.com/dir;v=1/a.zip").ext).toBe("zip");
    expect(parseUrlParts("https://x.com/a;jsessionid=B.mp4").ext).toBe("");
  });

  test("扩展名必须是 1–10 位字母数字", () => {
    expect(parseUrlParts("https://x.com/a.abcdefghij").ext).toBe("abcdefghij");
    expect(parseUrlParts("https://x.com/a.abcdefghijk").ext).toBe("");
    expect(parseUrlParts("https://x.com/a.m%34s").ext).toBe("");
    expect(parseUrlParts("https://x.com/a.tar-gz").ext).toBe("");
  });

  test("段首点、段尾点、末段带斜杠都不算扩展名", () => {
    expect(parseUrlParts("https://x.com/.htaccess").ext).toBe("");
    expect(parseUrlParts("https://x.com/a.").ext).toBe("");
    expect(parseUrlParts("https://x.com/a.m3u8/").ext).toBe("");
    expect(parseUrlParts("https://x.com/").ext).toBe("");
  });

  test("多个点取最后一段", () => {
    expect(parseUrlParts("https://x.com/a.tar.gz").ext).toBe("gz");
    expect(parseUrlParts("https://x.com/file.v1.2").ext).toBe("2");
  });

  test("非法 URL 退回文本匹配，host / path 为空", () => {
    expect(parseUrlParts("not a url/a.mp4?x")).toEqual({ host: "", path: "", ext: "mp4" });
    expect(parseUrlParts("not a url/a.mp4;sid=1#f").ext).toBe("mp4");
    expect(parseUrlParts("not a url").ext).toBe("");
  });
});

describe("extractExtension", () => {
  const known = new Set(["jpg", "mp4"]);

  test("path 扩展名优先；不传 queryKeys 只看 path", () => {
    expect(extractExtension("https://x/a.png?format=jpg", ["format"], known)).toBe("png");
    expect(extractExtension("https://x/a?format=jpg")).toBe("");
  });

  test("queryKeys 指定参数的值须在 known 内", () => {
    expect(extractExtension("https://x/media/A?format=jpg&name=orig", ["format"], known)).toBe("jpg");
    expect(extractExtension("https://x/media/A?format=exe", ["format"], known)).toBe("");
  });

  test("再扫描所有 query 值里以 .ext 结尾的", () => {
    expect(extractExtension("https://x/dl?file=movie.mp4", ["format"], known)).toBe("mp4");
    expect(extractExtension("https://x/dl?file=movie.exe", ["format"], known)).toBe("");
  });
});
