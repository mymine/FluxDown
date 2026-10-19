import type { JSX } from "preact";
import { Select } from "@/ui/components/Select";
import { useT } from "@/ui/i18n";
import { settings, updateSettings } from "../lib/store";
import { PanelShell, SettingRow, ToggleRow } from "../parts";

export function SniffPanel(): JSX.Element | null {
  const t = useT();
  const s = settings.value;
  if (!s) return null;

  return (
    <PanelShell title={t("options.nav.sniff")}>
      <ToggleRow
        title={t("options.sniff.enabled")}
        desc={t("options.sniff.enabledDesc")}
        checked={s.resourceSniffing}
        onChange={(v) => void updateSettings({ resourceSniffing: v })}
      />
      <ToggleRow
        title={t("options.sniff.pageDot")}
        desc={t("options.sniff.pageDotDesc")}
        checked={s.showPageDot}
        onChange={(v) => void updateSettings({ showPageDot: v })}
      />
      <ToggleRow
        title={t("options.sniff.floatButton")}
        desc={t("options.sniff.floatButtonDesc")}
        checked={s.showFloatingButton}
        onChange={(v) => void updateSettings({ showFloatingButton: v })}
      />
      <SettingRow title={t("options.sniff.floatTrigger")} desc={t("options.sniff.floatTriggerDesc")}>
        <Select<"hover" | "always">
          value={s.floatButtonTrigger}
          disabled={!s.showFloatingButton}
          onChange={(v) => void updateSettings({ floatButtonTrigger: v })}
          options={[
            { value: "hover", label: t("options.sniff.triggerHover") },
            { value: "always", label: t("options.sniff.triggerAlways") },
          ]}
        />
      </SettingRow>
      <SettingRow title={t("options.sniff.floatPosition")} desc={t("options.sniff.floatPositionDesc")}>
        <Select<"top-right" | "top-left" | "bottom-right" | "bottom-left">
          value={s.floatButtonPosition}
          disabled={!s.showFloatingButton}
          onChange={(v) => void updateSettings({ floatButtonPosition: v })}
          options={[
            { value: "top-right", label: t("options.sniff.positionTopRight") },
            { value: "top-left", label: t("options.sniff.positionTopLeft") },
            { value: "bottom-right", label: t("options.sniff.positionBottomRight") },
            { value: "bottom-left", label: t("options.sniff.positionBottomLeft") },
          ]}
        />
      </SettingRow>
      <ToggleRow
        title={t("options.sniff.images")}
        desc={t("options.sniff.imagesDesc")}
        checked={s.sniffImages}
        onChange={(v) => void updateSettings({ sniffImages: v })}
      />
      <ToggleRow
        title={t("options.sniff.lowConfidence")}
        desc={t("options.sniff.lowConfidenceDesc")}
        checked={s.showLowConfidence}
        onChange={(v) => void updateSettings({ showLowConfidence: v })}
      />
    </PanelShell>
  );
}
