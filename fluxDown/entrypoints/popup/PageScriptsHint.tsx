/** popup 里的页面脚本提示行：有待启用的插件，或已启用但浏览器开关 / 权限未就绪时显示，链接到选项页对应面板。 */
import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import type { PageScriptsStatus } from "@/background/page-scripts/model";
import { useT } from "@/ui/i18n";
import { sendAction } from "@/utils/messaging";

type Hint = Pick<PageScriptsStatus, "attention" | "pendingCount">;

export function PageScriptsHint(): JSX.Element | null {
  const t = useT();
  const [hint, setHint] = useState<Hint | null>(null);

  useEffect(() => {
    let alive = true;
    // stale：缓存新于 1 分钟不重新同步，popup 打开不会额外打扰桌面端。
    sendAction<"pageScriptsStatus">({ action: "pageScriptsStatus", refresh: "stale" })
      .then((status) => {
        if (alive) setHint({ attention: status.attention, pendingCount: status.pendingCount });
      })
      .catch((e) => console.debug("[FluxDown Popup] pageScriptsStatus failed:", e));
    return () => {
      alive = false;
    };
  }, []);

  if (!hint || hint.attention === null) return null;
  const text =
    hint.attention === "switch"
      ? t("popup.pageScripts.switch")
      : t("popup.pageScripts.pending", { count: String(hint.pendingCount) });

  return (
    <div class="pp-ps-hint" role="status">
      <span>{text}</span>
      <button
        type="button"
        class="pp-link"
        onClick={() => {
          void browser.tabs.create({ url: browser.runtime.getURL("/options.html#pageScripts") });
          window.close();
        }}
      >
        {t("popup.pageScripts.open")}
      </button>
    </div>
  );
}
