/** 主题：local `theme`（"light" | "dark"，缺省跟随系统）→ root 元素的 data-theme。 */
import { useEffect } from "preact/hooks";
import { browser } from "wxt/browser";

const THEME_KEY = "theme";

function apply(root: HTMLElement, value: unknown): void {
  if (value === "light" || value === "dark") root.setAttribute("data-theme", value);
  else root.removeAttribute("data-theme");
}

export function useThemeAttr(root: HTMLElement): void {
  useEffect(() => {
    let alive = true;
    browser.storage.local
      .get(THEME_KEY)
      .then((r) => alive && apply(root, r[THEME_KEY]))
      .catch(() => {});
    const onChanged = (changes: Record<string, { newValue?: unknown }>, area: string) => {
      if (area === "local" && THEME_KEY in changes) apply(root, changes[THEME_KEY].newValue);
    };
    browser.storage.onChanged.addListener(onChanged);
    return () => {
      alive = false;
      browser.storage.onChanged.removeListener(onChanged);
    };
  }, [root]);
}

/** 写入主题；"system" 清除手动选择。 */
export async function saveTheme(theme: "light" | "dark" | "system"): Promise<void> {
  if (theme === "system") await browser.storage.local.remove(THEME_KEY);
  else await browser.storage.local.set({ [THEME_KEY]: theme });
}
