/**
 * 侧边栏「诊断」页：桌面端连接 / 协议 / 解析插件 / 规则同步状态，
 * 当前页资源命中的规则聚合、扩展错误日志，以及调试日志与连接诊断导出。
 */
import type { JSX } from "preact";
import { useEffect, useMemo, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import type { HostStatus, RuleDocSummary, TabView } from "@/background/protocol";
import { exportConnectionDiagnostics } from "@/utils/connection-diagnostics";
import { stringifyResourceDebugLog } from "@/utils/resource-debug-log";
import { sendAction } from "@/utils/messaging";
import { Badge } from "@/ui/components/Badge";
import { Button } from "@/ui/components/Button";
import { Icon } from "@/ui/components/Icon";
import { showToast } from "@/ui/components/Toast";
import { resourceDisplayName } from "@/ui/features/labels";
import { useT } from "@/ui/i18n";
import { syncProblemLabel, userSyncProblems } from "../options/panels/rules/sync-labels";
import { ErrorLogCard } from "./ErrorLogCard";

interface Props {
  view: TabView | null;
  /** null = 尚未取得（检测中）。 */
  status: HostStatus | null;
  onRefresh: () => void;
}

interface RuleHit {
  ruleId: string;
  items: { name: string; url: string }[];
}

/** 遍历 view 的所有行，按 ruleId 聚合命中的资源。 */
function collectRuleHits(view: TabView | null, fallbackName: string): RuleHit[] {
  if (!view) return [];
  const hits = new Map<string, RuleHit["items"]>();
  const add = (ruleIds: string[], name: string, url: string) => {
    for (const id of ruleIds) {
      const items = hits.get(id) ?? [];
      items.push({ name, url });
      hits.set(id, items);
    }
  };
  for (const row of [...view.playing, ...view.otherElements, ...view.unbound]) {
    add(row.ruleIds, row.candidate.title, row.candidate.variants[0]?.videoUrl ?? row.candidate.pageUrl);
  }
  for (const row of view.files) {
    add(row.ruleIds, resourceDisplayName(row.resource, fallbackName), row.resource.url);
  }
  return [...hits.entries()]
    .map(([ruleId, items]) => ({ ruleId, items }))
    .sort((a, b) => b.items.length - a.items.length || a.ruleId.localeCompare(b.ruleId));
}

/** 把文本作为文件下载：优先 downloads API，Firefox 个别版本拒绝 blob: 时退回 <a download>。 */
async function saveTextFile(filename: string, text: string, mime: string): Promise<void> {
  const blobUrl = URL.createObjectURL(new Blob([text], { type: mime }));
  try {
    try {
      await browser.downloads.download({ url: blobUrl, filename, saveAs: true });
    } catch {
      const anchor = document.createElement("a");
      anchor.href = blobUrl;
      anchor.download = filename;
      anchor.click();
    }
  } finally {
    window.setTimeout(() => URL.revokeObjectURL(blobUrl), 30_000);
  }
}

function docLine(doc: RuleDocSummary | null, never: string): string {
  if (!doc) return never;
  const expires = doc.expiresAt ? ` · → ${doc.expiresAt.slice(0, 10)}` : "";
  return `#${doc.sequence} · ${doc.issuedAt.slice(0, 10)}${expires}`;
}

function Row(props: { label: string; children: JSX.Element | string }): JSX.Element {
  return (
    <div class="sp-diag-row">
      <span class="sp-diag-label">{props.label}</span>
      <span class="sp-diag-value">{props.children}</span>
    </div>
  );
}

export function DiagnosticsPane(props: Props): JSX.Element {
  const t = useT();
  const { view, status } = props;
  const [titles, setTitles] = useState<Map<string, string>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [exportingLog, setExportingLog] = useState(false);
  const [exportingConn, setExportingConn] = useState(false);

  // 规则标题仅用于展示，取不到时回退到 id
  useEffect(() => {
    let alive = true;
    sendAction<"rulesList">({ action: "rulesList" })
      .then((list) => {
        if (!alive) return;
        const next = new Map<string, string>();
        for (const e of list) next.set(e.rule.id, e.rule.title ?? "");
        setTitles(next);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  const hits = useMemo(() => collectRuleHits(view, t("sidepanel.diag.unnamed")), [view, t]);

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  const exportResourceLog = async () => {
    if (!view) return;
    setExportingLog(true);
    try {
      const log = await sendAction<"getDebugLog">({ action: "getDebugLog", tabId: view.tabId });
      const stamp = new Date().toISOString().replace(/[.:]/g, "-");
      await saveTextFile(
        `fluxdown-resource-debug-${stamp}.json`,
        stringifyResourceDebugLog(log),
        "application/json",
      );
      showToast(t("panel.exportDebugLogDone"), "success");
    } catch {
      showToast(t("panel.exportDebugLogFailed"), "error");
    } finally {
      setExportingLog(false);
    }
  };

  const exportConnection = async () => {
    setExportingConn(true);
    try {
      await exportConnectionDiagnostics();
      showToast(t("diagnostics.exportStarted"), "success");
    } catch {
      showToast(t("diagnostics.exportFailed"), "error");
    } finally {
      setExportingConn(false);
    }
  };

  const never = t("sidepanel.diag.never");
  const sync = status?.ruleSync;
  const resolvers = status?.capabilities.resolvers ?? [];
  const pluginText = !status
    ? "—"
    : status.protocol < 2
      ? t("handoff.unavailable.old-app")
      : status.capabilities.pluginsSupported
        ? t("sidepanel.diag.pluginCount", {
            total: String(resolvers.length),
            enabled: String(resolvers.filter((r) => r.enabled && r.loaded).length),
          })
        : t("handoff.unavailable.plugins-unsupported");

  return (
    <div class="sp-diag">
      <section class="sp-card">
        <div class="sp-card-head">
          <span>{t("sidepanel.diag.connection")}</span>
          <Button variant="ghost" size="sm" onClick={props.onRefresh}>
            <Icon name="refresh" size={12} /> {t("sidepanel.diag.refresh")}
          </Button>
        </div>
        <Row label={t("sidepanel.diag.status")}>
          {!status ? (
            <Badge>{t("header.checking")}</Badge>
          ) : status.connected ? (
            <Badge tone="success">{t("header.connected")}</Badge>
          ) : (
            <Badge tone="danger">{t("header.disconnected")}</Badge>
          )}
        </Row>
        <Row label={t("sidepanel.diag.protocol")}>{status ? `v${status.protocol}` : "—"}</Row>
        <Row label={t("sidepanel.diag.plugins")}>{pluginText}</Row>
      </section>

      <section class="sp-card">
        <div class="sp-card-head">
          <span>{t("sidepanel.diag.ruleSync")}</span>
        </div>
        {sync ? (
          <>
            <Row label={t("sidepanel.diag.syncEnabled")}>
              {sync.enabled ? t("switch.enabled") : t("switch.disabled")}
            </Row>
            <Row label={t("sidepanel.diag.communityEnabled")}>
              {sync.communityEnabled ? t("switch.enabled") : t("switch.disabled")}
            </Row>
            <Row label={t("sidepanel.diag.official")}>{docLine(sync.official, never)}</Row>
            <Row label={t("sidepanel.diag.community")}>{docLine(sync.community, never)}</Row>
            <Row label={t("sidepanel.diag.lastSync")}>
              {sync.lastSyncAt ? new Date(sync.lastSyncAt).toLocaleString() : never}
            </Row>
            {userSyncProblems(sync.problems).length > 0 ? (
              <Row label={t("sidepanel.diag.lastError")}>
                <span class="sp-error">{userSyncProblems(sync.problems).map(syncProblemLabel).join("；")}</span>
              </Row>
            ) : null}
            {sync.host?.lastError ? (
              <Row label={t("sidepanel.diag.hostError")}>
                <span class="sp-error">{sync.host.lastError}</span>
              </Row>
            ) : null}
            {sync.compileError ? (
              <Row label={t("rules.sync.compileError")}>
                <span class="sp-error">{sync.compileError}</span>
              </Row>
            ) : null}
          </>
        ) : (
          <div class="sp-diag-none">—</div>
        )}
      </section>

      <section class="sp-card">
        <div class="sp-card-head">
          <span>{t("sidepanel.diag.pageRules")}</span>
        </div>
        {hits.length === 0 ? (
          <div class="sp-diag-none">{t("sidepanel.diag.noRules")}</div>
        ) : (
          hits.map((hit) => {
            const open = expanded.has(hit.ruleId);
            const title = titles.get(hit.ruleId);
            return (
              <div key={hit.ruleId} class="sp-rule">
                <button type="button" class="sp-rule-head" onClick={() => toggle(hit.ruleId)}>
                  <Icon name={open ? "chevronDown" : "chevronRight"} size={12} />
                  <span class="sp-rule-id" title={hit.ruleId}>
                    {title ? `${title} · ${hit.ruleId}` : hit.ruleId}
                  </span>
                  <Badge tone="accent">{hit.items.length}</Badge>
                </button>
                {open ? (
                  <ul class="sp-rule-items">
                    {hit.items.map((it, i) => (
                      <li key={`${it.url}#${i}`}>
                        <div class="sp-rule-name">{it.name}</div>
                        <div class="sp-rule-url" title={it.url}>
                          {it.url}
                        </div>
                      </li>
                    ))}
                  </ul>
                ) : null}
              </div>
            );
          })
        )}
      </section>

      <ErrorLogCard />

      <div class="sp-diag-actions">
        <Button disabled={!view || exportingLog} onClick={exportResourceLog}>
          {t("sidepanel.diag.exportLog")}
        </Button>
        <Button disabled={exportingConn} title={t("diagnostics.description")} onClick={exportConnection}>
          {exportingConn ? t("diagnostics.exporting") : t("sidepanel.diag.exportConn")}
        </Button>
      </div>
    </div>
  );
}
