/**
 * 浏览器侧能力探测：`userScripts` 可选权限是否已授予、API 是否可用（Chrome 开关）、是否支持 per-world worldId。
 */
import { browser } from "wxt/browser";
import type { PageScriptsAvailability } from "./model";

/** Chrome 系浏览器的主版本号（UA 里的 `Chrome/NNN`）；Firefox 等没有该标记返回 null。 */
export function chromeMajorVersion(userAgent: string): number | null {
  const match = /\bChrome\/(\d+)/.exec(userAgent);
  return match ? Number(match[1]) : null;
}

/** per-world worldId 的最低 Chrome 版本（契约：Chrome ≥133；Firefox ≥136 由 strict_min_version 140 保证）。 */
export const MIN_CHROME_FOR_WORLD_ID = 133;
/** Chrome 138 起 userScripts 用扩展详情页的「允许用户脚本」开关，之前是开发者模式。 */
export const CHROME_USER_SCRIPTS_TOGGLE_VERSION = 138;

/** 可注入的探测环境（测试用）。 */
export interface AvailabilityEnv {
  userAgent: string;
  hasPermission(): Promise<boolean>;
  /** `browser.userScripts`；Chrome 138+ 开关未开时为 undefined。 */
  userScripts(): UserScriptsProbe | undefined;
}

export interface UserScriptsProbe {
  getScripts(): Promise<unknown>;
  getWorldConfigurations?: unknown;
}

export async function probeAvailability(env: AvailabilityEnv): Promise<PageScriptsAvailability> {
  const chrome = chromeMajorVersion(env.userAgent);
  if (chrome !== null && chrome < MIN_CHROME_FOR_WORLD_ID) return "browser-unsupported";
  if (!(await env.hasPermission())) return "permission-missing";
  const api = env.userScripts();
  if (!api) return "switch-off";
  try {
    await api.getScripts();
  } catch {
    // Chrome <138 开发者模式未开、138+ 开关未开时调用会抛错。
    return "switch-off";
  }
  // Chrome 133+ / Firefox 136+ 才有 per-world API；缺失即无法按插件隔离。
  if (typeof api.getWorldConfigurations !== "function") return "browser-unsupported";
  return "ok";
}

export const browserAvailabilityEnv: AvailabilityEnv = {
  get userAgent() {
    return navigator.userAgent;
  },
  hasPermission: async () => {
    try {
      return await browser.permissions.contains({ permissions: ["userScripts"] });
    } catch {
      return false;
    }
  },
  // 类型上 `browser.userScripts` 恒存在，但 Chrome 138+ 开关未开时运行时为 undefined。
  userScripts: (): UserScriptsProbe | undefined => browser.userScripts,
};
