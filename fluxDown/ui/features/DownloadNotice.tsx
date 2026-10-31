/**
 * 页内「浏览器下载」提示卡（顶层帧，由 sensor.content.ts 创建）。
 *
 * 拦截开启时浏览器下载 UI 被隐藏，FluxDown 放行给浏览器的下载（blob:/data:、SW 合成等）
 * 不在浏览器里留任何痕迹。background 在浏览器接下下载时发 started、结束时发 done / failed，
 * 这里在页面右下角显示，不依赖系统通知权限。首次收到提示才挂载 Shadow DOM。
 */
import { render } from "preact";
import type { JSX } from "preact";
import { signal } from "@preact/signals";
import type { ContentScriptContext } from "wxt/utils/content-script-context";
import { createShadowRootUi, type ShadowRootContentScriptUi } from "wxt/utils/content-script-ui/shadow-root";
import type { BrowserDownloadNotice } from "@/background/protocol";
import { contentCss } from "@/ui/content-css";
import { Icon } from "@/ui/components/Icon";
import { useT } from "@/ui/i18n";
import { useThemeAttr } from "@/ui/theme";
import { initI18n } from "@/utils/i18n";
import { sendAction } from "@/utils/messaging";
import "@/ui/theme/tokens.css";
import "@/ui/theme/base.css";
import "./download-notice.css";
import { logWarn } from "@/utils/error-log";

/** 完成卡片停留时长；进行中的卡片一直保留到完成 / 失败 / 手动关闭。 */
const DONE_LINGER_MS = 8000;
const MAX_CARDS = 4;

interface Card {
  downloadId: number;
  phase: "started" | "done";
  name: string;
}

const cards = signal<Card[]>([]);
const timers = new Map<number, ReturnType<typeof setTimeout>>();

function dismiss(downloadId: number): void {
  clearTimeout(timers.get(downloadId));
  timers.delete(downloadId);
  cards.value = cards.value.filter((c) => c.downloadId !== downloadId);
}

function apply(notice: BrowserDownloadNotice): void {
  if (notice.phase === "failed") {
    dismiss(notice.downloadId);
    return;
  }
  const card: Card = { downloadId: notice.downloadId, phase: notice.phase, name: notice.name };
  const rest = cards.value.filter((c) => c.downloadId !== notice.downloadId);
  cards.value = [...rest, card].slice(-MAX_CARDS);
  clearTimeout(timers.get(notice.downloadId));
  timers.delete(notice.downloadId);
  if (notice.phase === "done") timers.set(notice.downloadId, setTimeout(() => dismiss(notice.downloadId), DONE_LINGER_MS));
}

function showInFolder(downloadId: number): void {
  sendAction({ action: "showBrowserDownload", downloadId })
    .then(() => dismiss(downloadId))
    .catch((e: unknown) => logWarn("[FluxDown] showBrowserDownload failed:", e));
}

function NoticeRoot(props: { host: HTMLElement }): JSX.Element | null {
  const t = useT();
  useThemeAttr(props.host);
  const list = cards.value;
  if (list.length === 0) return null;
  return (
    <div class="fd-dlnotice-stack" role="status" aria-live="polite">
      {list.map((card) => {
        const done = card.phase === "done";
        return (
          <div class="fd-dlnotice" data-phase={card.phase} key={card.downloadId}>
            <span class="fd-dlnotice-icon">
              <Icon name={done ? "checkCircle" : "download"} size={18} />
            </span>
            <div class="fd-dlnotice-body">
              <div class="fd-dlnotice-title">
                {t(done ? "notify.browserDownloadDoneTitle" : "notify.browserDownloadStartedTitle")}
              </div>
              <div class="fd-dlnotice-name" title={card.name}>
                {card.name}
              </div>
              <div class="fd-dlnotice-hint">{t("browserNotice.hint")}</div>
              {done && (
                <button type="button" class="fd-dlnotice-action" onClick={() => showInFolder(card.downloadId)}>
                  <Icon name="folder" size={14} />
                  {t("browserNotice.showInFolder")}
                </button>
              )}
            </div>
            <button
              type="button"
              class="fd-dlnotice-close"
              aria-label={t("ui.close")}
              onClick={() => dismiss(card.downloadId)}
            >
              <Icon name="close" size={14} />
            </button>
            {!done && <span class="fd-dlnotice-progress" />}
          </div>
        );
      })}
    </div>
  );
}

export interface DownloadNoticeController {
  /** 显示 / 更新提示；返回是否已显示（false 时 background 退回系统通知）。 */
  handle(notice: BrowserDownloadNotice): Promise<boolean>;
  destroy(): void;
}

export function createDownloadNotice(ctx: ContentScriptContext): DownloadNoticeController {
  let ui: ShadowRootContentScriptUi<HTMLElement> | null = null;
  let mounting: Promise<boolean> | null = null;

  async function mount(): Promise<boolean> {
    try {
      await initI18n();
      const created = await createShadowRootUi<HTMLElement>(ctx, {
        name: "fluxdown-download-notice",
        css: contentCss(),
        position: "inline",
        anchor: () => document.documentElement,
        append: "last",
        onMount(container, _shadow, host) {
          render(<NoticeRoot host={host} />, container);
          return container;
        },
        onRemove(container) {
          if (container) render(null, container);
        },
      });
      created.mount();
      ui = created;
      return true;
    } catch (e) {
      logWarn("[FluxDown] download notice mount failed:", e);
      return false;
    }
  }

  return {
    async handle(notice) {
      if (!ui) {
        // 进行中卡片只在可见页面上有意义；后台标签页交给系统通知
        if (notice.phase === "failed") return true;
        if (document.visibilityState !== "visible") return false;
        mounting ??= mount().finally(() => {
          mounting = null;
        });
        if (!(await mounting)) return false;
      }
      apply(notice);
      return true;
    },
    destroy() {
      for (const timer of timers.values()) clearTimeout(timer);
      timers.clear();
      ui?.remove();
      ui = null;
    },
  };
}
