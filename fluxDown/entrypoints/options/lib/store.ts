/**
 * options 页的数据层：
 * - `settings` 信号：sync `settings` + local 远程配置的合并视图，storage.onChanged 时自动刷新；
 *   所有写入经 updateSettings → saveSettings（串行，先乐观更新再以落盘结果为准）。
 * - useLocalValue：绑定某个 storage.local 键的 hook。
 */
import { signal } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import { showToast } from "@/ui/components/Toast";
import { t } from "@/utils/i18n";
import { REMOTE_SETTINGS_KEY } from "@/utils/remote-settings";
import { loadSettings, saveSettings, type FluxDownSettings } from "@/utils/settings";
import { logError } from "@/utils/error-log";

export const settings = signal<FluxDownSettings | null>(null);

let writeQueue: Promise<void> = Promise.resolve();

export async function refreshSettings(): Promise<void> {
  settings.value = await loadSettings();
}

/** 写入设置：立即乐观更新界面，落盘失败时回读并提示。 */
export function updateSettings(patch: Partial<FluxDownSettings>): Promise<void> {
  if (settings.value) settings.value = { ...settings.value, ...patch };
  writeQueue = writeQueue
    .then(() => saveSettings(patch))
    .then(refreshSettings)
    .catch(async (e) => {
      logError("[FluxDown Options] saveSettings failed:", e);
      showToast(t("options.saveFailed"), "error");
      await refreshSettings().catch(() => {});
    });
  return writeQueue;
}

/** 启动设置同步：首次加载 + 监听 sync/local 变更；返回取消函数。 */
export function startSettingsSync(): () => void {
  refreshSettings().catch((e) => logError("[FluxDown Options] loadSettings failed:", e));
  const onChanged = (changes: Record<string, unknown>, area: string) => {
    if ((area === "sync" && "settings" in changes) || (area === "local" && REMOTE_SETTINGS_KEY in changes)) {
      refreshSettings().catch(() => {});
    }
  };
  browser.storage.onChanged.addListener(onChanged);
  return () => browser.storage.onChanged.removeListener(onChanged);
}

/**
 * 绑定 storage.local 的一个键：读取时经 parse 规整，外部写入（其它页面 / background）自动同步。
 * 返回 [当前值, 写入函数]；写入先更新本地状态再落盘，落盘失败则回滚并提示。
 */
export function useLocalValue<T>(key: string, parse: (raw: unknown) => T): [T, (next: T) => Promise<void>] {
  const [value, setValue] = useState<T>(() => parse(undefined));
  useEffect(() => {
    let alive = true;
    browser.storage.local
      .get(key)
      .then((r) => alive && setValue(parse(r[key])))
      .catch(() => {});
    const onChanged = (changes: Record<string, { newValue?: unknown }>, area: string) => {
      if (area === "local" && key in changes) setValue(parse(changes[key].newValue));
    };
    browser.storage.onChanged.addListener(onChanged);
    return () => {
      alive = false;
      browser.storage.onChanged.removeListener(onChanged);
    };
  }, [key]);
  const latest = useRef(value);
  latest.current = value;
  // 落盘失败时回滚到写入前的值并提示，避免界面显示一份从未持久化的状态。
  const write = async (next: T) => {
    const previous = latest.current;
    latest.current = next;
    setValue(next);
    try {
      await browser.storage.local.set({ [key]: next });
    } catch (e) {
      logError(`[FluxDown Options] storage.local.set(${key}) failed:`, e);
      latest.current = previous;
      setValue(previous);
      showToast(t("options.saveFailed"), "error");
    }
  };
  return [value, write];
}
