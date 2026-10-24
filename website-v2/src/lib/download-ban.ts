/**
 * /api/download 的按 IP 封禁：15 分钟内超过 20 次下载即封禁 12 小时，封禁期间一律 429。
 *
 * 计数口径是「下载次数」而不是请求数：同一 IP 在 60 秒内重复请求同一文件只算一次，
 * 多线程下载器的各分段连接、断线重试都会重复请求同一个 /api/download 地址，不能把它们
 * 算成多次下载。窗口为滑动窗口（记录每次计数的时间）。
 *
 * 状态在进程内存（单容器部署），容器重启后清空。反代另有每 IP 每分钟 30 次的限频
 * （conf.d/fluxdown-download-limit.conf），两者互不替代。
 */

export const WINDOW_MS = 15 * 60 * 1000;
export const MAX_DOWNLOADS = 20;
export const BAN_MS = 12 * 3600 * 1000;
/** 同一文件的重复请求在此时长内合并为一次下载。 */
const REPEAT_MS = 60 * 1000;
/** 跟踪的 IP 上限，超出时淘汰最早写入的条目（封禁记录单独保存，不受影响）。 */
const MAX_TRACKED = 200_000;

interface Tracker {
  /** 每次计数的时间（升序）。 */
  hits: number[];
  /** 文件 → 最近一次请求时间，用于合并重复请求。 */
  recent: Map<string, number>;
}

const trackers = new Map<string, Tracker>();
const bans = new Map<string, number>();

/** 返回该 IP 当前封禁的剩余毫秒数；未封禁为 0。 */
export function banRemaining(ip: string, now = Date.now()): number {
  const until = bans.get(ip);
  if (until === undefined) return 0;
  if (until > now) return until - now;
  bans.delete(ip);
  return 0;
}

/**
 * 记录一次下载请求，返回记录后的封禁剩余毫秒数（0 = 放行）。
 * `file` 为文件名与 tag 的组合，用于合并同一文件的重复请求。
 */
export function recordDownload(ip: string, file: string, now = Date.now()): number {
  if (!ip || ip === "unknown") return 0;
  const banned = banRemaining(ip, now);
  if (banned > 0) return banned;

  let tracker = trackers.get(ip);
  if (tracker) {
    trackers.delete(ip);
  } else {
    tracker = { hits: [], recent: new Map() };
  }
  trackers.set(ip, tracker);
  while (trackers.size > MAX_TRACKED) {
    const oldest = trackers.keys().next();
    if (oldest.done) break;
    trackers.delete(oldest.value);
  }

  for (const [name, at] of tracker.recent) {
    if (now - at >= REPEAT_MS) tracker.recent.delete(name);
  }
  const last = tracker.recent.get(file);
  tracker.recent.set(file, now);
  if (last !== undefined && now - last < REPEAT_MS) return 0;

  const hits = tracker.hits;
  while (hits.length > 0 && now - hits[0] >= WINDOW_MS) hits.shift();
  hits.push(now);
  if (hits.length <= MAX_DOWNLOADS) return 0;

  bans.set(ip, now + BAN_MS);
  trackers.delete(ip);
  console.warn(`[download-ban] ${ip} 15 分钟内下载 ${hits.length} 次，封禁 12 小时`);
  return BAN_MS;
}

/** 测试用：清空计数与封禁。 */
export function resetDownloadBans(): void {
  trackers.clear();
  bans.clear();
}
