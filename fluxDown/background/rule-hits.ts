/**
 * 规则命中统计：内存 Map（容量封顶，淘汰最久未命中者）+ storage.session 节流落盘，
 * 让 service worker 被回收后计数不丢。含 `ignore` 决定，用于回答「为什么没识别到 X」与清理死规则。
 * 空闲静默：只有发生命中时才会安排一次延迟写入。
 */
import { browser } from "wxt/browser";
import type { RuleHit } from "./rule-view";

const HITS_KEY = "fluxdown_rule_hits";
const MAX_TRACKED = 2000;
const FLUSH_DELAY_MS = 10_000;

const hits = new Map<string, RuleHit>();
let loading: Promise<void> | null = null;
let flushTimer: ReturnType<typeof setTimeout> | undefined;

/** 记一次命中；超出容量时淘汰 lastAt 最旧的条目。 */
export function bumpHit(map: Map<string, RuleHit>, id: string, now: number, max: number): void {
  const existing = map.get(id);
  if (existing) {
    existing.count++;
    existing.lastAt = now;
    // 重新插入使 Map 迭代顺序 = 最近命中顺序。
    map.delete(id);
    map.set(id, existing);
    return;
  }
  map.set(id, { count: 1, lastAt: now });
  if (map.size > max) {
    const oldest = map.keys().next();
    if (!oldest.done) map.delete(oldest.value);
  }
}

/** 合并持久化的统计：计数相加，lastAt 取较大。 */
export function mergeHits(map: Map<string, RuleHit>, raw: unknown, max: number): void {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return;
  for (const [id, value] of Object.entries(raw)) {
    if (value === null || typeof value !== "object") continue;
    const { count, lastAt } = value as Partial<RuleHit>;
    if (!Number.isInteger(count) || !Number.isFinite(lastAt)) continue;
    const existing = map.get(id);
    if (existing) {
      existing.count += count as number;
      existing.lastAt = Math.max(existing.lastAt, lastAt as number);
    } else if (map.size < max) {
      map.set(id, { count: count as number, lastAt: lastAt as number });
    }
  }
}

function ensureLoaded(): Promise<void> {
  loading ??= (async () => {
    try {
      const raw = (await browser.storage.session.get(HITS_KEY))[HITS_KEY];
      mergeHits(hits, raw, MAX_TRACKED);
    } catch (e) {
      // 无 storage.session（旧 Firefox）或读取失败：仅内存计数。
      console.debug("[FluxDown] rule hits restore skipped:", e);
    }
  })();
  return loading;
}

async function flush(): Promise<void> {
  flushTimer = undefined;
  try {
    await ensureLoaded();
    await browser.storage.session.set({ [HITS_KEY]: Object.fromEntries(hits) });
  } catch (e) {
    console.debug("[FluxDown] rule hits flush skipped:", e);
  }
}

export function recordRuleHit(id: string): void {
  void ensureLoaded();
  bumpHit(hits, id, Date.now(), MAX_TRACKED);
  flushTimer ??= setTimeout(() => void flush(), FLUSH_DELAY_MS);
}

/** 当前统计快照（先等持久化部分合并完）。 */
export async function getRuleHits(): Promise<ReadonlyMap<string, RuleHit>> {
  await ensureLoaded();
  return hits;
}
