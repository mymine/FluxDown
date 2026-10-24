// Cookie 罐与登录态请求头（对齐 yt-dlp `_base.py`：PREF / SOCS 初始化、SAPISIDHASH）。
import { parseQuery } from "./query.js";
import { sha1Hex } from "./sha1.js";

const YOUTUBE_DOMAIN_RE = /(^|\.)youtube\.com$/i;

function looksNetscape(text) {
  return /^# (Netscape )?HTTP Cookie File/m.test(text) || /^[^\t\n]+\t(TRUE|FALSE)\t[^\t\n]*\t(TRUE|FALSE)\t/m.test(text);
}

/** Netscape cookies.txt → [name, value]（只取 youtube.com 域；`#HttpOnly_` 前缀行也算）。 */
function parseNetscape(text) {
  const pairs = [];
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.startsWith("#HttpOnly_") ? rawLine.slice("#HttpOnly_".length) : rawLine;
    if (!line || line.startsWith("#")) continue;
    const cols = line.split("\t");
    if (cols.length < 7) continue;
    if (!YOUTUBE_DOMAIN_RE.test(cols[0].replace(/^\./, ""))) continue;
    pairs.push([cols[5], cols.slice(6).join("\t")]);
  }
  return pairs;
}

/** Cookie 头（`k=v; k2=v2`）→ [name, value]。 */
function parseHeader(text) {
  const pairs = [];
  for (const part of text.split(";")) {
    const kv = part.trim();
    const eq = kv.indexOf("=");
    if (eq <= 0) continue;
    pairs.push([kv.slice(0, eq).trim(), kv.slice(eq + 1).trim()]);
  }
  return pairs;
}

/** 有序 Cookie 罐：后写覆盖前写，`header()` 输出请求头。 */
export class CookieJar {
  constructor(pairs = []) {
    this.map = new Map();
    for (const [name, value] of pairs) this.set(name, value);
  }

  static parse(text) {
    const raw = String(text || "").trim();
    if (!raw) return new CookieJar();
    return new CookieJar(looksNetscape(raw) ? parseNetscape(raw) : parseHeader(raw));
  }

  get(name) {
    return this.map.get(name);
  }

  set(name, value) {
    if (name) this.map.set(name, value);
  }

  /** 吸收响应的 Set-Cookie（宿主把多个同名头按换行拼接）；过期 / Max-Age=0 视为删除。 */
  absorb(setCookie, nowMs) {
    for (const line of String(setCookie || "").split("\n")) {
      const [pair, ...attrs] = line.split(";");
      const eq = pair.indexOf("=");
      if (eq <= 0) continue;
      const name = pair.slice(0, eq).trim();
      const value = pair.slice(eq + 1).trim();
      const expired = attrs.some((a) => {
        const [k, v = ""] = a.split("=", 2).map((s) => s.trim());
        if (/^max-age$/i.test(k)) return Number(v) <= 0;
        if (/^expires$/i.test(k)) {
          const t = Date.parse(v);
          return Number.isFinite(t) && t <= nowMs;
        }
        return false;
      });
      if (expired) this.map.delete(name);
      else this.set(name, value);
    }
  }

  header() {
    return [...this.map].map(([k, v]) => `${k}=${v}`).join("; ");
  }

  /** yt-dlp 的登录判据：LOGIN_INFO + 任一 SAPISID 系 Cookie。 */
  isAuthenticated() {
    return this.map.has("LOGIN_INFO") && Boolean(this.sapisid() || this.get("__Secure-1PAPISID"));
  }

  sapisid() {
    // SAPISID 缺失时 YouTube 自己也回退到 __Secure-3PAPISID。
    return this.get("SAPISID") || this.get("__Secure-3PAPISID");
  }

  /**
   * 首次请求前的偏好：PREF 合并 hl=en、tz=UTC（解析英文报错与 UTC 时间）；未登录且未拒绝
   * 同意时写 SOCS=CAI，避免欧盟地区被重定向到同意页。
   */
  applyDefaults() {
    const pref = parseQuery(this.get("PREF") || "");
    pref.hl = "en";
    pref.tz = "UTC";
    this.set(
      "PREF",
      Object.keys(pref)
        .map((k) => `${encodeURIComponent(k)}=${encodeURIComponent(pref[k])}`)
        .join("&"),
    );
    if (this.isAuthenticated()) return;
    const socs = this.get("SOCS");
    if (!socs || socs.startsWith("CAA")) this.set("SOCS", "CAI");
  }
}

function sidHash(scheme, sid, origin, userSessionId, nowSec) {
  const parts = [];
  if (userSessionId) parts.push(userSessionId);
  parts.push(String(nowSec), sid, origin);
  const hash = sha1Hex(parts.join(" "));
  return `${scheme} ${[String(nowSec), hash, ...(userSessionId ? ["u"] : [])].join("_")}`;
}

/** `Authorization` 头：SAPISIDHASH / SAPISID1PHASH / SAPISID3PHASH（有哪个 Cookie 出哪个）。 */
export function sidAuthorization(jar, origin, userSessionId, nowSec) {
  const out = [];
  for (const [scheme, sid] of [
    ["SAPISIDHASH", jar.sapisid()],
    ["SAPISID1PHASH", jar.get("__Secure-1PAPISID")],
    ["SAPISID3PHASH", jar.get("__Secure-3PAPISID")],
  ]) {
    if (sid) out.push(sidHash(scheme, sid, origin, userSessionId, nowSec));
  }
  return out.length ? out.join(" ") : null;
}
