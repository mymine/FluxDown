/**
 * 同步计划（纯函数）：索引 × 已批准插件 × 浏览器里已注册的脚本 → 要拉取 / 注册 / 更新 / 注销的集合。
 */
import type { PageScriptIndexEntry, RegisteredRecord } from "./model";
import { fingerprintOf, pluginKey, pluginKeyOfRegistration, registrationId, SCRIPT_ID_PREFIX } from "./registration";

export interface PluginPlan {
  identity: string;
  /** 该插件在索引里的全部脚本（已批准）。 */
  wanted: PageScriptIndexEntry[];
  /** 需要拉源码并 register / update 的脚本（缺失，或 sha256 / 元数据指纹变化）。 */
  upsert: PageScriptIndexEntry[];
}

export interface SyncPlan {
  plugins: PluginPlan[];
  /** 已注册（`fd.` 命名）但不再被任何已批准插件的索引条目需要的注册 id → 注销。 */
  orphans: string[];
}

export function planSync(
  scripts: readonly PageScriptIndexEntry[],
  approved: ReadonlySet<string>,
  registeredIds: readonly string[],
  records: Readonly<Record<string, RegisteredRecord>>,
): SyncPlan {
  const byPlugin = new Map<string, PageScriptIndexEntry[]>();
  const wantedIds = new Set<string>();
  for (const entry of scripts) {
    if (!approved.has(entry.identity)) continue;
    const list = byPlugin.get(entry.identity) ?? [];
    list.push(entry);
    byPlugin.set(entry.identity, list);
    wantedIds.add(registrationId(entry.identity, entry.id));
  }
  const registered = new Set(registeredIds);

  const plugins: PluginPlan[] = [];
  for (const [identity, wanted] of byPlugin) {
    const upsert = wanted.filter((entry) => {
      const id = registrationId(identity, entry.id);
      return !registered.has(id) || records[id]?.fingerprint !== fingerprintOf(entry);
    });
    plugins.push({ identity, wanted, upsert });
  }

  const orphans = registeredIds.filter((id) => id.startsWith(SCRIPT_ID_PREFIX) && !wantedIds.has(id));
  return { plugins, orphans };
}

/** 某插件名下当前已注册的注册 id。 */
export function registrationsOfPlugin(identity: string, registeredIds: readonly string[]): string[] {
  const key = pluginKey(identity);
  return registeredIds.filter((id) => pluginKeyOfRegistration(id) === key);
}
