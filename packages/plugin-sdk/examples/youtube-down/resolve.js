// YouTube 下载插件的 resolver（classic script，入口挂 globalThis.resolve；`flux` 由引擎注入）。
//
// 解析经 FluxDown 的 yt-dlp 组件完成（`flux.ytdlp`，`-J` 只取元数据不下载），插件只负责：
//   1. 把各种 YouTube 链接归一成视频 / 播放列表；
//   2. 从 yt-dlp 的格式表挑出可直接 HTTP 下载的档位，组成「视频轨 + 同容器音频轨」交回引擎，
//      引擎做多段并发下载、断点续传，下载完成后合并音视频；
//   3. 播放列表返回清单（manifest），引擎按条目裂变子任务，每个子任务再走一次二段解析。
//
// 运行环境是 QuickJS：没有 DOM、没有 URL 全局，链接解析用正则。

const VIDEO_ID_RE = /^[A-Za-z0-9_-]{11}$/;
const LIST_ID_RE = /^[A-Za-z0-9_-]{2,64}$/;
/** 单次 yt-dlp 调用的超时；resolver 整体墙钟顶为 30 s（manifest timeoutMs），留出余量给 JSON 处理。 */
const YTDLP_TIMEOUT_MS = 27000;
/** 清晰度弹框最多列出的视频档数（另加 1 个仅音频档）。 */
const MAX_VIDEO_OPTIONS = 9;
const PRESETS = ["best", "2160", "1440", "1080", "720", "480", "360", "audio"];
/** 播放列表条目的可选规格；首项为引擎自动全选时使用的默认规格。 */
const ITEM_PRESETS = [
  { id: "best", label: "最高画质" },
  { id: "1080", label: "≤ 1080p" },
  { id: "720", label: "≤ 720p" },
  { id: "480", label: "≤ 480p" },
  { id: "audio", label: "仅音频" },
];
const UNAVAILABLE_TITLES = ["[Private video]", "[Deleted video]", "[Unavailable video]"];

// ---------------------------------------------------------------------------
// 链接归一
// ---------------------------------------------------------------------------

function queryParam(query, name) {
  const m = new RegExp(`(?:^|&)${name}=([^&#]*)`).exec(query);
  if (!m) return "";
  try {
    return decodeURIComponent(m[1].replace(/\+/g, " "));
  } catch {
    return "";
  }
}

/**
 * 解析 YouTube 链接：返回 `{ videoId }`、`{ playlistId }` 或 null。
 * `watch?v=…&list=…` 视为单个视频（用户正在看的那一个）；只有 `/playlist?list=…` 才是整个列表。
 */
function parseTarget(url) {
  const m = /^https?:\/\/([^/?#]+)([^?#]*)(?:\?([^#]*))?/i.exec(url || "");
  if (!m) return null;
  const host = m[1].toLowerCase().replace(/:\d+$/, "");
  const path = m[2] || "/";
  const query = m[3] || "";
  const segments = path.split("/").filter(Boolean);
  const video = (id) => (VIDEO_ID_RE.test(id) ? { videoId: id } : null);

  if (host === "youtu.be") return video(segments[0] || "");
  if (!/(^|\.)youtube(-nocookie)?\.com$/.test(host)) return null;
  const head = (segments[0] || "").toLowerCase();
  if (head === "watch") return video(queryParam(query, "v"));
  if (head === "shorts" || head === "live" || head === "embed") return video(segments[1] || "");
  if (head === "playlist") {
    const list = queryParam(query, "list");
    return LIST_ID_RE.test(list) ? { playlistId: list } : null;
  }
  return null;
}

/** 二段解析的条目标识 `<videoId>@<preset>`（清单条目 id + 规格 id）。 */
function parseItem(resolverItem) {
  if (!resolverItem) return null;
  const at = resolverItem.indexOf("@");
  const videoId = at >= 0 ? resolverItem.slice(0, at) : resolverItem;
  const preset = at >= 0 ? resolverItem.slice(at + 1) : "best";
  if (!VIDEO_ID_RE.test(videoId)) throw new Error(`无效的播放列表条目: ${resolverItem}`);
  return { videoId, preset: PRESETS.includes(preset) ? preset : "best" };
}

// ---------------------------------------------------------------------------
// yt-dlp 调用
// ---------------------------------------------------------------------------

function jsRuntimeArgs() {
  const runtime = String(flux.settings.jsRuntime || "auto");
  // yt-dlp 只默认启用 deno，且总用「已启用且可用」中优先级最高的（deno > node > quickjs > bun）。
  if (runtime === "auto") return ["--js-runtimes", "node", "--js-runtimes", "quickjs", "--js-runtimes", "bun"];
  if (runtime === "deno") return [];
  return ["--no-js-runtimes", "--js-runtimes", runtime];
}

function looksNetscape(text) {
  return /^#\s*(Netscape )?HTTP Cookie File/im.test(text) || /\t/.test(text);
}

/** `k=v; k2=v2` 请求头 → Netscape cookies.txt（域固定为 .youtube.com）。 */
function cookieHeaderToNetscape(header) {
  const lines = ["# Netscape HTTP Cookie File"];
  for (const part of header.split(";")) {
    const kv = part.trim();
    const eq = kv.indexOf("=");
    if (eq <= 0) continue;
    lines.push([".youtube.com", "TRUE", "/", "TRUE", "0", kv.slice(0, eq).trim(), kv.slice(eq + 1).trim()].join("\t"));
  }
  return lines.length > 1 ? `${lines.join("\n")}\n` : "";
}

/** 任务携带的 Cookie（浏览器扩展发起的下载）优先，其次是设置里的备用 Cookie。 */
function cookieFileContent(ctx) {
  const fromTask = flux.settings.useTaskCookies !== false ? (ctx.cookies || "").trim() : "";
  if (fromTask) return cookieHeaderToNetscape(fromTask);
  const fallback = String(flux.settings.cookies || "").trim();
  if (!fallback) return "";
  return looksNetscape(fallback) ? `${fallback}\n` : cookieHeaderToNetscape(fallback);
}

function friendlyError(stderr, withCookies) {
  // 只带 yt-dlp 的最后一条 ERROR 行（截断的 stderr 尾巴常从半个单词开始）。
  const lines = stderr.trim().split("\n");
  const detail = (lines.filter((l) => l.startsWith("ERROR:")).pop() || lines.pop() || "").slice(0, 300);
  if (/confirm you.?re not a bot|Sign in to confirm/i.test(stderr)) {
    // yt-dlp 原文建议的 --cookies-from-browser 在 FluxDown 里不可用，不附带。
    return withCookies
      ? "YouTube 要求验证「不是机器人」，当前 Cookie 未通过。请在浏览器重新登录 YouTube 后再从浏览器发起下载。"
      : "YouTube 要求验证「不是机器人」。请登录 YouTube 后从浏览器扩展发起下载（会自动携带 Cookie），或在插件设置里填写备用 Cookie。";
  }
  // 带登录 Cookie 时缺运行时的最终 ERROR 是 "The page needs to be reloaded"，原因只在前面的 WARNING 里。
  if (/n challenge solving failed|No supported JavaScript runtime|Only images are available|nsig extraction failed/i.test(stderr)) {
    return `yt-dlp 找不到可用的 JavaScript 运行时，无法求解 YouTube 签名。请安装 Deno、Node.js 或 Bun，并确保在终端里能直接运行。${detail}`;
  }
  if (/Private video|members-only|Join this channel|age.?restricted|confirm your age|inappropriate for some users/i.test(stderr)) {
    return `视频受限（私享 / 会员 / 年龄限制），需要有权限的登录 Cookie。${detail}`;
  }
  if (/Video unavailable|This video is not available|has been removed|The playlist does not exist/i.test(stderr)) {
    return `视频或播放列表不可用：${detail}`;
  }
  return detail ? `yt-dlp 解析失败：${detail}` : "yt-dlp 没有返回任何数据";
}

async function runYtdlp(ctx, args, url) {
  if (!flux.ytdlp) throw new Error("缺少 ytdlp 权限（flux.ytdlp 不可用）");
  const avail = await flux.ytdlp.available();
  if (!avail.available) throw new Error("yt-dlp 组件未安装，请在 App「组件」页安装 yt-dlp（音视频合并另需 ffmpeg）");

  // 不加 --no-warnings：缺 JS 运行时等根因只出现在 WARNING 里，friendlyError 靠它们归因。
  const full = ["-J", ...jsRuntimeArgs(), ...args];
  const clients = String(flux.settings.playerClients || "").trim();
  if (clients) full.push("--extractor-args", `youtube:player_client=${clients}`);
  const cookies = cookieFileContent(ctx);
  // 同时解析的任务（播放列表裂变）各用一个文件，避免互相覆盖；用完即删。
  const cookieFile = cookies ? `cookies-${Date.now()}-${Math.floor(Math.random() * 1e9)}.txt` : "";
  if (cookieFile) {
    await flux.fs.writeFile(cookieFile, cookies);
    full.push("--cookies", cookieFile);
  }
  full.push(url);

  let out;
  try {
    out = await flux.ytdlp.run({ args: full, timeoutMs: YTDLP_TIMEOUT_MS });
  } finally {
    if (cookieFile) await flux.fs.remove(cookieFile);
  }
  if (out.timedOut) throw new Error(`yt-dlp 解析超时：${url}`);
  const raw = (out.stdout || "").trim();
  // 风控时 yt-dlp 可能以 0 退出却输出 null，必须同时校验输出。
  if (out.code !== 0 || !raw || raw === "null") throw new Error(friendlyError(out.stderr || "", cookieFile !== ""));
  if (out.truncatedStdout) throw new Error("yt-dlp 输出过大被截断");
  const info = JSON.parse(raw);
  if (!info || typeof info !== "object") throw new Error(friendlyError(out.stderr || "", cookieFile !== ""));
  return info;
}

// ---------------------------------------------------------------------------
// 格式挑选
// ---------------------------------------------------------------------------

function sanitizeFileName(name, fallback) {
  const clean = String(name || "")
    .replace(/[\\/:*?"<>|\u0000-\u001f]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^\.+/, "")
    .slice(0, 120)
    .trim();
  return clean || fallback;
}

const hasVideo = (f) => typeof f.vcodec === "string" && f.vcodec !== "none";
const hasAudio = (f) => typeof f.acodec === "string" && f.acodec !== "none";
const sizeOf = (f) => Number(f.filesize) || Number(f.filesize_approx) || 0;

/** 只保留引擎能按 HTTP Range 直接下载的格式（排除 HLS / 故事板 / DRM / 动态范围压缩音轨）。 */
function usableFormats(formats) {
  return (Array.isArray(formats) ? formats : []).filter(
    (f) =>
      f &&
      typeof f.url === "string" &&
      /^https?:\/\//.test(f.url) &&
      (f.protocol === "https" || f.protocol === "http") &&
      !f.has_drm &&
      !/-drc$/.test(String(f.format_id || "")) &&
      (hasVideo(f) || hasAudio(f)),
  );
}

/** 清晰度取短边：竖屏视频（Shorts）的 1080×1920 也算 1080p。 */
function pOf(f) {
  const w = Number(f.width) || 0;
  const h = Number(f.height) || 0;
  return w > 0 && h > 0 ? Math.min(w, h) : h;
}

function codecName(codec) {
  const c = String(codec || "").toLowerCase();
  if (c.startsWith("avc1") || c.startsWith("h264")) return "H.264";
  if (c.startsWith("av01")) return "AV1";
  if (c.startsWith("vp9") || c.startsWith("vp09")) return "VP9";
  if (c.startsWith("mp4a")) return "AAC";
  if (c.startsWith("opus")) return "Opus";
  return c.split(".")[0].toUpperCase();
}

function videoScore(f, codec) {
  const name = codecName(f.vcodec);
  const rank =
    codec === "quality"
      ? { AV1: 3, VP9: 2, "H.264": 1 }[name] || 0
      : { "H.264": 3, AV1: 2, VP9: 1 }[name] || 0;
  const sdr = !f.dynamic_range || f.dynamic_range === "SDR" ? 1 : 0;
  return ((sdr * 10 + rank) * 1000 + (Number(f.fps) || 0)) * 1e6 + (Number(f.tbr) || 0);
}

/** 音轨评分：原声语言优先；同客户端（UA 相同，直链与请求头配套）优先；兼容优先时 AAC 优先；再比码率。 */
function audioScore(f, preferExt, userAgent) {
  const lang = Number(f.language_preference) || 0;
  const sameClient = userAgent && f.http_headers && f.http_headers["User-Agent"] === userAgent ? 1 : 0;
  const ext = preferExt && f.ext === preferExt ? 1 : 0;
  return (((lang + 100) * 10 + sameClient) * 10 + ext) * 1e6 + (Number(f.abr) || Number(f.tbr) || 0);
}

function bestBy(list, score) {
  let best = null;
  let bestScore = -Infinity;
  for (const f of list) {
    const s = score(f);
    if (s > bestScore) {
      best = f;
      bestScore = s;
    }
  }
  return best;
}

const containerLabel = (ext) => ({ mp4: "MP4", webm: "WebM", m4a: "M4A", mkv: "MKV" })[ext] || ext.toUpperCase();

/**
 * 档位列表：按清晰度从高到低，每档一个「最佳视频轨 + 同容器音频轨」组合，末尾一个仅音频档。
 * 视频与音频容器不一致时落成 mkv（引擎合并时保留原编码，mkv 能装下任意组合）。
 */
function buildOptions(info, codec, base) {
  const formats = usableFormats(info.formats);
  const audios = formats.filter((f) => hasAudio(f) && !hasVideo(f));
  const videos = formats.filter((f) => hasVideo(f) && pOf(f) > 0);
  const heights = [...new Set(videos.map(pOf))].sort((a, b) => b - a).slice(0, MAX_VIDEO_OPTIONS);
  const options = [];

  for (const p of heights) {
    const v = bestBy(
      videos.filter((f) => pOf(f) === p),
      (f) => videoScore(f, codec),
    );
    const ua = v.http_headers ? v.http_headers["User-Agent"] : "";
    let a = null;
    let ext = v.ext || "mp4";
    if (!hasAudio(v)) {
      const wanted = ext === "webm" ? "webm" : "m4a";
      a = bestBy(audios, (f) => audioScore(f, wanted, ua));
      if (!a) continue;
      if (a.ext !== wanted) ext = "mkv";
    }
    const fps = Number(v.fps) > 30 ? Math.round(Number(v.fps)) : "";
    const hdr = v.dynamic_range && v.dynamic_range !== "SDR" ? ` ${v.dynamic_range}` : "";
    options.push({
      preset: String(p),
      height: p,
      label: `${p}p${fps}${hdr} · ${codecName(v.vcodec)} · ${containerLabel(ext)}`,
      video: v,
      audio: a,
      fileName: `${base}.${ext}`,
      container: ext,
    });
  }

  const audioOnly = bestBy(audios, (f) => audioScore(f, codec === "quality" ? "" : "m4a", ""));
  if (audioOnly) {
    const ext = audioOnly.ext || "m4a";
    options.push({
      preset: "audio",
      height: 0,
      label: `仅音频 · ${codecName(audioOnly.acodec)} · ${containerLabel(ext)}`,
      video: audioOnly,
      audio: null,
      fileName: `${base}.${ext}`,
      container: ext,
    });
  }
  return options;
}

/** 固定清晰度：不超过目标高度的最高档；目标比所有档都低时取最低档。 */
function pickOption(options, preset) {
  const videos = options.filter((o) => o.height > 0);
  if (preset === "audio") return options.find((o) => o.height === 0) || videos[0] || null;
  if (preset === "best" || preset === "ask") return videos[0] || options[0] || null;
  const limit = Number(preset);
  return videos.find((o) => o.height <= limit) || videos[videos.length - 1] || options[0] || null;
}

function toVariant(o) {
  return {
    label: o.label,
    url: o.video.url,
    // 恒显式给出（无音轨传空串），避免引擎沿用上一档的音频直链。
    audioUrl: o.audio ? o.audio.url : "",
    fileName: o.fileName,
    totalBytes: sizeOf(o.video) + (o.audio ? sizeOf(o.audio) : 0),
    bandwidth: Math.round((Number(o.video.tbr) || 0) * 1000),
    width: Number(o.video.width) || 0,
    height: Number(o.video.height) || 0,
    container: o.container,
  };
}

// ---------------------------------------------------------------------------
// 解析入口
// ---------------------------------------------------------------------------

async function resolveVideo(ctx, videoId, preset, interactive) {
  const info = await runYtdlp(ctx, ["--no-playlist"], `https://www.youtube.com/watch?v=${videoId}`);
  if (info.live_status === "is_live" || info.live_status === "is_upcoming") {
    throw new Error("直播尚未结束，结束后可下载回放");
  }
  const base = sanitizeFileName(info.title, videoId);
  const options = buildOptions(info, String(flux.settings.codec || "compat"), base);
  if (options.length === 0) {
    throw new Error(
      info.live_status === "post_live"
        ? "直播回放仍在处理中，稍后再试"
        : "没有可直接下载的格式（可能被风控或需要登录 Cookie）",
    );
  }

  const chosen = pickOption(options, preset);
  const top = toVariant(chosen);
  flux.logger.info("[youtube-down]", videoId, preset, "→", chosen.label);
  const result = {
    url: top.url,
    audioUrl: top.audioUrl,
    fileName: top.fileName,
    totalBytes: top.totalBytes || undefined,
    // googlevideo 直链带签名与过期时间：跳过探测（探测会浪费一次性直链），并担保支持 Range。
    ephemeral: true,
    rangeSupported: true,
  };
  const headers = chosen.video.http_headers;
  if (headers && typeof headers === "object") result.extraHeaders = headers;
  if (interactive && preset === "ask" && options.length > 1) {
    // variants[0] 即最高画质，弹框超时 / 免打扰 / headless 时按它下载。
    result.variants = options.map(toVariant);
    result.defaultVariantIndex = 0;
  }
  return result;
}

async function listPlaylist(ctx, playlistId) {
  const limit = Math.min(1000, Math.max(1, Math.floor(Number(flux.settings.playlistLimit) || 200)));
  const info = await runYtdlp(
    ctx,
    ["--flat-playlist", "--playlist-end", String(limit)],
    `https://www.youtube.com/playlist?list=${playlistId}`,
  );
  const entries = Array.isArray(info.entries) ? info.entries : [];
  const quality = String(flux.settings.quality || "ask");
  const first = ITEM_PRESETS.find((p) => p.id === quality) || ITEM_PRESETS[0];
  const variants = [first, ...ITEM_PRESETS.filter((p) => p !== first)];
  const seen = new Set();
  const items = [];
  for (const e of entries) {
    if (!e || !VIDEO_ID_RE.test(String(e.id || "")) || seen.has(e.id)) continue;
    if (UNAVAILABLE_TITLES.includes(e.title)) continue;
    seen.add(e.id);
    items.push({
      id: e.id,
      name: `${sanitizeFileName(e.title, e.id)}.${first.id === "audio" ? "m4a" : "mp4"}`,
      kind: "file",
      variants,
    });
  }
  if (items.length === 0) throw new Error("播放列表为空，或其中的视频都不可用");
  flux.logger.info("[youtube-down] playlist", playlistId, items.length, "items");
  return { manifest: { name: sanitizeFileName(info.title, playlistId), items } };
}

globalThis.resolve = async (ctx) => {
  const item = parseItem(ctx.resolverItem);
  if (item) return resolveVideo(ctx, item.videoId, item.preset, false);

  const target = parseTarget(ctx.url);
  if (!target) throw new Error(`不是可下载的 YouTube 视频或播放列表链接：${ctx.url}`);
  if (target.playlistId) return listPlaylist(ctx, target.playlistId);
  // 新建下载对话框的前置预解析（还没有任务）只关心播放列表清单；单个视频不必白跑一次 yt-dlp。
  if (!ctx.taskId) return null;
  const quality = String(flux.settings.quality || "ask");
  return resolveVideo(ctx, target.videoId, PRESETS.includes(quality) || quality === "ask" ? quality : "ask", true);
};
