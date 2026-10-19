/** 拦截统计：sent / failed 按天累计。 */
import { browser } from "wxt/browser";
import type { Browser } from "wxt/browser";

interface DailyStats {
  sent: number;
  failed: number;
  date: string;
}

// Bug 4 修复：用序列化 Promise 链防止 incrementStat 并发读写竞态
// chrome.storage 不提供事务，多个并发 get→modify→set 会导致写入丢失
let _statChain: Promise<void> = Promise.resolve();

export function incrementStat(field: "sent" | "failed"): Promise<void> {
  _statChain = _statChain.then(async () => {
    try {
      const today = new Date().toDateString();
      const result = await browser.storage.local.get("stats");
      let stats = (result.stats as DailyStats | undefined) ?? { sent: 0, failed: 0, date: "" };
      if (stats.date !== today) {
        stats = { sent: 0, failed: 0, date: today };
      }
      stats[field]++;
      await browser.storage.local.set({ stats });
    } catch {
      /* storage 失败不影响主流程 */
    }
  });
  return _statChain;
}
