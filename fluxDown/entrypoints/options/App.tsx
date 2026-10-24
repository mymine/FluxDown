import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import { HostAccessNotice } from "@/ui/components/HostAccessNotice";
import { Icon, type UiIconName } from "@/ui/components/Icon";
import { ToastHost } from "@/ui/components/Toast";
import { useSidePanelOpener } from "@/ui/hooks/useSidePanelOpener";
import { useT } from "@/ui/i18n";
import { useThemeAttr } from "@/ui/theme";
import type { MessageKey } from "@/utils/i18n";
import { PANELS, parseHash, type PanelId } from "./lib/route";
import { startSettingsSync } from "./lib/store";
import { AboutPanel } from "./panels/AboutPanel";
import { BackupPanel } from "./panels/BackupPanel";
import { GeneralPanel } from "./panels/GeneralPanel";
import { InterceptPanel } from "./panels/InterceptPanel";
import { RemotePanel } from "./panels/RemotePanel";
import { PageScriptsPanel } from "./panels/PageScriptsPanel";
import { RulesPanel } from "./panels/rules/RulesPanel";
import { ShortcutsPanel } from "./panels/ShortcutsPanel";
import { SitesPanel } from "./panels/SitesPanel";
import { SniffPanel } from "./panels/SniffPanel";

const NAV: Record<PanelId, { icon: UiIconName; label: MessageKey }> = {
  general: { icon: "settings", label: "options.nav.general" },
  intercept: { icon: "download", label: "options.nav.intercept" },
  sniff: { icon: "search", label: "options.nav.sniff" },
  sites: { icon: "globe", label: "options.nav.sites" },
  rules: { icon: "link", label: "options.nav.rules" },
  pageScripts: { icon: "play", label: "options.nav.pageScripts" },
  remote: { icon: "upload", label: "options.nav.remote" },
  shortcuts: { icon: "keyboard", label: "options.nav.shortcuts" },
  backup: { icon: "folder", label: "options.nav.backup" },
  about: { icon: "external", label: "options.nav.about" },
};

export function App(): JSX.Element {
  const t = useT();
  useThemeAttr(document.documentElement);
  const sidePanel = useSidePanelOpener(false);
  const [route, setRoute] = useState(() => parseHash(location.hash));

  useEffect(() => {
    const stopSync = startSettingsSync();
    const onHash = () => setRoute(parseHash(location.hash));
    window.addEventListener("hashchange", onHash);
    return () => {
      stopSync();
      window.removeEventListener("hashchange", onHash);
    };
  }, []);

  useEffect(() => {
    document.title = t("options.title");
  });

  const go = (panel: PanelId) => {
    history.replaceState(null, "", `#${panel}`);
    setRoute({ panel, params: new URLSearchParams() });
  };

  const manifest = browser.runtime.getManifest();
  const version = manifest.version_name ?? `v${manifest.version}`;
  const prefillHost = route.panel === "sites" ? route.params.get("host") : null;

  let content: JSX.Element;
  switch (route.panel) {
    case "intercept":
      content = <InterceptPanel />;
      break;
    case "sniff":
      content = <SniffPanel />;
      break;
    case "sites":
      content = <SitesPanel prefillHost={prefillHost} onPrefillConsumed={() => go("sites")} />;
      break;
    case "rules":
      content = <RulesPanel />;
      break;
    case "pageScripts":
      content = <PageScriptsPanel />;
      break;
    case "remote":
      content = <RemotePanel />;
      break;
    case "shortcuts":
      content = <ShortcutsPanel />;
      break;
    case "backup":
      content = <BackupPanel />;
      break;
    case "about":
      content = <AboutPanel />;
      break;
    default:
      content = <GeneralPanel />;
  }

  return (
    <div class="opt-layout">
      <aside class="opt-sidebar">
        <div class="opt-logo">
          <img src="/icon/fluxdown_logo.png" alt="FluxDown" width={24} height={24} />
          <span class="opt-logo-title">FluxDown</span>
        </div>

        <div class="opt-nav-group">{t("options.navSettings")}</div>
        <nav class="opt-nav">
          {PANELS.map((id) => (
            <button
              key={id}
              type="button"
              class={`opt-nav-item${route.panel === id ? " active" : ""}`}
              onClick={() => go(id)}
            >
              <Icon name={NAV[id].icon} size={15} />
              <span>{t(NAV[id].label)}</span>
            </button>
          ))}
        </nav>

        <div class="opt-sidebar-footer">
          {sidePanel.available ? (
            <button type="button" class="opt-nav-item" onClick={sidePanel.open}>
              <Icon name="sidebar" size={15} />
              <span>{t("options.openSidePanel")}</span>
            </button>
          ) : null}
          <span class="opt-version">{version}</span>
        </div>
      </aside>

      <main class="opt-content">
        <HostAccessNotice flush />
        {content}
      </main>
      <ToastHost />
    </div>
  );
}
