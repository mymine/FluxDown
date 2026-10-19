import type { JSX } from "preact";
import { browser } from "wxt/browser";
import { Icon } from "@/ui/components/Icon";
import { useT } from "@/ui/i18n";
import { PanelShell, SettingRow } from "../parts";

const LINKS = [
  { key: "options.about.website", href: "https://fluxdown.com/" },
  { key: "options.about.github", href: "https://github.com/zerx-lab/FluxDown" },
  { key: "options.about.releases", href: "https://github.com/zerx-lab/FluxDown/releases/latest" },
  { key: "options.about.feedback", href: "https://fluxdown.com/feedback" },
] as const;

export function AboutPanel(): JSX.Element {
  const t = useT();
  const manifest = browser.runtime.getManifest();
  const version = manifest.version_name ?? `v${manifest.version}`;

  return (
    <PanelShell title={t("options.nav.about")}>
      <SettingRow title={t("options.about.version")} desc={t("options.about.versionDesc")} inline>
        <span class="opt-cell-mono">{version}</span>
      </SettingRow>
      <SettingRow title={t("options.about.links")} stack>
        <ul class="opt-links">
          {LINKS.map((link) => (
            <li key={link.href}>
              <a href={link.href} target="_blank" rel="noreferrer noopener">
                <Icon name="external" size={14} />
                {t(link.key)}
              </a>
            </li>
          ))}
        </ul>
      </SettingRow>
    </PanelShell>
  );
}
