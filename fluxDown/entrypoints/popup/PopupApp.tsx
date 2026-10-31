/**
 * Popup 根组件（380×560），首屏不需整页滚动：
 * 头部（连接状态 + 总拦截开关）→ 站点栏（默认收起一行）→ 资源 / 任务页签（仅页签内容区滚动）→ 页脚。
 * 默认页签按首个 TabView 决定：本页嗅探到资源则「资源」，否则「任务」；用户切换后不再自动改。
 * 资源/预览逻辑全部由 ui/features 提供；本文件只负责组装与 popup 独有的状态。
 */
import type { JSX } from "preact";
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import type { TabView } from "@/background/protocol";
import { sendAction } from "@/utils/messaging";
import { exportConnectionDiagnostics } from "@/utils/connection-diagnostics";
import { loadSettings, saveSettings, type BypassKey } from "@/utils/settings";
import { Button } from "@/ui/components/Button";
import { HostAccessNotice } from "@/ui/components/HostAccessNotice";
import { Icon } from "@/ui/components/Icon";
import { IconButton } from "@/ui/components/IconButton";
import { Tabs, type TabItem } from "@/ui/components/Tabs";
import { ToastHost, showToast } from "@/ui/components/Toast";
import { Toggle } from "@/ui/components/Toggle";
import { ResourceList } from "@/ui/features/ResourceList";
import { SiteCard } from "@/ui/features/SiteCard";
import { QuickDownload } from "@/ui/features/QuickDownload";
import { TaskList } from "@/ui/features/TaskList";
import { usePlatform } from "@/ui/hooks/usePlatform";
import { useSidePanelOpener } from "@/ui/hooks/useSidePanelOpener";
import { useTabView } from "@/ui/hooks/useTabView";
import { useT } from "@/ui/i18n";
import { useThemeAttr } from "@/ui/theme";
import "@/ui/theme/tokens.css";
import "@/ui/theme/base.css";
import "./popup.css";
import { PageScriptsHint } from "./PageScriptsHint";

/** 摘要中除「当前播放」外最多显示的行数。 */
const SUMMARY_EXTRA_ROWS = 5;

type ConnectionState = "checking" | "connected" | "disconnected";
type PaneKey = "resources" | "tasks";

interface DailyStats {
  sent?: number;
  failed?: number;
  date?: string;
}

/**
 * 头部连接状态：与侧边栏同取 background 的 getHostStatus（下载实际走的共享 NMH 连接 / 远程路由），
 * 两处显示同一事实。
 * 并发守卫：连续触发刷新时只让最后一次的结果落到 UI（epoch 递增，旧结果丢弃）。
 */
function useConnection(): { state: ConnectionState; refresh: () => void } {
  const [state, setState] = useState<ConnectionState>("checking");
  const epochRef = useRef(0);

  const refresh = useCallback(() => {
    const epoch = ++epochRef.current;
    setState("checking");
    sendAction<"getHostStatus">({ action: "getHostStatus" })
      .then((status) => status.connected)
      .catch(() => false)
      .then((available) => {
        if (epoch !== epochRef.current) return;
        setState(available ? "connected" : "disconnected");
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { state, refresh };
}

/** 当前窗口活动 tab 的 id；查询不到时为 null。 */
function useActiveTabId(): number | null {
  const [tabId, setTabId] = useState<number | null>(null);
  useEffect(() => {
    let alive = true;
    browser.tabs
      .query({ active: true, currentWindow: true })
      .then((tabs) => {
        if (alive) setTabId(tabs[0]?.id ?? null);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  return tabId;
}

/** 今日统计；日期不是今天则视为 0（跨天归零由 background/stats 在下次累计时落盘）。 */
function useStats(): { sent: number; failed: number } {
  const [stats, setStats] = useState({ sent: 0, failed: 0 });
  useEffect(() => {
    let alive = true;
    browser.storage.local
      .get("stats")
      .then((r) => {
        const raw = r.stats as DailyStats | undefined;
        if (!alive || !raw || raw.date !== new Date().toDateString()) return;
        setStats({ sent: raw.sent || 0, failed: raw.failed || 0 });
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  return stats;
}

/** 拦截总开关（settings.enabled）与跳过接管修饰键（页脚提示用）。 */
function useInterceptEnabled(): [boolean, (next: boolean) => void, BypassKey] {
  const [enabled, setEnabled] = useState(true);
  const [bypassKey, setBypassKey] = useState<BypassKey>("off");
  useEffect(() => {
    let alive = true;
    loadSettings()
      .then((s) => {
        if (!alive) return;
        setEnabled(s.enabled);
        setBypassKey(s.bypassKey);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  const update = (next: boolean) => {
    setEnabled(next);
    saveSettings({ enabled: next }).catch(() => setEnabled(!next));
  };
  return [enabled, update, bypassKey];
}

/** 修饰键在当前平台上的叫法：macOS 的 Alt 是 Option。 */
function bypassKeyLabel(key: BypassKey): string {
  if (key === "shift") return "Shift";
  return /Mac/i.test(navigator.userAgent) ? "Option" : "Alt";
}

/** 资源摘要用的视图：playing 全留，其余候选与文件合计只取前 N 行；truncated = 有行被省略。 */
function summarize(view: TabView): { summary: TabView; total: number; truncated: boolean } {
  const total =
    view.playing.length + view.otherElements.length + view.unbound.length + view.files.length;
  let budget = SUMMARY_EXTRA_ROWS;
  const take = <T,>(rows: T[]): T[] => {
    const kept = rows.slice(0, budget);
    budget -= kept.length;
    return kept;
  };
  const otherElements = take(view.otherElements);
  const unbound = take(view.unbound);
  const files = take(view.files);
  const truncated = total > view.playing.length + SUMMARY_EXTRA_ROWS;
  return { summary: { ...view, otherElements, unbound, files }, total, truncated };
}

export function PopupApp(): JSX.Element {
  useThemeAttr(document.documentElement);
  const t = useT();
  const connection = useConnection();
  const [enabled, setEnabled, bypassKey] = useInterceptEnabled();
  const tabId = useActiveTabId();
  const view = useTabView(tabId).value;
  const stats = useStats();
  const sidePanel = useSidePanelOpener(true);
  const [exporting, setExporting] = useState(false);
  const platform = usePlatform();

  const manifest = browser.runtime.getManifest();
  const version = manifest.version_name ?? `v${manifest.version}`;
  const summary = view ? summarize(view) : null;
  const resourceTotal = summary?.total ?? 0;
  // 扩展页、chrome:// 等非网页没有站点语义（host 会是扩展 id），不显示站点栏
  const siteHost = view && /^https?:/i.test(view.pageUrl) ? view.site.host : "";
  const [activeTasks, setActiveTasks] = useState(0);
  const [pane, setPane] = useState<PaneKey | null>(null);
  const viewLoaded = view !== null;
  useEffect(() => {
    if (pane === null && viewLoaded) setPane(resourceTotal > 0 || platform.android ? "resources" : "tasks");
  }, [pane, viewLoaded, resourceTotal, platform.android]);
  const current: PaneKey = platform.android ? "resources" : (pane ?? "resources");
  const tabs: TabItem<PaneKey>[] = [
    { key: "resources", label: t("popup.tabs.resources"), count: resourceTotal },
    ...(platform.android ? [] : [{ key: "tasks" as const, label: t("popup.tabs.tasks"), count: activeTasks }]),
  ];

  const exportDiagnostics = async () => {
    setExporting(true);
    try {
      await exportConnectionDiagnostics();
      showToast(t("diagnostics.exportStarted"), "success");
    } catch {
      showToast(t("diagnostics.exportFailed"), "error");
    } finally {
      setExporting(false);
    }
  };

  const switchLabel = `${t("switch.label")} · ${enabled ? t("switch.enabled") : t("switch.disabled")}`;
  // Android 的「已连接」只代表下载会交给 FluxDown Android App（Intent），不是桌面连接
  const statusLabel =
    platform.android && connection.state === "connected" ? t("header.androidHandoff") : t(`header.${connection.state}`);

  return (
    <div class="pp-root">
      <header class="pp-header">
        <div class="pp-logo">
          <img src="/icon/fluxdown_logo.png" alt="FluxDown" width={22} height={22} />
          <span class="pp-title">FluxDown</span>
        </div>
        <div class="pp-header-right">
          <button
            type="button"
            class={`pp-status pp-${connection.state}`}
            title={statusLabel}
            onClick={connection.refresh}
          >
            <span class="pp-status-dot" />
            <span>{statusLabel}</span>
          </button>
          <Toggle checked={enabled} onChange={setEnabled} label={switchLabel} />
        </div>
      </header>

      <HostAccessNotice />
      {platform.desktopApp ? <PageScriptsHint /> : null}

      {siteHost ? (
        <div class="pp-site">
          <SiteCard host={siteHost} collapsible />
        </div>
      ) : null}

      {tabs.length > 1 ? <Tabs class="pp-tabs" items={tabs} active={current} onChange={setPane} /> : null}

      <main class="pp-content">
        {/* Android：任务列表不可用，快速下载直接置顶 */}
        {platform.android ? (
          <div class="pp-quick">
            <QuickDownload />
          </div>
        ) : null}
        <section class={`pp-pane${current === "resources" ? "" : " fd-hidden"}`}>
          {summary && summary.total > 0 ? (
            <>
              <ResourceList view={summary.summary} density="compact" showSearch={false} />
              {sidePanel.available && summary.truncated ? (
                <Button variant="ghost" size="sm" class="pp-view-all" onClick={sidePanel.open}>
                  {t("popup.viewAll", { count: String(summary.total) })}
                </Button>
              ) : null}
            </>
          ) : (
            <div class="pp-empty">{t("popup.resources.empty")}</div>
          )}
        </section>

        {/* 任务页签隐藏时仍挂载：持续轮询以维持页签计数；Android 无 NMH 任务列表（快速下载见上方） */}
        {platform.android ? null : (
          <section class={`pp-pane${current === "tasks" ? "" : " fd-hidden"}`}>
            <TaskList density="compact" quickDownload onActiveCountChange={setActiveTasks} />
          </section>
        )}
      </main>

      <footer class="pp-footer">
        <div class="pp-footer-left">
          <IconButton
            icon="settings"
            label={t("footer.settings")}
            onClick={() => {
              void browser.runtime.openOptionsPage();
              window.close();
            }}
          />
          {sidePanel.available ? (
            <IconButton icon="sidebar" label={t("popup.sidePanel")} onClick={sidePanel.open} />
          ) : null}
          {/* 导出诊断依赖 downloads API 与 NMH 侧数据，Android 不可用 */}
          {platform.desktopApp ? (
            <button
              type="button"
              class="pp-link"
              disabled={exporting}
              title={t("diagnostics.description")}
              onClick={exportDiagnostics}
            >
              {exporting ? t("diagnostics.exporting") : t("diagnostics.export")}
            </button>
          ) : null}
        </div>
        <div class="pp-footer-right">
          {/* 跳过接管修饰键是键盘操作，触屏无意义 */}
          {platform.desktopApp && enabled && bypassKey !== "off" ? (
            <span class="pp-bypass-hint" title={t("popup.bypassHintTip", { key: bypassKeyLabel(bypassKey) })}>
              {t("popup.bypassHint", { key: bypassKeyLabel(bypassKey) })}
            </span>
          ) : null}
          <span class="pp-stats" title={t("stats.title")}>
            <Icon name="download" size={11} />
            <span class="pp-stat-ok">{stats.sent}</span>
            <span class="pp-stat-sep">·</span>
            <span class="pp-stat-fail">{stats.failed}</span>
          </span>
          <span class="pp-version">{version}</span>
        </div>
      </footer>
      <ToastHost />
    </div>
  );
}
