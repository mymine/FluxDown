// InnerTube streamingData → 可直接 HTTP 下载的格式 → 「视频轨 + 音频轨」档位。
// 字段与判据对齐 yt-dlp `_video.py` _extract_formats_and_subtitles 与 utils.parse_codecs。
import { parseQuery, setUrlParam, urlParam } from "./query.js";

/** 清晰度弹框最多列出的视频档数（另加 1 个仅音频档）。 */
export const MAX_VIDEO_OPTIONS = 9;

const VIDEO_CODECS = new Set(["avc1", "avc2", "avc3", "avc4", "vp9", "vp8", "hev1", "hev2", "h263", "h264", "mp4v", "hvc1", "av1", "theora", "dvh1", "dvhe"]);
const AUDIO_CODECS = new Set(["flac", "mp4a", "opus", "vorbis", "mp3", "aac", "ac-4", "ac-3", "ec-3", "eac3", "dtsc", "dtse", "dtsh", "dtsl"]);

/** RFC 6381 codecs → `{ vcodec, acodec, dynamicRange }`（yt-dlp parse_codecs 的判据）。 */
export function parseCodecs(codecs) {
  let vcodec = "none";
  let acodec = "none";
  let dynamicRange = "SDR";
  for (const raw of String(codecs || "").split(",").map((s) => s.trim()).filter(Boolean)) {
    const full = raw.replace(/^[^.]+/, (m) => m.toLowerCase());
    const parts = full.replace(/0+(?=\d)/g, "").split(".");
    if (VIDEO_CODECS.has(parts[0])) {
      if (vcodec !== "none") continue;
      vcodec = full;
      if (parts[0] === "dvh1" || parts[0] === "dvhe") dynamicRange = "DV";
      else if (parts[0] === "av1" && parts[3] === "10") dynamicRange = "HDR10";
      else if (parts[0] === "vp9" && parts[1] === "2") dynamicRange = "HDR10";
    } else if (AUDIO_CODECS.has(parts[0]) && acodec === "none") {
      acodec = full;
    }
  }
  return { vcodec, acodec, dynamicRange };
}

/** mimeType → 扩展名（yt-dlp mimetype2ext 的 YouTube 子集）。 */
function extOf(mime) {
  if (mime === "audio/mp4") return "m4a";
  if (mime === "video/3gpp") return "3gp";
  const sub = mime.split("/")[1] || "";
  return sub === "mp4" || sub === "webm" ? sub : sub || "bin";
}

function languageOf(track) {
  const name = String((track && track.displayName) || "").toLowerCase();
  const code = String((track && track.id) || "").split(".")[0] || "";
  if (name.includes("descriptive")) return { code: code ? `${code}-desc` : "", preference: -10 };
  if (name.includes("original")) return { code, preference: 10 };
  if (track && track.audioIsDefault) return { code, preference: 5 };
  return { code, preference: track ? -1 : 0 };
}

/**
 * 第一遍：按客户端顺序收集可用的流（首个客户端优先，按 itag + 音轨 + DRC 去重），并汇总
 * 全部 n / 签名挑战，供一次性求解。跳过直播分片、OTF、DRM 与没有地址的 SABR-only 格式。
 * @param {{client: string, response: any}[]} responses
 */
export function collectStreams(responses) {
  const streams = [];
  const seen = new Set();
  const n = new Set();
  const sig = new Set();
  for (const { client, response } of responses) {
    const sd = response && response.streamingData;
    if (!sd) continue;
    const duration = Number(response.videoDetails && response.videoDetails.lengthSeconds) || 0;
    for (const f of [...(sd.formats || []), ...(sd.adaptiveFormats || [])]) {
      if (!f || typeof f !== "object" || f.targetDurationSec || f.drmFamilies || f.type === "FORMAT_STREAM_TYPE_OTF") continue;
      const key = `${f.itag}|${(f.audioTrack && f.audioTrack.id) || ""}|${f.isDrc ? 1 : 0}`;
      if (seen.has(key)) continue;
      let url = typeof f.url === "string" ? f.url : "";
      let s = "";
      let sp = "signature";
      if (!url && typeof f.signatureCipher === "string") {
        const sc = parseQuery(f.signatureCipher);
        url = sc.url || "";
        s = sc.s || "";
        sp = sc.sp || "signature";
        if (!s) url = "";
      }
      if (!/^https:\/\//.test(url)) continue;
      seen.add(key);
      const challenge = urlParam(url, "n") || "";
      if (challenge) n.add(challenge);
      if (s) sig.add(s);
      streams.push({ client, f, url, s, sp, n: challenge, duration });
    }
  }
  return { streams, nChallenges: [...n], sigChallenges: [...sig] };
}

/** 第二遍：套用求解结果（缺结果的格式丢弃，与 yt-dlp 一致）并归一字段。 */
export function finalizeStreams(streams, solved) {
  const out = [];
  for (const { client, f, url: base, s, sp, n, duration } of streams) {
    let url = base;
    if (s) {
      const solvedSig = solved.sig[s];
      if (!solvedSig) continue;
      url = setUrlParam(url, sp, solvedSig);
    }
    if (n) {
      const solvedN = solved.n[n];
      if (!solvedN) continue;
      url = setUrlParam(url, "n", solvedN);
    }
    const m = /^([^/]+\/[^;]+)(?:;\s*codecs="([^"]+)")?/.exec(String(f.mimeType || ""));
    if (!m) continue;
    const mime = m[1].toLowerCase();
    const codecs = parseCodecs(m[2]);
    if (!m[2] && mime.startsWith("audio/")) codecs.acodec = mime.split("/")[1];
    const tbr = (Number(f.averageBitrate) || Number(f.bitrate) || 0) / 1000;
    const formatDuration = Number(f.approxDurationMs) / 1000;
    const fps = Number(f.fps) > 1 ? Number(f.fps) : 0;
    const lang = languageOf(f.audioTrack);
    const xtags = parseQuery(urlParam(url, "xtags") || "");
    const filesize = Number(f.contentLength) || 0;
    out.push({
      itag: Number(f.itag) || 0,
      client,
      url,
      ext: extOf(mime),
      vcodec: codecs.vcodec,
      acodec: codecs.acodec,
      dynamicRange: codecs.dynamicRange,
      width: Number(f.width) || 0,
      height: Number(f.height) || 0,
      fps,
      tbr,
      filesize,
      languagePreference: lang.preference,
      language: lang.code,
      isDrc: Boolean(f.isDrc),
      superResolution: xtags.sr === "1",
      // 时长不到全片一半的格式可能在转码时损坏（yt-dlp 同一判据，降为不可选）。
      damaged: Boolean(duration && formatDuration && formatDuration < Math.floor(duration / 2)),
      projection: String(f.projectionType || "RECTANGULAR"),
    });
  }
  return out;
}

const hasVideo = (f) => f.vcodec !== "none";
const hasAudio = (f) => f.acodec !== "none";

/** 引擎按 Range 直接下载的候选：排除损坏、动态范围压缩音轨与全景投影。 */
function usable(formats) {
  return formats.filter((f) => !f.damaged && !f.isDrc && f.projection === "RECTANGULAR" && (hasVideo(f) || hasAudio(f)));
}

/** 清晰度取短边：竖屏视频（Shorts）的 1080×1920 也算 1080p。 */
export function pOf(f) {
  return f.width > 0 && f.height > 0 ? Math.min(f.width, f.height) : f.height;
}

export function codecName(codec) {
  const c = String(codec || "").toLowerCase();
  if (c.startsWith("avc1") || c.startsWith("h264")) return "H.264";
  if (c.startsWith("av01")) return "AV1";
  if (c.startsWith("vp9") || c.startsWith("vp09")) return "VP9";
  if (c.startsWith("mp4a")) return "AAC";
  if (c.startsWith("opus")) return "Opus";
  return c.split(".")[0].toUpperCase();
}

/** 视频评分：原画优先于 AI 超分；有独立音轨可配时分离轨优先于音画合一（后者音质低、常无准确大小）；SDR 优先；再按编码偏好、帧率、码率。 */
function videoScore(f, codec) {
  const name = codecName(f.vcodec);
  const rank = codec === "quality" ? { AV1: 3, VP9: 2, "H.264": 1 }[name] || 0 : { "H.264": 3, AV1: 2, VP9: 1 }[name] || 0;
  const original = f.superResolution ? 0 : 1;
  const separate = hasAudio(f) ? 0 : 1;
  const sdr = f.dynamicRange === "SDR" ? 1 : 0;
  return ((((original * 10 + separate) * 10 + sdr) * 10 + rank) * 1000 + f.fps) * 1e6 + f.tbr;
}

/** 音轨评分：原声语言优先；与视频同客户端优先；兼容优先时同容器优先；再比码率。 */
function audioScore(f, preferExt, client) {
  const sameClient = client && f.client === client ? 1 : 0;
  const ext = preferExt && f.ext === preferExt ? 1 : 0;
  return (((f.languagePreference + 100) * 10 + sameClient) * 10 + ext) * 1e6 + f.tbr;
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
export function buildOptions(formats, codec, base) {
  const list = usable(formats);
  const audios = list.filter((f) => hasAudio(f) && !hasVideo(f));
  const videos = list.filter((f) => hasVideo(f) && pOf(f) > 0);
  const heights = [...new Set(videos.map(pOf))].sort((a, b) => b - a).slice(0, MAX_VIDEO_OPTIONS);
  const options = [];

  for (const p of heights) {
    // 没有独立音轨可配时只剩音画合一的格式可选。
    const candidates = videos.filter((f) => pOf(f) === p && (audios.length > 0 || hasAudio(f)));
    if (candidates.length === 0) continue;
    const v = bestBy(candidates, (f) => videoScore(f, codec));
    let a = null;
    let ext = v.ext;
    if (!hasAudio(v)) {
      const wanted = ext === "webm" ? "webm" : "m4a";
      a = bestBy(audios, (f) => audioScore(f, wanted, v.client));
      if (!a) continue;
      if (a.ext !== wanted) ext = "mkv";
    }
    const fps = v.fps > 30 ? Math.round(v.fps) : "";
    const hdr = v.dynamicRange !== "SDR" ? ` ${v.dynamicRange}` : "";
    const sr = v.superResolution ? " · AI 超分" : "";
    options.push({
      preset: String(p),
      height: p,
      label: `${p}p${fps}${hdr} · ${codecName(v.vcodec)} · ${containerLabel(ext)}${sr}`,
      video: v,
      audio: a,
      fileName: `${base}.${ext}`,
      container: ext,
    });
  }

  const audioOnly = bestBy(audios, (f) => audioScore(f, codec === "quality" ? "" : "m4a", ""));
  if (audioOnly) {
    options.push({
      preset: "audio",
      height: 0,
      label: `仅音频 · ${codecName(audioOnly.acodec)} · ${containerLabel(audioOnly.ext)}`,
      video: audioOnly,
      audio: null,
      fileName: `${base}.${audioOnly.ext}`,
      container: audioOnly.ext,
    });
  }
  return options;
}

/** 固定清晰度：不超过目标高度的最高档；目标比所有档都低时取最低档。 */
export function pickOption(options, preset) {
  const videos = options.filter((o) => o.height > 0);
  if (preset === "audio") return options.find((o) => o.height === 0) || videos[0] || null;
  if (preset === "best" || preset === "ask") return videos[0] || options[0] || null;
  const limit = Number(preset);
  return videos.find((o) => o.height <= limit) || videos[videos.length - 1] || options[0] || null;
}

export function toVariant(o) {
  const tracks = o.audio ? [o.video, o.audio] : [o.video];
  return {
    label: o.label,
    url: o.video.url,
    // 恒显式给出（无音轨传空串），避免引擎沿用上一档的音频直链。
    audioUrl: o.audio ? o.audio.url : "",
    fileName: o.fileName,
    // 只报准确大小（contentLength）：引擎跳过探测、按它切分 Range，估算值会请求越界（416）。
    // 0 = 未知，引擎改为边下边确认长度。
    totalBytes: tracks.every((f) => f.filesize > 0) ? tracks.reduce((sum, f) => sum + f.filesize, 0) : 0,
    bandwidth: Math.round(o.video.tbr * 1000),
    width: o.video.width,
    height: o.video.height,
    container: o.container,
  };
}
