/**
 * 侧边栏「诊断」页的扩展错误卡片：只读 `storage.local` 里的扩展错误日志（变化时自动刷新），
 * 清空经 background 完成（写入方唯一）。
 */
import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import { clearErrorLog } from "@/utils/error-log";
import { ERROR_LOG_KEY, ErrorLogStore, type ErrorLogEntry } from "@/utils/error-log-store";
import { Badge } from "@/ui/components/Badge";
import { Button } from "@/ui/components/Button";
import { showToast } from "@/ui/components/Toast";
import { useT } from "@/ui/i18n";

/** 卡片里最多列出的条数（最新在前）；完整记录随连接诊断导出。 */
const MAX_SHOWN = 30;

export function ErrorLogCard(): JSX.Element {
  const t = useT();
  const [entries, setEntries] = useState<ErrorLogEntry[] | null>(null);
  const [clearing, setClearing] = useState(false);

  useEffect(() => {
    let alive = true;
    const store = new ErrorLogStore(browser.storage.local);
    const load = () => {
      store.snapshot().then(
        (list) => alive && setEntries(list.reverse()),
        () => alive && setEntries([]),
      );
    };
    const onChanged = (changes: Record<string, unknown>, area: string) => {
      if (area === "local" && ERROR_LOG_KEY in changes) load();
    };
    load();
    browser.storage.onChanged.addListener(onChanged);
    return () => {
      alive = false;
      browser.storage.onChanged.removeListener(onChanged);
    };
  }, []);

  const clear = async () => {
    setClearing(true);
    try {
      await clearErrorLog();
      showToast(t("sidepanel.diag.errorLogCleared"), "success");
    } catch {
      showToast(t("sidepanel.diag.errorLogClearFailed"), "error");
    } finally {
      setClearing(false);
    }
  };

  const total = entries?.length ?? 0;
  return (
    <section class="sp-card">
      <div class="sp-card-head">
        <span title={t("sidepanel.diag.errorLogHint")}>{t("sidepanel.diag.errorLog")}</span>
        <span class="sp-errlog-actions">
          {total > 0 ? <Badge tone="danger">{total}</Badge> : null}
          <Button variant="ghost" size="sm" disabled={clearing || total === 0} onClick={clear}>
            {t("sidepanel.diag.errorLogClear")}
          </Button>
        </span>
      </div>
      {total === 0 ? (
        <div class="sp-diag-none">{entries ? t("sidepanel.diag.errorLogEmpty") : "—"}</div>
      ) : (
        <ul class="sp-errlog-list">
          {entries?.slice(0, MAX_SHOWN).map((entry) => (
            <li key={`${entry.source}|${entry.level}|${entry.message}`} class="sp-errlog-item">
              <div class="sp-errlog-meta">
                <Badge tone={entry.level === "error" ? "danger" : "warning"}>
                  {t(`sidepanel.diag.errorLevel.${entry.level}`)}
                </Badge>
                <span>{t(`sidepanel.diag.errorSource.${entry.source}`)}</span>
                <span>{new Date(entry.lastAt).toLocaleString()}</span>
                {entry.count > 1 ? <span>×{entry.count}</span> : null}
              </div>
              <div class="sp-errlog-msg">{entry.message}</div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
