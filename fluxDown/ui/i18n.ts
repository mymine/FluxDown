/**
 * Preact 侧的 i18n 绑定：`useT()` 读取 `localeVersion` 信号，语言切换时组件自动重渲染。
 * 其它页面（或本页）调用 saveLocale 写入 storage 后，storage.onChanged 会同步 setLocale 并递增版本。
 */
import { signal } from "@preact/signals";
import { browser } from "wxt/browser";
import { getLocale, setLocale, t } from "@/utils/i18n";

export const localeVersion = signal(0);

const STORAGE_KEY = "fluxdown_locale";
let listening = false;

function ensureListener(): void {
  if (listening) return;
  listening = true;
  try {
    browser.storage.onChanged.addListener((changes, area) => {
      if (area !== "local" || !(STORAGE_KEY in changes)) return;
      const next = changes[STORAGE_KEY].newValue;
      if (typeof next === "string") setLocale(next);
      localeVersion.value++;
    });
  } catch {
    // 非扩展环境忽略
  }
}

/** 初始化后调用一次（或在首次 useT 时自动挂接）：当前语言变化需手动通知时用 bumpLocale。 */
export function bumpLocale(): void {
  localeVersion.value++;
}

export function useT(): typeof t {
  ensureListener();
  // 读取信号以建立订阅；返回值本身不变，t() 内部读取当前语言
  void localeVersion.value;
  return t;
}

export { getLocale };
