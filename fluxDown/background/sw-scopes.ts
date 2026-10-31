/**
 * 页面 Service Worker 作用域登记（识别 SW 合成的下载，判定见 utils/download-source.ts）。
 *
 * sensor 转发 MAIN world 钩子上报的作用域；MV3 Service Worker 随时被回收，登记表落
 * storage.session，只在新增作用域时写入。
 */
import { browser } from "wxt/browser";
import { ServiceWorkerScopes } from "@/utils/download-source";
import { logWarn } from "@/utils/error-log";

const STORAGE_KEY = "fluxdownServiceWorkerScopes";

let registry: ServiceWorkerScopes | null = null;
let restoring: Promise<ServiceWorkerScopes> | null = null;
/** 恢复完成前收到的上报，恢复后合并。 */
const early: string[] = [];

function persist(scopes: ServiceWorkerScopes): void {
  browser.storage.session
    ?.set({ [STORAGE_KEY]: scopes.toJSON() })
    .catch((e: unknown) => logWarn("[FluxDown] failed to persist service worker scopes:", e));
}

/** 同步可用的登记表；SW 冷启动、恢复完成前为 null（调用方需走异步路径）。 */
export function peekServiceWorkerScopes(): ServiceWorkerScopes | null {
  return registry;
}

export function loadServiceWorkerScopes(): Promise<ServiceWorkerScopes> {
  if (registry) return Promise.resolve(registry);
  restoring ??= (async () => {
    let stored: unknown;
    try {
      stored = (await browser.storage.session?.get(STORAGE_KEY))?.[STORAGE_KEY];
    } catch (e) {
      logWarn("[FluxDown] failed to restore service worker scopes:", e);
    }
    const scopes = ServiceWorkerScopes.from(stored);
    let added = false;
    for (const scope of early.splice(0)) added = scopes.add(scope) || added;
    if (added) persist(scopes);
    registry = scopes;
    return scopes;
  })();
  return restoring;
}

/** 登记一个已校验同源的作用域。 */
export function rememberServiceWorkerScope(scope: string): void {
  if (!registry) {
    early.push(scope);
    void loadServiceWorkerScopes();
    return;
  }
  if (registry.add(scope)) persist(registry);
}
