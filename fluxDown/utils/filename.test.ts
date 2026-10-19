import { describe, expect, test } from "bun:test";
import {
  decodeLegacyBytes,
  extractCleanFilename,
  looksLikeRealFilename,
  mimeToExtension,
  parseContentDispositionFilename,
  queryDispositionFilename,
  readDispositionHeader,
  resolveDownloadFilename,
} from "./filename";
import fixtures from "../../native/engine/src/naming/fixtures/content_disposition.json";

/** 反查表：用 TextDecoder 枚举双字节码位，把字符串编码为 GBK/Big5 字节。 */
function legacyEncoder(label: "gbk" | "big5") {
  const decoder = new TextDecoder(label);
  const map = new Map<string, number[]>();
  for (let lead = 0x81; lead <= 0xfe; lead++) {
    for (let trail = 0x40; trail <= 0xfe; trail++) {
      const ch = decoder.decode(Uint8Array.of(lead, trail));
      if (ch.length === 1 && ch !== "\ufffd" && !map.has(ch)) {
        map.set(ch, [lead, trail]);
      }
    }
  }
  return (s: string): number[] => {
    const out: number[] = [];
    for (const ch of s) {
      if (ch.charCodeAt(0) < 0x80) out.push(ch.charCodeAt(0));
      else out.push(...(map.get(ch) ?? []));
    }
    return out;
  };
}

const encodeGbk = legacyEncoder("gbk");
const encodeBig5 = legacyEncoder("big5");

/** 把字节数组变成字节载体字符串（每个 char = 一个字节）。 */
const carrier = (bytes: number[]) => String.fromCharCode(...bytes);

describe("readDispositionHeader", () => {
  test("binaryValue becomes a byte carrier", () => {
    const d = readDispositionHeader({ binaryValue: [0x61, 0xd2, 0xda] }, false);
    expect(d).toEqual({ text: "a\u00d2\u00da", byteCarrier: true });
  });

  test("value with chars above 0xFF is decoded text everywhere", () => {
    expect(readDispositionHeader({ value: "filename=你好.zip" }, true)).toEqual({
      text: "filename=你好.zip",
      byteCarrier: false,
    });
  });

  test("latin-1 range value: carrier on Firefox, text on Chromium", () => {
    const value = "filename=caf\u00e9.zip";
    expect(readDispositionHeader({ value }, true)?.byteCarrier).toBe(true);
    expect(readDispositionHeader({ value }, false)?.byteCarrier).toBe(false);
  });

  test("empty header yields null", () => {
    expect(readDispositionHeader({}, false)).toBeNull();
  });
});

describe("parseContentDispositionFilename", () => {
  test("Chrome decoded text keeps Chinese name intact", () => {
    expect(
      parseContentDispositionFilename('attachment; filename="你好.zip"', false),
    ).toBe("你好.zip");
  });

  test("GBK bytes in a byte carrier", () => {
    const name = "亿图模切机插件V1.3.4.rar";
    const header = carrier([
      ...Array.from("attachment; filename=\"").map((c) => c.charCodeAt(0)),
      ...encodeGbk(name),
      0x22,
    ]);
    expect(parseContentDispositionFilename(header, true)).toBe(name);
  });

  test("Big5 bytes in a byte carrier", () => {
    const name = "繁體中文檔名.pdf";
    const header = carrier([
      ...Array.from("attachment; filename=").map((c) => c.charCodeAt(0)),
      ...encodeBig5(name),
    ]);
    expect(parseContentDispositionFilename(header, true)).toBe(name);
  });

  test("UTF-8 bytes in a byte carrier", () => {
    const bytes = Array.from(new TextEncoder().encode("文件.txt"));
    const header = carrier([
      ...Array.from('attachment; filename="').map((c) => c.charCodeAt(0)),
      ...bytes,
      0x22,
    ]);
    expect(parseContentDispositionFilename(header, true)).toBe("文件.txt");
  });

  test("filename* UTF-8 with percent-encoding", () => {
    expect(
      parseContentDispositionFilename(
        "attachment; filename*=\"UTF-8''a%20b.exe\"",
      ),
    ).toBe("a b.exe");
  });

  test("filename* with GBK charset", () => {
    expect(
      parseContentDispositionFilename("attachment; filename*=GBK''%D2%DA%CD%BC.rar"),
    ).toBe("亿图.rar");
  });

  test("percent-encoded quoted filename", () => {
    expect(
      parseContentDispositionFilename('attachment; filename="%E6%B0%B8.mp4"'),
    ).toBe("永.mp4");
  });

  test("unquoted filename keeps inner spaces and stops at ;", () => {
    expect(
      parseContentDispositionFilename("attachment; filename=aaa bbb.zip"),
    ).toBe("aaa bbb.zip");
    expect(
      parseContentDispositionFilename("attachment; filename=aaa bbb.zip; size=1"),
    ).toBe("aaa bbb.zip");
  });

  test("no filename yields empty", () => {
    expect(parseContentDispositionFilename("attachment")).toBe("");
  });
});

describe("extractCleanFilename", () => {
  test("appends extension from mime for extensionless URL segment", () => {
    expect(
      extractCleanFilename(
        "",
        "https://pbs.twimg.com/media/AbC?format=jpg&name=large",
        "image/jpeg",
      ),
    ).toBe("AbC.jpg");
  });

  test("script endpoints return empty", () => {
    expect(
      extractCleanFilename("", "https://x/forum/attachment.php?aid=1"),
    ).toBe("");
    expect(
      extractCleanFilename("/tmp/attachment.php", "https://x/a/b", "image/png"),
    ).toBe("");
  });

  test("real extensions are never changed by mime", () => {
    expect(
      extractCleanFilename("/dl/report.pdf", "https://x/y", "image/png"),
    ).toBe("report.pdf");
    expect(
      extractCleanFilename("", "https://x/media/a.bin", "image/png"),
    ).toBe("a.bin");
  });

  test("skips unhelpful mimes and empty names", () => {
    expect(extractCleanFilename("", "https://x/abc", "text/html")).toBe("abc");
    expect(
      extractCleanFilename("", "https://x/abc", "application/octet-stream"),
    ).toBe("abc");
    expect(extractCleanFilename("", "https://x/", "image/png")).toBe("");
  });
});

describe("resolveDownloadFilename", () => {
  test("real header name wins", () => {
    expect(
      resolveDownloadFilename("你好.zip", "/dl/other.zip", "https://x/a"),
    ).toBe("你好.zip");
  });

  test("extensionless header name falls back to browser name", () => {
    expect(
      resolveDownloadFilename("garbled", "/dl/good.rar", "https://x/a.php"),
    ).toBe("good.rar");
  });

  test("extensionless header name used when nothing else", () => {
    expect(resolveDownloadFilename("report", "", "https://x/a.php")).toBe(
      "report",
    );
  });

  test("script header name and script URL yield empty", () => {
    expect(resolveDownloadFilename("a.php", "", "https://x/a.php")).toBe("");
  });
});

describe("helpers", () => {
  test("looksLikeRealFilename", () => {
    expect(looksLikeRealFilename("a.zip")).toBe(true);
    expect(looksLikeRealFilename("attachment.php")).toBe(false);
    expect(looksLikeRealFilename("x.action")).toBe(false);
    expect(looksLikeRealFilename("noext")).toBe(false);
  });

  test("mimeToExtension", () => {
    expect(mimeToExtension("image/jpeg; charset=x")).toBe("jpg");
    expect(mimeToExtension("text/html")).toBeNull();
    expect(mimeToExtension("application/xhtml+xml")).toBeNull();
    expect(mimeToExtension("application/octet-stream")).toBeNull();
    expect(mimeToExtension("constructor")).toBeNull();
  });
});

describe("shared content_disposition fixtures", () => {
  interface FixtureCase {
    name: string;
    header?: string;
    headerHex?: string;
    host?: string;
    pageCharset?: string;
    expected: string | null;
  }

  const cases: FixtureCase[] = fixtures.cases;

  const rawBytes = (c: FixtureCase): Uint8Array =>
    c.headerHex !== undefined
      ? Uint8Array.from(
          c.headerHex.match(/../g)?.map((h) => parseInt(h, 16)) ?? [],
        )
      : new TextEncoder().encode(c.header ?? "");

  const latin1 = (bytes: Uint8Array) => String.fromCharCode(...bytes);

  for (const c of cases) {
    const hints = { host: c.host, pageCharset: c.pageCharset };
    const expected = c.expected ?? "";

    test(`Chrome model: ${c.name}`, () => {
      const bytes = rawBytes(c);
      let text: string | null = null;
      try {
        text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      } catch {
        text = null;
      }
      const got =
        text !== null
          ? parseContentDispositionFilename(text, false, hints)
          : parseContentDispositionFilename(latin1(bytes), true, hints);
      expect(got).toBe(expected);
    });

    test(`Firefox model: ${c.name}`, () => {
      expect(
        parseContentDispositionFilename(latin1(rawBytes(c)), true, hints),
      ).toBe(expected);
    });
  }
});

describe("decodeLegacyBytes", () => {
  test("valid UTF-8 wins over every prior", () => {
    const bytes = new TextEncoder().encode("你好");
    expect(decodeLegacyBytes(bytes, { host: "a.tw", pageCharset: "big5" })).toBe(
      "你好",
    );
  });

  test("ASCII passes through", () => {
    expect(decodeLegacyBytes(new TextEncoder().encode("a.zip"))).toBe("a.zip");
  });
});

describe("extractCleanFilename mime fallback for batch items", () => {
  test("extensionless CDN segment gets extension from mime", () => {
    expect(
      extractCleanFilename(undefined, "https://cdn/x/abc123", "application/zip"),
    ).toBe("abc123.zip");
  });
});

describe("queryDispositionFilename", () => {
  // 与引擎 naming 测试同一组样例（GitHub release / S3 / Azure）。
  const github =
    "https://release-assets.githubusercontent.com/github-production-release-asset/212613049/55d46ea0-2e36-4071-bd9b-6536b7dae14d" +
    "?sp=r&sv=2018-11-09&rscd=attachment%3B+filename%3Dgh_2.62.0_macOS_arm64.zip&rsct=application%2Foctet-stream";

  test("reads Azure rscd and S3 response-content-disposition", () => {
    expect(queryDispositionFilename(github)).toBe("gh_2.62.0_macOS_arm64.zip");
    expect(
      queryDispositionFilename(
        "https://bucket.s3.amazonaws.com/key/abc123?response-content-disposition=attachment%3B%20filename%3D%22a%20b.zip%22&X-Amz-Signature=deadbeef",
      ),
    ).toBe("a b.zip");
    expect(
      queryDispositionFilename(
        "https://e.com/o?Response-Content-Disposition=attachment%3B%20filename%2A%3DUTF-8%27%27%25E4%25BD%25A0%25E5%25A5%25BD.zip",
      ),
    ).toBe("你好.zip");
  });

  test("no filename in the query disposition yields empty", () => {
    expect(queryDispositionFilename("https://e.com/f.bin?rscd=inline")).toBe("");
    expect(queryDispositionFilename("https://e.com/f.bin")).toBe("");
    expect(queryDispositionFilename("https://e.com/f.bin#rscd=attachment%3Bfilename%3Dx.zip")).toBe("");
  });

  test("signed CDN link without a browser name uses the query name, not the object id", () => {
    expect(extractCleanFilename("", github, "application/octet-stream")).toBe("gh_2.62.0_macOS_arm64.zip");
    // 浏览器定下的真实名仍然优先。
    expect(extractCleanFilename("/dl/renamed.zip", github)).toBe("renamed.zip");
  });
});
