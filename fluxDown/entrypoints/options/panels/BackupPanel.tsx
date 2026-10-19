import type { JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import { Button } from "@/ui/components/Button";
import { showToast } from "@/ui/components/Toast";
import { useT } from "@/ui/i18n";
import { applyBackup, backupFileName, collectBackup, parseBackup, type ParsedBackup } from "../lib/backup";
import { downloadJson } from "../lib/download";
import { refreshSettings } from "../lib/store";
import { ConfirmModal, PanelShell, SettingRow } from "../parts";

export function BackupPanel(): JSX.Element {
  const t = useT();
  const fileInput = useRef<HTMLInputElement>(null);
  const [pending, setPending] = useState<ParsedBackup | null>(null);

  const exportBackup = async () => {
    try {
      const now = new Date();
      downloadJson(backupFileName(now), await collectBackup(now));
      showToast(t("backup.exportDone"), "success");
    } catch (e) {
      console.error("[FluxDown Options] export backup failed:", e);
      showToast(t("backup.exportFailed"), "error");
    }
  };

  const pickFile = async (file: File | undefined) => {
    if (!file) return;
    const parsed = parseBackup(await file.text());
    if (!parsed.ok) {
      const messages = {
        json: t("backup.errJson"),
        format: t("backup.errFormat"),
        version: t("backup.errVersion"),
      };
      showToast(messages[parsed.error], "error");
      return;
    }
    setPending(parsed.backup);
  };

  const confirmImport = async () => {
    if (!pending) return;
    try {
      await applyBackup(pending);
      await refreshSettings();
      showToast(t("backup.importDone"), "success");
    } catch (e) {
      console.error("[FluxDown Options] import backup failed:", e);
      showToast(t("backup.importFailed"), "error");
    } finally {
      setPending(null);
    }
  };

  return (
    <PanelShell title={t("options.nav.backup")} desc={t("backup.desc")}>
      <SettingRow title={t("backup.exportTitle")} desc={t("backup.exportDesc")} inline>
        <Button variant="primary" onClick={() => void exportBackup()}>
          {t("backup.export")}
        </Button>
      </SettingRow>

      <SettingRow title={t("backup.importTitle")} desc={t("backup.importDesc")} inline>
        <input
          ref={fileInput}
          type="file"
          accept="application/json,.json"
          class="fd-hidden"
          onChange={(e) => {
            const input = e.currentTarget;
            void pickFile(input.files?.[0]).finally(() => {
              input.value = "";
            });
          }}
        />
        <Button onClick={() => fileInput.current?.click()}>{t("backup.import")}</Button>
      </SettingRow>

      {pending ? (
        <ConfirmModal
          title={t("backup.confirmTitle")}
          confirmLabel={t("backup.confirmImport")}
          danger
          onCancel={() => setPending(null)}
          onConfirm={() => void confirmImport()}
          message={
            <>
              <p>{t("backup.confirmMessage")}</p>
              <ul class="opt-summary">
                {pending.exportedAt ? <li>{t("backup.sumExportedAt", { time: new Date(pending.exportedAt).toLocaleString() })}</li> : null}
                <li>{t("backup.sumSites", { count: String(Object.keys(pending.siteProfiles).length) })}</li>
                <li>{t("backup.sumRules", { count: String(pending.localRules.length) })}</li>
                <li>{t("backup.sumOverrides", { count: String(Object.keys(pending.ruleOverrides).length) })}</li>
              </ul>
              {pending.rejectedRules.length > 0 ? (
                <div class="opt-warn">
                  <strong>{t("backup.sumRejected", { count: String(pending.rejectedRules.length) })}</strong>
                  <ul>
                    {pending.rejectedRules.map((r) => (
                      <li key={r.id}>
                        <code>{r.id || "?"}</code> {r.reason}
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}
              <p class="opt-field-hint">{t("backup.tokenNote")}</p>
            </>
          }
        />
      ) : null}
    </PanelShell>
  );
}
