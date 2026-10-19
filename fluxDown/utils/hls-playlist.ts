/**
 * HLS 播放列表的关联线索（不用于下载：下载仍把播放列表 URL 交给引擎的 HLS 下载器）。
 *
 * 嗅探只看得到一条条独立请求：主播放列表、各档媒体播放列表、成百上千个分片。没有这些
 * 关联时，同一个视频会以「主列表 + 每档子列表」多行出现，分片落进「未找到播放清单」，
 * 播放元素也无法关联到候选（浮钮不出现）。
 */

/** 单份播放列表的关联信息。 */
export interface HlsPlaylistInfo {
  /** 主播放列表引用的子播放列表（各档视频、独立音轨 / 字幕、I 帧列表），绝对 URL。 */
  children: string[];
  /** 媒体播放列表的分片所在目录（`origin + 目录路径`，不含文件名）。 */
  segmentDirs: string[];
  /** 主播放列表的各档视频（EXT-X-STREAM-INF 属性）。 */
  variants?: HlsVariant[];
  /** 使用 SAMPLE-AES / 非 identity KEYFORMAT（FairPlay / Widevine 等 DRM），下载得到的是密文。 */
  protected?: boolean;
}

/** 主播放列表里的一档。 */
export interface HlsVariant {
  url: string;
  bandwidth?: number;
  width?: number;
  height?: number;
  frameRate?: number;
  codecs?: string;
  /** 引用了 EXT-X-MEDIA 音频组：单独下载这一档的媒体播放列表会没有声音。 */
  audioGroup?: boolean;
}

/** 一份已观察到的播放列表（url 为去重键）。 */
export interface HlsPlaylistEntry {
  url: string;
  playlist: HlsPlaylistInfo;
}

const MAX_CHILDREN = 64;
const MAX_SEGMENT_DIRS = 16;

function absolute(uri: string, baseUrl: string): string | null {
  try {
    const parsed = new URL(uri, baseUrl);
    return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed.href : null;
  } catch {
    return null;
  }
}

/** 分片目录键：`origin + pathname` 去掉文件名（保留结尾 `/`）。 */
export function hlsSegmentDir(url: string): string | null {
  try {
    const parsed = new URL(url);
    const slash = parsed.pathname.lastIndexOf("/");
    return `${parsed.origin}${parsed.pathname.slice(0, slash + 1)}`;
  } catch {
    return null;
  }
}

/** 标签属性里的 `URI="..."`。 */
function uriAttribute(line: string): string | null {
  const match = /(?:^|[:,])URI="([^"]*)"/.exec(line);
  return match ? match[1] : null;
}

/** `#TAG:A=1,B="x,y"` 的属性表（键大写）。 */
function attributes(line: string): Record<string, string> {
  const out: Record<string, string> = {};
  const body = line.slice(line.indexOf(":") + 1);
  const re = /([A-Z0-9-]+)=("[^"]*"|[^,]*)/gi;
  for (let m = re.exec(body); m; m = re.exec(body)) {
    out[m[1].toUpperCase()] = m[2].replace(/^"|"$/g, "");
  }
  return out;
}

function positiveNumber(value: string | undefined): number | undefined {
  const n = value === undefined ? NaN : Number(value);
  return Number.isFinite(n) && n > 0 ? n : undefined;
}

/** 加密方式是否为 DRM（AES-128 由下载器自行解密，不算）。 */
function isDrmKey(line: string): boolean {
  const attrs = attributes(line);
  const method = (attrs.METHOD ?? "").toUpperCase();
  if (method === "NONE" || method === "") return false;
  if (method.startsWith("SAMPLE-AES")) return true;
  const format = (attrs.KEYFORMAT ?? "identity").toLowerCase();
  return format !== "identity";
}

/** 解析 m3u8 文本；不是 HLS 播放列表时返回 null。 */
export function parseHlsPlaylist(text: string, baseUrl: string): HlsPlaylistInfo | null {
  if (!text.trimStart().startsWith("#EXTM3U")) return null;
  const children = new Set<string>();
  const segmentDirs = new Set<string>();
  const variants: HlsVariant[] = [];
  let pendingVariant: Omit<HlsVariant, "url"> | null = null;
  let drm = false;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    if (line.startsWith("#")) {
      if (line.startsWith("#EXT-X-STREAM-INF")) {
        const attrs = attributes(line);
        const resolution = /^(\d+)x(\d+)$/i.exec(attrs.RESOLUTION ?? "");
        const bandwidth = positiveNumber(attrs["AVERAGE-BANDWIDTH"]) ?? positiveNumber(attrs.BANDWIDTH);
        const frameRate = positiveNumber(attrs["FRAME-RATE"]);
        pendingVariant = {
          ...(bandwidth ? { bandwidth } : {}),
          ...(resolution ? { width: Number(resolution[1]), height: Number(resolution[2]) } : {}),
          ...(frameRate ? { frameRate } : {}),
          ...(attrs.CODECS ? { codecs: attrs.CODECS } : {}),
          ...(attrs.AUDIO ? { audioGroup: true } : {}),
        };
      } else if (line.startsWith("#EXT-X-MEDIA:") || line.startsWith("#EXT-X-I-FRAME-STREAM-INF")) {
        const uri = uriAttribute(line);
        const url = uri ? absolute(uri, baseUrl) : null;
        if (url && children.size < MAX_CHILDREN) children.add(url);
      } else if (line.startsWith("#EXT-X-MAP")) {
        const uri = uriAttribute(line);
        const url = uri ? absolute(uri, baseUrl) : null;
        const dir = url ? hlsSegmentDir(url) : null;
        if (dir) segmentDirs.add(dir);
      } else if (line.startsWith("#EXT-X-KEY") || line.startsWith("#EXT-X-SESSION-KEY")) {
        if (isDrmKey(line)) drm = true;
      }
      continue;
    }
    const url = absolute(line, baseUrl);
    if (!url) {
      pendingVariant = null;
      continue;
    }
    if (pendingVariant) {
      if (children.size < MAX_CHILDREN) {
        children.add(url);
        variants.push({ url, ...pendingVariant });
      }
      pendingVariant = null;
      continue;
    }
    const dir = hlsSegmentDir(url);
    if (dir) segmentDirs.add(dir);
  }
  if (children.size === 0 && segmentDirs.size === 0) return null;
  const info: HlsPlaylistInfo = { children: [...children], segmentDirs: compactDirs([...segmentDirs]) };
  if (variants.length > 0) info.variants = variants;
  if (drm) info.protected = true;
  return info;
}

/**
 * 分片目录数超过上限（有的源每个分片单独一个目录）时，按 origin 收敛为公共目录前缀。
 * 公共前缀退化到站点根目录时不收敛（会把同站所有媒体都算进来），只保留前若干个目录。
 */
function compactDirs(dirs: string[]): string[] {
  if (dirs.length <= MAX_SEGMENT_DIRS) return dirs;
  const byOrigin = new Map<string, string[]>();
  for (const dir of dirs) {
    const origin = new URL(dir).origin;
    byOrigin.set(origin, [...(byOrigin.get(origin) ?? []), dir]);
  }
  const out: string[] = [];
  for (const [origin, group] of byOrigin) {
    let prefix = group[0];
    for (const dir of group) {
      while (!dir.startsWith(prefix)) prefix = prefix.slice(0, prefix.lastIndexOf("/", prefix.length - 2) + 1);
    }
    if (prefix.length > origin.length + 1) out.push(prefix);
    else out.push(...group.slice(0, MAX_SEGMENT_DIRS));
  }
  return out.slice(0, MAX_SEGMENT_DIRS);
}

function httpStrings(value: unknown, max: number): string[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((item): item is string => typeof item === "string" && /^https?:\/\//i.test(item))
    .slice(0, max);
}

function normalizeVariant(value: unknown): HlsVariant | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (typeof record.url !== "string" || !/^https?:\/\//i.test(record.url)) return null;
  const num = (key: string): number | undefined => {
    const n = record[key];
    return typeof n === "number" && Number.isFinite(n) && n > 0 ? n : undefined;
  };
  const bandwidth = num("bandwidth");
  const width = num("width");
  const height = num("height");
  const frameRate = num("frameRate");
  return {
    url: record.url,
    ...(bandwidth ? { bandwidth } : {}),
    ...(width ? { width } : {}),
    ...(height ? { height } : {}),
    ...(frameRate ? { frameRate } : {}),
    ...(typeof record.codecs === "string" && record.codecs ? { codecs: record.codecs } : {}),
    ...(record.audioGroup === true ? { audioGroup: true } : {}),
  };
}

/** 校验跨边界传来的关联信息（页面世界可伪造事件，持久化快照可能是旧版本）。 */
export function normalizeHlsPlaylist(raw: unknown): HlsPlaylistInfo | null {
  if (!raw || typeof raw !== "object") return null;
  const record = raw as Record<string, unknown>;
  const children = httpStrings(record.children, MAX_CHILDREN);
  const segmentDirs = httpStrings(record.segmentDirs, MAX_SEGMENT_DIRS);
  if (children.length === 0 && segmentDirs.length === 0) return null;
  const variants = Array.isArray(record.variants)
    ? record.variants.slice(0, MAX_CHILDREN).map(normalizeVariant).filter((v): v is HlsVariant => v !== null)
    : [];
  return {
    children,
    segmentDirs,
    ...(variants.length > 0 ? { variants } : {}),
    ...(record.protected === true ? { protected: true } : {}),
  };
}
