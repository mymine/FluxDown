/**
 * 文件名工具：Content-Disposition 解析/解码、浏览器/URL 文件名提取、
 * MIME → 扩展名映射。纯函数模块，无浏览器 API 依赖，便于单测。
 *
 * 解码语义与 Rust 引擎 `fluxdown_engine::naming`（native/engine/src/naming/）
 * 逐条一致，共享用例见 native/engine/src/naming/fixtures/content_disposition.json。
 */

/** 从 webRequest 响应头对象读出的 Content-Disposition。 */
export interface DispositionHeader {
  /** 头值字符串（字节载体时每个字符码 = 一个原始字节）。 */
  text: string;
  /**
   * true：`text` 是字节载体（每个 char code 为一个 0..255 字节），需要按字节
   * 还原后做 UTF-8/GBK/Big5 探测；false：`text` 已是解码后的 Unicode 文本。
   */
  byteCarrier: boolean;
}

interface WebRequestHeaderLike {
  value?: string;
  binaryValue?: ArrayLike<number>;
}

/**
 * 把 webRequest 的响应头对象转成 Content-Disposition 输入。
 *
 * - `binaryValue`（Chromium 对非法 UTF-8 头值只提供它）→ 字节载体；
 * - `value` 含 > 0xFF 的字符 → 已解码文本（Chromium 对合法 UTF-8 头值已解码）；
 * - `value` 全部 ≤ 0xFF：Firefox 以 Latin-1 字节载体暴露头字节 → 字节载体；
 *   Chromium 下是已解码文本（如 "café.zip"）。
 */
export function readDispositionHeader(
  h: WebRequestHeaderLike,
  isFirefox: boolean,
): DispositionHeader | null {
  if (h.binaryValue && h.binaryValue.length > 0) {
    let text = "";
    for (let i = 0; i < h.binaryValue.length; i++) {
      text += String.fromCharCode(h.binaryValue[i] & 0xff);
    }
    return { text, byteCarrier: true };
  }
  if (!h.value) return null;
  let wide = false;
  for (let i = 0; i < h.value.length; i++) {
    if (h.value.charCodeAt(i) > 0xff) {
      wide = true;
      break;
    }
  }
  return { text: h.value, byteCarrier: isFirefox && !wide };
}

/**
 * 该头是否值得向发起页询问字符集：仅字节载体、含非 ASCII 字节且不是合法 UTF-8
 * 时，未声明字符集的原始字节才需要 `document.characterSet` 先验。
 */
export function dispositionNeedsPageCharset(
  text: string,
  byteCarrier: boolean,
): boolean {
  if (!byteCarrier) return false;
  const bytes = new Uint8Array(text.length);
  let ascii = true;
  for (let i = 0; i < text.length; i++) {
    bytes[i] = text.charCodeAt(i) & 0xff;
    if (bytes[i] >= 0x80) ascii = false;
  }
  if (ascii) return false;
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return false;
  } catch {
    return true;
  }
}

/** 解码先验：下载 URL 的主机（TLD 先验）与发起页的 `document.characterSet`。 */
export interface NameHints {
  host?: string;
  pageCharset?: string;
}

const textEncoder = new TextEncoder();

/** 严格解码：非法字节或不支持的 WHATWG 标签一律返回 `null`。 */
function strictDecode(bytes: Uint8Array, label: string): string | null {
  try {
    return new TextDecoder(label, { fatal: true, ignoreBOM: true }).decode(
      bytes,
    );
  } catch {
    return null;
  }
}

/** WHATWG 标签 → 规范编码名；未知标签返回 `null`。 */
function canonicalEncoding(label: string | undefined): string | null {
  const l = label?.trim();
  if (!l) return null;
  try {
    return new TextDecoder(l).encoding;
  } catch {
    return null;
  }
}

function latin1ToBytes(s: string): Uint8Array {
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 0xff;
  return out;
}

function bytesToLatin1(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
  return s;
}

/** 仅裁剪 ASCII 空白：字节载体里 0xA0 等字节不能被 JS `trim()` 吞掉。 */
function trimAscii(s: string): string {
  return s.replace(/^[ \t]+|[ \t]+$/g, "");
}

// ── 旧式字节解码打分（与 Rust naming::charset 同一规范）──

function scoreGbk(b: Uint8Array): number {
  let s = 0;
  for (let i = 0; i < b.length; ) {
    const b1 = b[i];
    if (b1 < 0x80) {
      i++;
      continue;
    }
    // 单字节 0x80（欧元符号，GBK 唯一的单字节高位码）与缺尾字节的末尾首字节：-2，前进 1。
    if (b1 === 0x80 || i + 1 >= b.length) {
      s -= 2;
      i++;
      continue;
    }
    const b2 = b[i + 1];
    if (b2 >= 0x30 && b2 <= 0x39) {
      s -= 3;
      i += 4;
      continue;
    }
    const hi = b2 >= 0xa1 && b2 <= 0xfe;
    if (hi && b1 >= 0xb0 && b1 <= 0xd7) s += 2;
    else if (hi && b1 >= 0xd8 && b1 <= 0xf7) s += 0;
    else if (hi && (b1 === 0xa1 || b1 === 0xa3)) s += 0;
    else s -= 2;
    i += 2;
  }
  return s;
}

function scoreBig5(b: Uint8Array): number {
  let s = 0;
  for (let i = 0; i < b.length; ) {
    const b1 = b[i];
    if (b1 < 0x80) {
      i++;
      continue;
    }
    const b2 = i + 1 < b.length ? b[i + 1] : -1;
    if ((b1 >= 0xa4 && b1 <= 0xc5) || (b1 === 0xc6 && b2 <= 0x7e)) s += 2;
    else if (b1 >= 0xc9 && b1 <= 0xf9) s += 0;
    else if (b1 >= 0xa1 && b1 <= 0xa3) s += 0;
    else s -= 2;
    i += 2;
  }
  return s;
}

function scoreShiftJis(b: Uint8Array): number {
  let s = 0;
  for (let i = 0; i < b.length; ) {
    const b1 = b[i];
    if (b1 < 0x80) {
      i++;
      continue;
    }
    if (b1 >= 0xa1 && b1 <= 0xdf) {
      s -= 1;
      i += 1;
      continue;
    }
    if (b1 === 0x80 || b1 === 0xa0 || b1 >= 0xfd) {
      s -= 2;
      i += 1;
      continue;
    }
    if (i + 1 >= b.length) {
      s -= 2;
      break;
    }
    const b2 = b[i + 1];
    if (b1 === 0x82 && b2 >= 0x9f && b2 <= 0xf1) s += 2;
    else if (b1 === 0x83 && b2 >= 0x40 && b2 <= 0x96) s += 2;
    else if (b1 === 0x81) s += 0;
    else if (
      (b1 >= 0x89 && b1 <= 0x97) ||
      (b1 === 0x88 && b2 >= 0x9f) ||
      (b1 === 0x98 && b2 >= 0 && b2 <= 0x72)
    )
      s += 2;
    else if ((b1 >= 0x98 && b1 <= 0x9f) || (b1 >= 0xe0 && b1 <= 0xea)) s += 0;
    else s -= 2;
    i += 2;
  }
  return s;
}

function scoreWindows1252(b: Uint8Array): number {
  let s = 0;
  for (const x of b) {
    if (x < 0x80) continue;
    if (x <= 0x9f) s -= 3;
    else if (x <= 0xbf) s -= 1;
    else if (x === 0xd7 || x === 0xf7) s -= 1;
    else s += 1;
  }
  return s;
}

interface LegacyCandidate {
  label: string;
  score: (b: Uint8Array) => number;
  /** 该 TLD 命中时 +1。 */
  tlds: string[];
  /** `canonicalEncoding(pageCharset)` 命中时 +4。 */
  pages: string[];
}

// 顺序即平分时的优先级：GBK > Big5 > Shift_JIS > windows-1252。
const LEGACY_CANDIDATES: LegacyCandidate[] = [
  { label: "gbk", score: scoreGbk, tlds: ["cn"], pages: ["gbk", "gb18030"] },
  { label: "big5", score: scoreBig5, tlds: ["tw", "hk", "mo"], pages: ["big5"] },
  {
    label: "shift_jis",
    score: scoreShiftJis,
    tlds: ["jp"],
    pages: ["shift_jis"],
  },
  { label: "windows-1252", score: scoreWindows1252, tlds: [], pages: [] },
];

/** 有打分器的编码：page charset 命中它们时只加分，不直接采用。 */
const SCORED_ENCODINGS: Record<string, true> = {
  gbk: true,
  gb18030: true,
  big5: true,
  shift_jis: true,
  "windows-1252": true,
};

/** 不能直接采用的页面字符集：非 ASCII 兼容（ISO-2022-JP、replacement）与 x-user-defined。 */
const NON_ADOPTABLE_PAGE_ENCODINGS: Record<string, true> = {
  "iso-2022-jp": true,
  replacement: true,
  "x-user-defined": true,
};

function hostTld(host: string | undefined): string {
  const h = host?.trim().toLowerCase().replace(/\.$/, "") ?? "";
  const dot = h.lastIndexOf(".");
  return dot >= 0 ? h.slice(dot + 1) : "";
}

/**
 * 旧式字节解码（未声明字符集的文件名字节）；恒成功。
 * 顺序：严格 UTF-8 → 无打分器的 page charset → `.kr` 的 EUC-KR →
 * GBK / Big5 / Shift_JIS / windows-1252 打分（含 TLD 与 page charset 先验）。
 */
export function decodeLegacyBytes(
  bytes: Uint8Array,
  hints: NameHints = {},
): string {
  const utf8 = strictDecode(bytes, "utf-8");
  if (utf8 !== null) return utf8;

  const page = canonicalEncoding(hints.pageCharset);
  if (
    page &&
    page !== "utf-8" &&
    !page.startsWith("utf-16") &&
    !Object.prototype.hasOwnProperty.call(SCORED_ENCODINGS, page) &&
    !Object.prototype.hasOwnProperty.call(NON_ADOPTABLE_PAGE_ENCODINGS, page)
  ) {
    const decoded = strictDecode(bytes, page);
    if (decoded !== null) return decoded;
  }

  const tld = hostTld(hints.host);
  if (tld === "kr") {
    const decoded = strictDecode(bytes, "euc-kr");
    if (decoded !== null) return decoded;
  }

  let best: { text: string; score: number } | null = null;
  for (const c of LEGACY_CANDIDATES) {
    const text = strictDecode(bytes, c.label);
    if (text === null) continue;
    let score = c.score(bytes);
    if (c.tlds.includes(tld)) score += 1;
    if (page && c.pages.includes(page)) score += 4;
    if (best === null || score > best.score) best = { text, score };
  }
  // windows-1252 恒能解码，best 实际不会为 null；保底按 Latin-1 逐字节映射。
  return best ? best.text : bytesToLatin1(bytes);
}

function percentDecodeLatin1(s: string): Uint8Array {
  const out: number[] = [];
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "%" && /^[0-9a-fA-F]{2}$/.test(s.slice(i + 1, i + 3))) {
      out.push(parseInt(s.slice(i + 1, i + 3), 16));
      i += 2;
    } else {
      out.push(s.charCodeAt(i) & 0xff);
    }
  }
  return Uint8Array.from(out);
}

function concatBytes(chunks: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const c of chunks) total += c.length;
  const out = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.length;
  }
  return out;
}

/**
 * 按声明的字符集解码。空标签 → 旧式字节解码；utf-8 → 严格解码（`legacyOnUtf8Fail`
 * 时失败改走旧式字节解码）；其他 WHATWG 标签严格解码，未知标签/非法字节返回 `null`。
 */
function decodeWithCharset(
  charset: string,
  bytes: Uint8Array,
  hints: NameHints,
  legacyOnUtf8Fail: boolean,
): string | null {
  if (!charset) return decodeLegacyBytes(bytes, hints);
  if (canonicalEncoding(charset) === "utf-8") {
    const utf8 = strictDecode(bytes, "utf-8");
    if (utf8 !== null || !legacyOnUtf8Fail) return utf8;
    return decodeLegacyBytes(bytes, hints);
  }
  return strictDecode(bytes, charset);
}

interface DispositionParam {
  name: string;
  /** 去引号/反转义后的值（Latin-1 字节串）。 */
  value: string;
}

/** 按 `;` 切分；双引号内不分隔，引号内仅 `\"`、`\\` 视为转义对。 */
function splitParams(s: string): string[] {
  const out: string[] = [];
  let cur = "";
  let inQuote = false;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inQuote && c === "\\" && (s[i + 1] === '"' || s[i + 1] === "\\")) {
      cur += c + s[i + 1];
      i++;
      continue;
    }
    if (c === '"') inQuote = !inQuote;
    if (c === ";" && !inQuote) {
      out.push(cur);
      cur = "";
      continue;
    }
    cur += c;
  }
  out.push(cur);
  return out;
}

/** 去两端引号；引号内只把 `\"`→`"`、`\\`→`\`，反斜杠后跟其他字节原样保留。 */
function unquote(raw: string): string {
  const v = trimAscii(raw);
  if (!v.startsWith('"')) return v;
  let out = "";
  for (let i = 1; i < v.length; i++) {
    const c = v[i];
    if (c === "\\" && (v[i + 1] === '"' || v[i + 1] === "\\")) {
      out += v[i + 1];
      i++;
      continue;
    }
    if (c === '"') return out;
    out += c;
  }
  return out;
}

const EXT_VALUE_RE = /^([^']*)'[^']*'([\s\S]*)$/;

/** RFC 8187 `charset'lang'pct`。 */
function decodeExtValue(value: string, hints: NameHints): string | null {
  const m = EXT_VALUE_RE.exec(value);
  if (!m) return null;
  return decodeWithCharset(
    trimAscii(m[1]),
    percentDecodeLatin1(m[2]),
    hints,
    true,
  );
}

/** RFC 2231 续行：序号须从 0 连续；charset 取第 0 段。 */
function decodeContinuation(
  params: DispositionParam[],
  hints: NameHints,
): string | null {
  const segs = new Map<number, { value: string; star: boolean }>();
  for (const p of params) {
    const m = /^filename\*(\d+)(\*)?$/.exec(p.name);
    if (!m) continue;
    const n = Number(m[1]);
    if (!segs.has(n)) segs.set(n, { value: p.value, star: !!m[2] });
  }
  if (segs.size === 0) return null;
  let charset = "";
  const chunks: Uint8Array[] = [];
  for (let i = 0; i < segs.size; i++) {
    const seg = segs.get(i);
    if (!seg) return null;
    if (seg.star) {
      let rest = seg.value;
      if (i === 0) {
        const m = EXT_VALUE_RE.exec(seg.value);
        if (!m) return null;
        charset = trimAscii(m[1]);
        rest = m[2];
      }
      chunks.push(percentDecodeLatin1(rest));
    } else {
      chunks.push(latin1ToBytes(seg.value));
    }
  }
  return decodeWithCharset(charset, concatBytes(chunks), hints, true);
}

const ENCODED_WORD_RE = /=\?([^?\s]+)\?([bBqQ])\?([^?\s]*)\?=/g;

function decodeQ(text: string): Uint8Array {
  const out: number[] = [];
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "_") out.push(0x20);
    else if (c === "=" && /^[0-9a-fA-F]{2}$/.test(text.slice(i + 1, i + 3))) {
      out.push(parseInt(text.slice(i + 1, i + 3), 16));
      i += 2;
    } else out.push(text.charCodeAt(i) & 0xff);
  }
  return Uint8Array.from(out);
}

/** RFC 2047；没有 encoded-word 或任一 encoded-word 解码失败返回 `null`。 */
function decodeEncodedWords(value: string, hints: NameHints): string | null {
  const matches = [...value.matchAll(ENCODED_WORD_RE)];
  if (matches.length === 0) return null;
  let out = "";
  let last = 0;
  let prevWord = false;
  for (const m of matches) {
    const start = m.index ?? 0;
    const gap = value.slice(last, start);
    // 相邻 encoded-word 之间的空白丢弃
    if (gap && !(prevWord && /^[ \t]+$/.test(gap))) {
      out += decodeLegacyBytes(latin1ToBytes(gap), hints);
    }
    let payload: Uint8Array;
    if (m[2] === "b" || m[2] === "B") {
      try {
        payload = latin1ToBytes(atob(m[3]));
      } catch {
        return null;
      }
    } else {
      payload = decodeQ(m[3]);
    }
    // RFC 2231 语言后缀 `charset*lang`
    const decoded = decodeWithCharset(m[1].split("*")[0], payload, hints, false);
    if (decoded === null) return null;
    out += decoded;
    last = start + m[0].length;
    prevWord = true;
  }
  const tail = value.slice(last);
  if (tail) out += decodeLegacyBytes(latin1ToBytes(tail), hints);
  return out;
}

/** `filename=` 的值：RFC 2047 → 合法 `%XX` 百分号解码 → 旧式字节解码。 */
function decodePlainValue(value: string, hints: NameHints): string {
  const words = decodeEncodedWords(value, hints);
  if (words !== null) return words;
  const original = decodeLegacyBytes(latin1ToBytes(value), hints);
  if (/%[0-9a-fA-F]{2}/.test(value)) {
    const decoded = decodeLegacyBytes(percentDecodeLatin1(value), hints);
    if (decoded !== original) return decoded;
  }
  return original;
}

function nonEmpty(s: string | null): string | null {
  const t = s?.trim();
  return t ? t : null;
}

/**
 * 从 Content-Disposition 头解析文件名（未 sanitize，已 trim；无名返回 ""）。
 *
 * 优先级：`filename*` > `filename*N` 续行 > `filename`（含 RFC 2047 / 百分号 /
 * 原始字节）。`filename*` 未知字符集或严格解码失败时忽略该参数，继续下一级。
 *
 * @param byteCarrier `disposition` 是否为字节载体（见 `readDispositionHeader`）
 * @param hints 下载 URL 主机与发起页字符集，仅用于未声明字符集的字节
 */
export function parseContentDispositionFilename(
  disposition: string,
  byteCarrier = false,
  hints: NameHints = {},
): string {
  if (!disposition) return "";
  const bytes = byteCarrier
    ? latin1ToBytes(disposition)
    : textEncoder.encode(disposition);

  const params: DispositionParam[] = [];
  for (const seg of splitParams(bytesToLatin1(bytes))) {
    const eq = seg.indexOf("=");
    if (eq < 0) continue;
    params.push({
      name: trimAscii(seg.slice(0, eq)).toLowerCase(),
      value: unquote(seg.slice(eq + 1)),
    });
  }

  const star = params.find((p) => p.name === "filename*");
  if (star) {
    const decoded = nonEmpty(decodeExtValue(star.value, hints));
    if (decoded) return decoded;
  }
  const continued = nonEmpty(decodeContinuation(params, hints));
  if (continued) return continued;
  const plain = params.find((p) => p.name === "filename");
  if (plain) return nonEmpty(decodePlainValue(plain.value, hints)) ?? "";
  return "";
}

/** 动态页面/脚本端点扩展名：这类名字不是真实文件名，应交给引擎探测。 */
const SCRIPT_EXTS: Record<string, true> = {
  php: true,
  asp: true,
  aspx: true,
  jsp: true,
  jspx: true,
  cgi: true,
  pl: true,
  shtml: true,
  html: true,
  htm: true,
  do: true,
  action: true,
};

const PLAUSIBLE_EXT_RE = /\.([a-zA-Z0-9]{1,10})$/;

/** 文件名是否以动态页面/脚本端点扩展名结尾（php/aspx/...）。 */
export function hasScriptExtension(name: string): boolean {
  const m = name.match(PLAUSIBLE_EXT_RE);
  return (
    !!m && Object.prototype.hasOwnProperty.call(SCRIPT_EXTS, m[1].toLowerCase())
  );
}

/**
 * 判断一个文件名是否看起来像真实的文件名（而非 CDN hash / UUID / 无意义路径段）
 *
 * 真实文件名特征：有常见扩展名，如 "report.pdf", "video.mp4"
 * 非真实文件名：纯 hash "a1b2c3d4e5f6", UUID, 无扩展名 "download",
 *               脚本端点 "attachment.php"
 */
export function looksLikeRealFilename(name: string): boolean {
  return PLAUSIBLE_EXT_RE.test(name) && !hasScriptExtension(name);
}

/**
 * MIME → 扩展名（移植自 native/engine/src/downloader.rs `mime_to_ext`）。
 * 跳过 text/html、application/xhtml+xml、application/octet-stream 与未知类型，
 * 这些类型不能说明真实文件类型，返回 `null`。
 */
const MIME_EXT: Record<string, string> = {
  "application/pdf": "pdf",
  "application/zip": "zip",
  "application/x-gzip": "gz",
  "application/gzip": "gz",
  "application/x-tar": "tar",
  "application/x-bzip2": "bz2",
  "application/x-xz": "xz",
  "application/x-7z-compressed": "7z",
  "application/x-rar-compressed": "rar",
  "application/vnd.rar": "rar",
  "application/json": "json",
  "application/xml": "xml",
  "text/xml": "xml",
  "application/javascript": "js",
  "text/javascript": "js",
  "application/wasm": "wasm",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "xlsx",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document":
    "docx",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation":
    "pptx",
  "application/msword": "doc",
  "application/vnd.ms-excel": "xls",
  "application/vnd.ms-powerpoint": "ppt",
  "application/x-iso9660-image": "iso",
  "application/x-msdownload": "exe",
  "application/x-dosexec": "exe",
  "application/vnd.android.package-archive": "apk",
  "application/java-archive": "jar",
  "application/x-shockwave-flash": "swf",
  "application/x-debian-package": "deb",
  "application/x-rpm": "rpm",
  "application/x-msi": "msi",
  "application/vnd.apple.installer+xml": "pkg",
  "text/css": "css",
  "text/csv": "csv",
  "text/plain": "txt",
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/svg+xml": "svg",
  "image/bmp": "bmp",
  "image/x-icon": "ico",
  "image/vnd.microsoft.icon": "ico",
  "image/tiff": "tiff",
  "image/avif": "avif",
  "audio/mpeg": "mp3",
  "audio/ogg": "ogg",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "audio/flac": "flac",
  "audio/aac": "aac",
  "audio/mp4": "m4a",
  "audio/x-m4a": "m4a",
  "audio/webm": "weba",
  "video/mp4": "mp4",
  "video/webm": "webm",
  "video/x-matroska": "mkv",
  "video/x-msvideo": "avi",
  "video/quicktime": "mov",
  "video/x-flv": "flv",
  "video/mp2t": "ts",
  "video/3gpp": "3gp",
  "font/woff": "woff",
  "font/woff2": "woff2",
  "font/ttf": "ttf",
  "application/x-font-ttf": "ttf",
  "font/otf": "otf",
};

export function mimeToExtension(
  mimeType: string | undefined,
): string | null {
  if (!mimeType) return null;
  const ct = mimeType.split(";")[0].trim().toLowerCase();
  return Object.prototype.hasOwnProperty.call(MIME_EXT, ct)
    ? MIME_EXT[ct]
    : null;
}

function appendMimeExtension(name: string, mimeType?: string): string {
  if (!name || PLAUSIBLE_EXT_RE.test(name)) return name;
  const ext = mimeToExtension(mimeType);
  return ext ? `${name}.${ext}` : name;
}

function urlLastSegment(url: string): string {
  try {
    const segments = new URL(url).pathname.split("/").filter(Boolean);
    const last = segments[segments.length - 1];
    return last ? decodeURIComponent(last) : "";
  } catch {
    return "";
  }
}

/**
 * URL 查询里的 `response-content-disposition`（S3/OSS/COS/GCS）/ `rscd`（Azure SAS，
 * GitHub release 资产即此类）→ 文件名（未 sanitize；无名返回 ""）。服务器会原样回显该值，
 * 是确定性证据。与引擎 `naming::query_disposition_name` 同一规则：参数名不区分大小写，
 * 值按 `application/x-www-form-urlencoded` 解码（`+` = 空格、`%XX` = 字节）后按
 * Content-Disposition 解析。
 */
export function queryDispositionFilename(url: string): string {
  const queryStart = url.indexOf("?");
  if (queryStart < 0) return "";
  const query = url.slice(queryStart + 1).split("#")[0];
  const hints: NameHints = { host: urlHostOf(url) };
  for (const pair of query.split("&")) {
    const eq = pair.indexOf("=");
    if (eq < 0) continue;
    const key = pair.slice(0, eq).toLowerCase();
    if (key !== "response-content-disposition" && key !== "rscd") continue;
    const bytes = percentDecodeLatin1(pair.slice(eq + 1).replace(/\+/g, " "));
    const name = parseContentDispositionFilename(bytesToLatin1(bytes), true, hints);
    if (name) return name;
  }
  return "";
}

function urlHostOf(url: string): string | undefined {
  try {
    return new URL(url).hostname || undefined;
  } catch {
    return undefined;
  }
}

/**
 * 从浏览器的 downloadItem.filename（本地保存路径）和 URL 中提取有意义的文件名。
 *
 * 策略：
 * 1. 浏览器文件名有合法扩展名 → 使用；
 * 2. URL 查询里的 `response-content-disposition` / `rscd` 给出的名字 → 使用
 *    （预签名 CDN 链接的路径末段是对象 ID，见 `queryDispositionFilename`）；
 * 3. URL 最后一段有合法扩展名 → 使用；
 * 4. 放宽：浏览器文件名 / URL 最后一段（无扩展名也可，如 "download-no-header"）；
 * 5. 以上任何一步选中的名字若是动态页面/脚本端点（attachment.php 等），或
 *    无法确定 → 返回 ""，交给引擎探测服务器的 Content-Disposition；
 * 6. 选中的名字没有合理扩展名且 `mimeType` 可映射 → 追加对应扩展名
 *    （如 X.com 图片 `/media/<id>?format=jpg` + image/jpeg → `<id>.jpg`）。
 */
export function extractCleanFilename(
  browserFilename: string | undefined,
  url: string,
  mimeType?: string,
): string {
  const browserBase = browserFilename
    ? browserFilename.split(/[/\\]/).pop() || ""
    : "";
  if (browserBase && looksLikeRealFilename(browserBase)) {
    return browserBase;
  }

  const queryName = queryDispositionFilename(url);
  if (queryName && !hasScriptExtension(queryName)) {
    return queryName;
  }

  const segment = urlLastSegment(url);
  if (segment && looksLikeRealFilename(segment)) {
    return segment;
  }

  // 放宽：无扩展名的名字也接受，但脚本端点一律交给引擎探测
  if (browserBase) {
    return hasScriptExtension(browserBase)
      ? ""
      : appendMimeExtension(browserBase, mimeType);
  }
  if (segment) {
    return hasScriptExtension(segment)
      ? ""
      : appendMimeExtension(segment, mimeType);
  }
  return "";
}

/**
 * 决定发送给桌面端的文件名：
 * 1. Content-Disposition 解出的名字有真实扩展名 → 直接使用；
 * 2. 否则用 `extractCleanFilename(browserFilename, url, mimeType)`；
 * 3. 仍为空而头里的名字非空且不是脚本端点 → 使用头里的名字；
 * 4. 否则 ""（引擎自行探测）。
 */
export function resolveDownloadFilename(
  dispositionFilename: string | undefined,
  browserFilename: string | undefined,
  url: string,
  mimeType?: string,
): string {
  const disposition = dispositionFilename?.trim() || "";
  if (disposition && looksLikeRealFilename(disposition)) return disposition;
  const clean = extractCleanFilename(browserFilename, url, mimeType);
  if (clean) return clean;
  if (disposition && !hasScriptExtension(disposition)) return disposition;
  return "";
}
