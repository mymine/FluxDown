import { browser } from "wxt/browser";
import { ConnectionDiagnosticsStore, DIAGNOSTIC_MAX_AGE_MS, MAX_DIAGNOSTIC_ENTRIES } from "./connection-diagnostics-store";
import { recordConnectionDiagnostic, setDiagnosticRecorder } from "./connection-diagnostics";
import { nmhCheckFluxDownAvailableWithRetry } from "./native-messaging";
import { loadSettings } from "./settings";

export function installConnectionDiagnostics() {
  const store = new ConnectionDiagnosticsStore(browser.storage.local);
  setDiagnosticRecorder((event) => store.append(event, "background"));
  recordConnectionDiagnostic({ event: "background.started" });

  return async (message: { type: string; event?: unknown }, sender: { id?: string; url?: string }) => {
    const page = sender.url?.split(/[?#]/, 1)[0];
    const pages: Record<string, "popup" | "options" | "sidepanel"> = {
      [browser.runtime.getURL("/popup.html")]: "popup",
      [browser.runtime.getURL("/options.html")]: "options",
      [browser.runtime.getURL("/sidepanel.html")]: "sidepanel",
    };
    const source = page ? pages[page] ?? null : null;
    // Content scripts and web pages cannot read the journal or inject entries.
    if (sender.id !== browser.runtime.id || !source) {
      throw new Error("Diagnostics are only available to extension pages");
    }
    if (message.type === "diagnostics-record") {
      await store.append(message.event, source);
      return { ok: true };
    }

    // Pure liveness check: never warm up/start the desktop app or submit a task.
    const localReachable = await nmhCheckFluxDownAvailableWithRetry();
    const [settings, platform, events] = await Promise.all([
      loadSettings(), browser.runtime.getPlatformInfo(), store.snapshot(),
    ]);
    const manifest = browser.runtime.getManifest();
    const report = {
      format: "fluxdown-connection-diagnostics",
      schemaVersion: 1,
      exportedAt: new Date().toISOString(),
      extension: {
        id: browser.runtime.id,
        version: manifest.version,
        versionName: manifest.version_name,
        manifestVersion: manifest.manifest_version,
        nativeMessagingDeclared: manifest.permissions?.includes("nativeMessaging") === true,
      },
      browser: { userAgent: navigator.userAgent, platform },
      connection: {
        nativeHost: "com.fluxdown.nmh",
        localReachable,
        remoteMode: ["off", "fallback", "always"].includes(settings.remoteMode) ? settings.remoteMode : "unknown",
        remoteConfigured: Boolean(settings.remoteUrl),
        remoteTokenConfigured: Boolean(settings.remoteToken),
        remoteVerified: Boolean(settings.remoteVerified),
        interceptionEnabled: settings.enabled === true,
        protocolEnabled: settings.enableFluxdownProtocol === true,
      },
      retention: { maxEntries: MAX_DIAGNOSTIC_ENTRIES, maxAgeDays: DIAGNOSTIC_MAX_AGE_MS / 86_400_000 },
      events,
    };
    // Background owns the data URL and download. Closing popup/Save As cannot
    // revoke a page-owned blob midway through saving. Existing interceptors
    // already exclude data: URLs from takeover.
    await browser.downloads.download({
      url: `data:application/json;charset=utf-8,${encodeURIComponent(`${JSON.stringify(report, null, 2)}\n`)}`,
      filename: `fluxdown-connection-diagnostics-${new Date().toISOString().replace(/[:.]/g, "-")}.json`,
      saveAs: true,
    });
    return { ok: true };
  };
}
