/**
 * 全站访问权限（`<all_urls>`）的检测与申请。
 *
 * Firefox MV3 把清单里的 host_permissions 当作用户可随时撤销的权限；撤销后内容脚本、
 * webRequest 与 scripting 注入都会失效，嗅探与下载拦截静默退化。Chrome 构建不展示该提示。
 */
import { browser } from "wxt/browser";
import type { Browser } from "wxt/browser";
import { logWarn } from "./error-log";

export const ALL_URLS_PERMISSION: Browser.permissions.Permissions = { origins: ["<all_urls>"] };

/** 检测 / 申请所需的 permissions API 子集（便于注入替身）。 */
export interface HostAccessApi {
  contains(permissions: Browser.permissions.Permissions): Promise<boolean>;
  request(permissions: Browser.permissions.Permissions): Promise<boolean>;
}

/** 当前构建是否需要检查全站访问权限。 */
export const HOST_ACCESS_APPLICABLE: boolean = import.meta.env.FIREFOX === true;

/**
 * 是否持有全站访问权限。不适用的构建、或 API 调用失败时视为已授权：
 * 误报「权限被撤销」会让用户反复面对无法解决的提示。
 */
export async function hasHostAccess(
  api: HostAccessApi = browser.permissions,
  applicable: boolean = HOST_ACCESS_APPLICABLE,
): Promise<boolean> {
  if (!applicable) return true;
  try {
    return await api.contains(ALL_URLS_PERMISSION);
  } catch (e) {
    logWarn("[FluxDown] host access check failed:", e);
    return true;
  }
}

/**
 * 申请全站访问权限。必须在用户手势的同步调用栈内调用（点击回调里直接调用，不要先 await）。
 * 返回用户是否授予；申请被浏览器拒绝时返回 false。
 */
export function requestHostAccess(api: HostAccessApi = browser.permissions): Promise<boolean> {
  return api.request(ALL_URLS_PERMISSION).catch((e) => {
    logWarn("[FluxDown] host access request failed:", e);
    return false;
  });
}
