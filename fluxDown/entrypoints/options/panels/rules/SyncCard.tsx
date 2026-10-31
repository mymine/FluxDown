import type { JSX } from "preact";
import { useState } from "preact/hooks";
import type { RuleDocSummary, RuleSyncStatus } from "@/background/protocol";
import { Badge } from "@/ui/components/Badge";
import { Button } from "@/ui/components/Button";
import { Icon } from "@/ui/components/Icon";
import { showToast } from "@/ui/components/Toast";
import { Toggle } from "@/ui/components/Toggle";
import { useT } from "@/ui/i18n";
import { docExpiry } from "@/utils/doc-expiry";
import { sendAction } from "@/utils/messaging";
import { settings, updateSettings } from "../../lib/store";
import { syncOutcomeLabels, syncProblemLabel, userSyncProblems } from "./sync-labels";
import { logError } from "@/utils/error-log";

interface SyncCardProps {
  sync: RuleSyncStatus | null;
  onSynced: (status: RuleSyncStatus) => void;
}

/** ISO 时间 → 本地化显示；无法解析时原样返回。 */
function formatIso(iso: string | undefined): string {
  if (!iso) return "—";
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? iso : new Date(ms).toLocaleString();
}

export function SyncCard(props: SyncCardProps): JSX.Element | null {
  const t = useT();
  const s = settings.value;
  const [syncing, setSyncing] = useState(false);
  if (!s) return null;

  const { sync } = props;
  const syncNow = async () => {
    setSyncing(true);
    try {
      const { outcome, status } = await sendAction<"rulesSyncNow">({ action: "rulesSyncNow" });
      props.onSynced(status);
      const [msg, kind] = syncOutcomeLabels()[outcome];
      showToast(msg, kind);
    } catch (e) {
      logError("[FluxDown Options] rulesSyncNow failed:", e);
      showToast(t("rules.sync.failed"), "error");
    } finally {
      setSyncing(false);
    }
  };

  const docs: [string, RuleDocSummary | null | undefined, boolean][] = [
    [t("rules.sync.official"), sync?.official, false],
    [t("rules.sync.community"), sync?.community, true],
    [t("rules.sync.plugin"), sync?.plugin, false],
  ];
  const now = Date.now();
  const problems = userSyncProblems(sync?.problems ?? []);
  const host = sync?.host ?? null;
  const hostFetch = host?.lastFetchUnix;
  const outcome = sync?.lastOutcome ?? null;
  const outcomeText = outcome ? syncOutcomeLabels()[outcome][0] : t("rules.sync.never");

  return (
    <div class="opt-card">
      <div class="opt-card-head">
        <h2 class="opt-card-title">{t("rules.sync.title")}</h2>
        <Button variant="primary" size="sm" disabled={syncing || !s.ruleSyncEnabled} onClick={() => void syncNow()}>
          <Icon name="refresh" size={13} />
          {syncing ? t("rules.sync.syncing") : t("rules.sync.now")}
        </Button>
      </div>

      <table class="opt-table opt-table-compact">
        <thead>
          <tr>
            <th>{t("rules.sync.colPack")}</th>
            <th>{t("rules.sync.colCount")}</th>
            <th>{t("rules.sync.colIssued")}</th>
            <th>{t("rules.sync.colExpires")}</th>
            <th>{t("rules.sync.colSequence")}</th>
          </tr>
        </thead>
        <tbody>
          {docs.map(([label, doc, pausesCommunity]) => {
            const expiry = docExpiry(doc?.expiresAt, now);
            return (
              <tr key={label}>
                <td>{label}</td>
                <td class="opt-cell-mono">{doc ? doc.count : "—"}</td>
                <td>{doc ? formatIso(doc.issuedAt) : t("rules.sync.none")}</td>
                <td>
                  {doc ? formatIso(doc.expiresAt) : "—"}
                  {expiry.state === "expired" ? (
                    <Badge tone="danger" title={pausesCommunity ? t("rules.sync.expiryHint") : undefined}>
                      {t("rules.sync.expired")}
                    </Badge>
                  ) : expiry.state === "soon" ? (
                    <Badge tone="warning" title={pausesCommunity ? t("rules.sync.expiryHint") : undefined}>
                      {t("rules.sync.expiresIn", { n: String(expiry.days) })}
                    </Badge>
                  ) : null}
                </td>
                <td class="opt-cell-mono">{doc ? doc.sequence : "—"}</td>
              </tr>
            );
          })}
        </tbody>
      </table>

      <div role="status" aria-live="polite">
        <dl class="opt-kv">
          <dt>{t("rules.sync.lastSync")}</dt>
          <dd>{sync?.lastSyncAt ? new Date(sync.lastSyncAt).toLocaleString() : t("rules.sync.never")}</dd>
          <dt>{t("rules.sync.lastResult")}</dt>
          <dd>
            {outcomeText}
            {sync?.lastAttemptAt ? ` · ${new Date(sync.lastAttemptAt).toLocaleString()}` : ""}
          </dd>
          {hostFetch ? (
            <>
              <dt>{t("rules.sync.hostFetch")}</dt>
              <dd>
                {new Date(hostFetch * 1000).toLocaleString()}
                {host?.refreshing ? ` · ${t("rules.sync.hostRefreshing")}` : ""}
              </dd>
            </>
          ) : null}
          <dt>{t("rules.sync.lastError")}</dt>
          <dd class={problems.length > 0 ? "opt-error-text" : ""}>
            {problems.length > 0
              ? problems.map((problem) => (
                  <div key={`${problem.code}:${problem.tier ?? ""}`}>{syncProblemLabel(problem)}</div>
                ))
              : t("rules.sync.noError")}
          </dd>
          {host?.lastError ? (
            <>
              <dt>{t("rules.sync.hostError")}</dt>
              <dd>{host.lastError}</dd>
            </>
          ) : null}
          {sync?.compileError ? (
            <>
              <dt>{t("rules.sync.compileError")}</dt>
              <dd class="opt-error-text">{sync.compileError}</dd>
            </>
          ) : null}
        </dl>
      </div>

      <div class="opt-card-toggles">
        <label class="opt-inline-toggle">
          <Toggle
            checked={s.ruleSyncEnabled}
            label={t("rules.sync.enabled")}
            onChange={(v) => void updateSettings({ ruleSyncEnabled: v })}
          />
          <span>
            <strong>{t("rules.sync.enabled")}</strong>
            <span class="opt-row-desc">{t("rules.sync.enabledDesc")}</span>
          </span>
        </label>
        <label class="opt-inline-toggle">
          <Toggle
            checked={s.communityRulesEnabled}
            label={t("rules.sync.communityEnabled")}
            onChange={(v) => void updateSettings({ communityRulesEnabled: v })}
          />
          <span>
            <strong>{t("rules.sync.communityEnabled")}</strong>
            <span class="opt-row-desc">{t("rules.sync.communityEnabledDesc")}</span>
          </span>
        </label>
      </div>
    </div>
  );
}
