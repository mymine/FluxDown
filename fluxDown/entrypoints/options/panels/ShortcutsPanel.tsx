import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import { Button } from "@/ui/components/Button";
import { IconButton } from "@/ui/components/IconButton";
import { showToast } from "@/ui/components/Toast";
import { useT } from "@/ui/i18n";
import type { MessageKey } from "@/utils/i18n";
import { captureShortcut, shortcutId, shortcutKeys, shortcutSettingsUrl } from "../lib/shortcuts";
import { PanelShell, SettingRow } from "../parts";

/** Firefox 独有的可写 commands API；Chromium 系只能在浏览器自带的快捷键页修改。 */
interface CommandsWriteApi {
  update(detail: { name: string; shortcut: string }): Promise<void>;
  reset(name: string): Promise<void>;
  onChanged?: { addListener(cb: () => void): void; removeListener(cb: () => void): void };
}

const commandsApi = browser.commands as unknown as Partial<CommandsWriteApi>;
const WRITABLE: CommandsWriteApi | null =
  typeof commandsApi.update === "function" && typeof commandsApi.reset === "function"
    ? (commandsApi as CommandsWriteApi)
    : null;

const COMMAND_TEXT: Record<string, { title: MessageKey; desc: MessageKey }> = {
  "toggle-intercept": {
    title: "options.shortcuts.toggleInterceptTitle",
    desc: "options.shortcuts.toggleInterceptDesc",
  },
  "download-playing-media": {
    title: "options.shortcuts.downloadMediaTitle",
    desc: "options.shortcuts.downloadMediaDesc",
  },
  _execute_action: { title: "options.shortcuts.openPopupTitle", desc: "options.shortcuts.openPopupDesc" },
  _execute_browser_action: { title: "options.shortcuts.openPopupTitle", desc: "options.shortcuts.openPopupDesc" },
  "open-side-panel": { title: "options.shortcuts.openSidePanelTitle", desc: "options.shortcuts.openSidePanelDesc" },
  _execute_sidebar_action: {
    title: "options.shortcuts.openSidePanelTitle",
    desc: "options.shortcuts.openSidePanelDesc",
  },
};
const COMMAND_ORDER = Object.keys(COMMAND_TEXT);

interface CommandRow {
  name: string;
  shortcut: string;
  description: string;
}

interface Snapshot {
  mac: boolean;
  commands: CommandRow[];
}

async function loadSnapshot(): Promise<Snapshot> {
  const [platform, all] = await Promise.all([browser.runtime.getPlatformInfo(), browser.commands.getAll()]);
  const commands = all
    .filter((c): c is typeof c & { name: string } => typeof c.name === "string" && c.name !== "")
    .map((c) => ({ name: c.name, shortcut: c.shortcut ?? "", description: c.description ?? "" }));
  const rank = (name: string) => {
    const i = COMMAND_ORDER.indexOf(name);
    return i === -1 ? COMMAND_ORDER.length : i;
  };
  commands.sort((a, b) => rank(a.name) - rank(b.name));
  return { mac: platform.os === "mac", commands };
}

export function ShortcutsPanel(): JSX.Element {
  const t = useT();
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [recording, setRecording] = useState<string | null>(null);
  const [held, setHeld] = useState<string[]>([]);
  const [error, setError] = useState<{ name: string; message: string } | null>(null);

  const refresh = async () => {
    try {
      setSnapshot(await loadSnapshot());
    } catch (e) {
      showToast(t("options.shortcuts.loadFailed", { error: e instanceof Error ? e.message : String(e) }), "error");
    }
  };

  useEffect(() => {
    void refresh();
    // 在浏览器快捷键页改完切回本页、或 Firefox 其它入口改动后自动刷新。
    const onVisible = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    const onChanged = () => void refresh();
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onChanged);
    WRITABLE?.onChanged?.addListener(onChanged);
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", onChanged);
      WRITABLE?.onChanged?.removeListener(onChanged);
    };
  }, []);

  const mac = snapshot?.mac ?? false;
  const commands = snapshot?.commands ?? [];
  const titleOf = (c: CommandRow) => {
    const text = COMMAND_TEXT[c.name];
    return text ? t(text.title) : c.description || c.name;
  };

  const openBrowserSettings = async () => {
    try {
      await browser.tabs.create({ url: shortcutSettingsUrl(navigator.userAgent) });
    } catch (e) {
      showToast(t("options.shortcuts.openFailed", { error: e instanceof Error ? e.message : String(e) }), "error");
    }
  };

  const startRecording = (name: string) => {
    setRecording(name);
    setHeld([]);
    setError(null);
  };

  const stopRecording = () => {
    setRecording(null);
    setHeld([]);
  };

  const apply = async (api: CommandsWriteApi, row: CommandRow, shortcut: string) => {
    const id = shortcutId(shortcut, mac);
    const other = id ? commands.find((c) => c.name !== row.name && shortcutId(c.shortcut, mac) === id) : undefined;
    if (other) {
      setError({ name: row.name, message: t("options.shortcuts.conflict", { name: titleOf(other) }) });
      return;
    }
    try {
      await api.update({ name: row.name, shortcut });
    } catch (e) {
      setError({
        name: row.name,
        message: t("options.shortcuts.saveFailed", { error: e instanceof Error ? e.message : String(e) }),
      });
      return;
    }
    stopRecording();
    setError(null);
    await refresh();
    showToast(t(shortcut ? "options.shortcuts.saved" : "options.shortcuts.cleared"), "success");
  };

  const reset = async (api: CommandsWriteApi, name: string) => {
    try {
      await api.reset(name);
    } catch (e) {
      showToast(t("options.shortcuts.saveFailed", { error: e instanceof Error ? e.message : String(e) }), "error");
      return;
    }
    stopRecording();
    setError(null);
    await refresh();
    showToast(t("options.shortcuts.resetDone"), "success");
  };

  const onKeyDown = (api: CommandsWriteApi, row: CommandRow, e: KeyboardEvent) => {
    // 无修饰键的 Tab 保留焦点切换（失焦即结束录制）。
    if (e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) return;
    e.preventDefault();
    e.stopPropagation();
    const capture = captureShortcut(e, mac);
    switch (capture.kind) {
      case "pending":
        setHeld(capture.modifiers);
        setError(null);
        return;
      case "cancel":
        stopRecording();
        setError(null);
        return;
      case "clear":
        void apply(api, row, "");
        return;
      case "invalid":
        setHeld([]);
        setError({
          name: row.name,
          message: t(capture.reason === "modifier" ? "options.shortcuts.needModifier" : "options.shortcuts.unsupportedKey"),
        });
        return;
      case "ok":
        void apply(api, row, capture.shortcut);
    }
  };

  const onKeyUp = (e: KeyboardEvent) => {
    const capture = captureShortcut(e, mac);
    if (capture.kind === "pending") setHeld(capture.modifiers);
  };

  const defaults = browser.runtime.getManifest().commands ?? {};

  return (
    <PanelShell
      title={t("options.nav.shortcuts")}
      desc={t(WRITABLE ? "options.shortcuts.descInline" : "options.shortcuts.descBrowser")}
    >
      {commands.map((row) => {
        const active = recording === row.name;
        const keys = shortcutKeys(active ? held.join("+") : row.shortcut, mac);
        const rowError = error?.name === row.name ? error.message : null;
        const text = COMMAND_TEXT[row.name];
        const customized =
          shortcutId(row.shortcut, mac) !== shortcutId(defaults[row.name]?.suggested_key?.default ?? "", mac);
        return (
          <SettingRow
            key={row.name}
            title={titleOf(row)}
            desc={text ? t(text.desc) : undefined}
            inline
          >
            <div class="opt-shortcut-cell">
              <div class="opt-shortcut-line">
                {WRITABLE && customized ? (
                  <IconButton
                    icon="refresh"
                    label={t("options.shortcuts.reset")}
                    onClick={() => void reset(WRITABLE, row.name)}
                  />
                ) : null}
                <button
                  type="button"
                  class={`opt-shortcut${active ? " recording" : ""}${rowError ? " invalid" : ""}`}
                  title={t(WRITABLE ? "options.shortcuts.clickToRecord" : "options.shortcuts.clickToOpen")}
                  aria-label={titleOf(row)}
                  onClick={() => (WRITABLE ? startRecording(row.name) : void openBrowserSettings())}
                  onKeyDown={WRITABLE && active ? (e) => onKeyDown(WRITABLE, row, e) : undefined}
                  onKeyUp={active ? onKeyUp : undefined}
                  onBlur={active ? stopRecording : undefined}
                >
                  {keys.length > 0 ? (
                    keys.map((k, i) => (
                      <kbd key={i} class="opt-kbd">
                        {k}
                      </kbd>
                    ))
                  ) : (
                    <span class="opt-shortcut-placeholder">
                      {t(active ? "options.shortcuts.recording" : "options.shortcuts.notSet")}
                    </span>
                  )}
                </button>
              </div>
              {rowError ? (
                <div class="opt-shortcut-msg opt-error-text">{rowError}</div>
              ) : active ? (
                <div class="opt-shortcut-msg">{t("options.shortcuts.recordHint")}</div>
              ) : null}
            </div>
          </SettingRow>
        );
      })}

      {snapshot && !WRITABLE ? (
        <SettingRow title={t("options.shortcuts.browserTitle")} desc={t("options.shortcuts.browserDesc")} inline>
          <Button onClick={() => void openBrowserSettings()}>{t("options.shortcuts.browserOpen")}</Button>
        </SettingRow>
      ) : null}
    </PanelShell>
  );
}
