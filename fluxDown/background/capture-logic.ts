/**
 * capture 跟踪的纯决策逻辑（零 browser 依赖，可在 bun 下测试）。
 *
 * 桌面端对每次外部下载开一个「事务」（transactionId），用户在确认框里接受 / 取消。
 * 扩展轮询事务状态：被拒（取消）且开启 `fallbackOnCancel` 时回退浏览器下载；
 * 同一站点 7 天内被取消 ≥ 3 次时询问是否不再接管该站点（#484）。
 */

export interface PendingCapture {
  id: string;
  url: string;
  filename?: string;
  host: string;
  createdAt: number;
}

export type CaptureState = "pending" | "accepted" | "rejected" | "failed" | "unknown";

/** host → 被拒时间戳（只保留 7 天内）。 */
export type CancelStats = Record<string, number[]>;

export interface CaptureSettings {
  fallbackOnReject: boolean;
  fallbackOnCancel: boolean;
  askExcludeAfterCancels: boolean;
}

export type CaptureAction =
  /** 回退浏览器下载。 */
  | { kind: "fallback"; capture: PendingCapture }
  /** 记录一次被拒。 */
  | { kind: "recordReject"; host: string }
  /** 询问是否对该站点不再接管。 */
  | { kind: "askExclude"; host: string };

export const PENDING_MAX_AGE_MS = 30 * 60 * 1000;
export const CANCEL_STATS_WINDOW_MS = 7 * 24 * 60 * 60 * 1000;
export const ASK_EXCLUDE_THRESHOLD = 3;

/** 只保留窗口内的时间戳；空 host 项被移除。 */
export function pruneCancelStats(stats: CancelStats, now: number): CancelStats {
  const out: CancelStats = {};
  for (const [host, times] of Object.entries(stats)) {
    const recent = times.filter((t) => now - t < CANCEL_STATS_WINDOW_MS);
    if (recent.length > 0) out[host] = recent;
  }
  return out;
}

/**
 * 根据最新状态决定动作，并给出仍需继续轮询的条目。
 * - rejected：记入被拒统计；`fallbackOnCancel` 为真则回退；达到阈值且无站点配置时询问；
 * - failed：`fallbackOnReject` 为真则回退；
 * - accepted / unknown：移除；pending 或没有状态：保留；超过 30 min：丢弃。
 */
export function nextCaptureActions(
  pending: PendingCapture[],
  statuses: Record<string, CaptureState | undefined>,
  settings: CaptureSettings,
  stats: CancelStats,
  now: number,
  profiledHosts: ReadonlySet<string> = new Set(),
): { actions: CaptureAction[]; keep: PendingCapture[] } {
  const actions: CaptureAction[] = [];
  const keep: PendingCapture[] = [];
  const counts = new Map<string, number>(
    Object.entries(pruneCancelStats(stats, now)).map(([host, times]) => [host, times.length]),
  );
  const asked = new Set<string>();

  for (const capture of pending) {
    if (now - capture.createdAt > PENDING_MAX_AGE_MS) continue;
    const state = statuses[capture.id];
    if (state === undefined || state === "pending") {
      keep.push(capture);
      continue;
    }
    if (state === "rejected") {
      actions.push({ kind: "recordReject", host: capture.host });
      const count = (counts.get(capture.host) ?? 0) + 1;
      counts.set(capture.host, count);
      if (settings.fallbackOnCancel) actions.push({ kind: "fallback", capture });
      if (
        settings.askExcludeAfterCancels &&
        count >= ASK_EXCLUDE_THRESHOLD &&
        !profiledHosts.has(capture.host) &&
        !asked.has(capture.host)
      ) {
        asked.add(capture.host);
        actions.push({ kind: "askExclude", host: capture.host });
      }
    } else if (state === "failed") {
      if (settings.fallbackOnReject) actions.push({ kind: "fallback", capture });
    }
    // accepted / unknown：不再跟踪
  }
  return { actions, keep };
}

/**
 * 轮询往返（一次 NMH 调用）期间 `current` 可能新增了条目：
 * 把本轮 `keep`（来自快照 `snapshotIds`）与往返期间新登记、不在快照里的条目合并，
 * 避免整体赋值 `pending = keep` 丢掉新事务。
 */
export function reconcilePending(
  current: readonly PendingCapture[],
  snapshotIds: ReadonlySet<string>,
  keep: readonly PendingCapture[],
): PendingCapture[] {
  return [...keep, ...current.filter((p) => !snapshotIds.has(p.id))];
}
