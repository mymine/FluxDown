// YouTube 会话：Cookie 罐、网页 ytcfg、InnerTube API 调用（对齐 yt-dlp `_base.py`
// generate_api_headers / _call_api 与 `_video.py` _extract_player_response）。
import { BROWSER_UA, CLIENTS, EMBED_URL } from "./clients.js";
import { CookieJar, sidAuthorization } from "./cookies.js";

export const ORIGIN = "https://www.youtube.com";

/** 网页里第一处 `ytcfg.set({...});` 的 JSON（yt-dlp 同一正则）；没有或解析失败 → null。 */
export function extractYtcfg(html) {
  const m = /ytcfg\.set\s*\(\s*({.+?})\s*\)\s*;/.exec(html || "");
  if (!m) return null;
  try {
    const cfg = JSON.parse(m[1]);
    return cfg && typeof cfg === "object" ? cfg : null;
  } catch {
    return null;
  }
}

function shortUrl(url) {
  return String(url).replace(/\?.*$/, "");
}

export class Session {
  /**
   * @param {string} cookieText 任务 Cookie 头或备用 Cookie（头 / Netscape 均可）
   * @param {() => number} now 毫秒时钟（测试注入）
   */
  constructor(cookieText, now = Date.now) {
    this.now = now;
    this.jar = CookieJar.parse(cookieText);
    this.authenticated = this.jar.isAuthenticated();
    this.jar.applyDefaults();
    /**
     * 登录态时的会话 ytcfg（会话序号、账号 id、visitorData、播放器地址），取自嵌入页：
     * 这些字段与播放页 ytcfg 相同，嵌入页约 150 KB，播放页约 1.5 MB（flux.fetch 不解压）。
     */
    this.ytcfg = null;
    /** videoId → 嵌入页 ytcfg 的 Promise（会话配置与 web_embedded 客户端共用一次下载）。 */
    this.embedConfigs = new Map();
  }

  /** flux.fetch 包装：带上 Cookie 罐、吸收 Set-Cookie、拒收被截断的响应体。 */
  async request({ method = "GET", url, headers = {}, body }) {
    const cookie = this.jar.header();
    const res = await flux.fetch({
      method,
      url,
      headers: cookie ? { ...headers, Cookie: cookie } : headers,
      ...(body === undefined ? {} : { body }),
    });
    this.jar.absorb(res.headers ? res.headers["set-cookie"] : "", this.now());
    if (res.truncated) throw new Error(`响应过大被截断：${shortUrl(url)}`);
    return res;
  }

  /** GET 网页（桌面浏览器 UA），非 200 → null。 */
  async page(url, extraHeaders = {}) {
    const res = await this.request({
      url,
      headers: { "User-Agent": BROWSER_UA, "Accept-Language": "en-US,en;q=0.9", ...extraHeaders },
    });
    return res.status === 200 ? res.body : null;
  }

  /** 登录态：取会话 ytcfg（会话序号与账号 id 只在网页里）。失败不致命。 */
  async loadSessionConfig(videoId) {
    this.ytcfg = await this.embedConfig(videoId);
    return this.ytcfg;
  }

  visitorData() {
    const cfg = this.ytcfg;
    return (cfg && (cfg.VISITOR_DATA || (cfg.INNERTUBE_CONTEXT && cfg.INNERTUBE_CONTEXT.client && cfg.INNERTUBE_CONTEXT.client.visitorData))) || "";
  }

  /** yt-dlp `_generate_cookie_auth_headers`：有 SAPISID 系 Cookie 才生成。 */
  authHeaders() {
    const cfg = this.ytcfg || {};
    const [first, second] = String(cfg.DATASYNC_ID || "").split("||");
    const delegated = cfg.DELEGATED_SESSION_ID || (second ? first : "");
    const user = cfg.USER_SESSION_ID || (second ? second : first) || "";
    const parsedIndex = Number.parseInt(String(cfg.SESSION_INDEX ?? ""), 10);
    const index = Number.isInteger(parsedIndex) ? parsedIndex : null;
    const headers = {};
    if (delegated) headers["X-Goog-PageId"] = delegated;
    if (delegated || index !== null) headers["X-Goog-AuthUser"] = String(index ?? 0);
    const auth = sidAuthorization(this.jar, ORIGIN, user, Math.round(this.now() / 1000));
    if (auth) {
      headers.Authorization = auth;
      headers["X-Origin"] = ORIGIN;
    }
    if (cfg.LOGGED_IN === true) headers["X-Youtube-Bootstrap-Logged-In"] = "true";
    return headers;
  }

  /**
   * POST /youtubei/v1/<endpoint>。`client` 取自 clients.js；`ytcfg` 为该客户端自己的网页配置
   * （仅 web_embedded 下载嵌入页），其 INNERTUBE_CONTEXT 优先于内置上下文。
   */
  async api(client, endpoint, payload, ytcfg = null) {
    const baseContext = ytcfg && ytcfg.INNERTUBE_CONTEXT && typeof ytcfg.INNERTUBE_CONTEXT === "object" ? ytcfg.INNERTUBE_CONTEXT : {};
    const clientContext = { ...(baseContext.client || client.context), hl: "en", timeZone: "UTC", utcOffsetMinutes: 0 };
    const context = { ...baseContext, client: clientContext };
    if (client.embedded) context.thirdParty = { ...(context.thirdParty || {}), embedUrl: EMBED_URL };
    const headers = {
      "content-type": "application/json",
      "X-YouTube-Client-Name": String(client.id),
      "X-YouTube-Client-Version": (ytcfg && ytcfg.INNERTUBE_CLIENT_VERSION) || clientContext.clientVersion,
      Origin: ORIGIN,
      "User-Agent": clientContext.userAgent || client.context.userAgent || BROWSER_UA,
      ...this.authHeaders(),
    };
    const visitor = this.visitorData();
    if (visitor) headers["X-Goog-Visitor-Id"] = visitor;
    const res = await this.request({
      method: "POST",
      url: `${ORIGIN}/youtubei/v1/${endpoint}?prettyPrint=false`,
      headers,
      body: JSON.stringify({ context, ...payload }),
    });
    let data = null;
    try {
      data = JSON.parse(res.body);
    } catch {
      data = null;
    }
    if (res.status !== 200) {
      const message = data && data.error && data.error.message ? `：${data.error.message}` : "";
      throw new Error(`YouTube ${endpoint} 接口返回 HTTP ${res.status}${message}`);
    }
    if (!data || typeof data !== "object") throw new Error(`YouTube ${endpoint} 接口返回的不是 JSON`);
    return data;
  }

  /** 嵌入页 ytcfg（INNERTUBE_CONTEXT、encryptedHostFlags、会话字段）；同一视频只下载一次。失败 → null。 */
  embedConfig(videoId) {
    let pending = this.embedConfigs.get(videoId);
    if (!pending) {
      pending = this.page(`${ORIGIN}/embed/${videoId}?html5=1`, {
        "User-Agent": CLIENTS.web_embedded.context.userAgent,
        Referer: EMBED_URL,
      }).then(extractYtcfg);
      this.embedConfigs.set(videoId, pending);
    }
    return pending;
  }

  /** 单个客户端的 /player 响应；`sts` 为播放器的 signatureTimestamp。 */
  async playerResponse(client, videoId, sts) {
    const ytcfg = client.embedded ? await this.embedConfig(videoId) : null;
    const contentPlaybackContext = { html5Preference: "HTML5_PREF_WANTS" };
    if (sts) contentPlaybackContext.signatureTimestamp = sts;
    const flags =
      ytcfg &&
      ytcfg.WEB_PLAYER_CONTEXT_CONFIGS &&
      ytcfg.WEB_PLAYER_CONTEXT_CONFIGS.WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER &&
      ytcfg.WEB_PLAYER_CONTEXT_CONFIGS.WEB_PLAYER_CONTEXT_CONFIG_ID_EMBEDDED_PLAYER.encryptedHostFlags;
    if (flags) contentPlaybackContext.encryptedHostFlags = flags;
    return this.api(
      client,
      "player",
      { videoId, playbackContext: { contentPlaybackContext }, contentCheckOk: true, racyCheckOk: true },
      ytcfg,
    );
  }
}
