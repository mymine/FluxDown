/**
 * 注册模型（契约 §5）：worldId / 脚本 id 命名、world 配置、userScripts 注册对象构造。纯函数。
 */
import type { Browser } from "wxt/browser";
import type { PageScriptIndexEntry } from "./model";
import { BOOT_GLOBAL, RUNTIME_FILE, WORLD_CSP } from "./wire";
import type { BootData } from "./wire";

export const SCRIPT_ID_PREFIX = "fd.";

/** `fd.` + identity（`@` 换成 `.`）去掉前缀的部分。 */
export function pluginKey(identity: string): string {
  return identity.replaceAll("@", ".");
}

export function worldIdOf(identity: string): string {
  return `${SCRIPT_ID_PREFIX}${pluginKey(identity)}`;
}

export function registrationId(identity: string, scriptId: string): string {
  return `${worldIdOf(identity)}.${scriptId}`;
}

/** 注册 id 所属的插件 key（脚本 id 不含 `.`，取最后一个 `.` 之前）；非本扩展命名返回 null。 */
export function pluginKeyOfRegistration(regId: string): string | null {
  if (!regId.startsWith(SCRIPT_ID_PREFIX)) return null;
  const dot = regId.lastIndexOf(".");
  return dot > SCRIPT_ID_PREFIX.length - 1 ? regId.slice(SCRIPT_ID_PREFIX.length, dot) : null;
}

export function worldProperties(worldId: string): Browser.userScripts.WorldProperties {
  return { worldId, messaging: true, csp: WORLD_CSP };
}

/** 启动数据：`globalThis[...] = {…}`。JSON 字面量即合法 JS 表达式，不拼接任何未转义内容。 */
export function bootstrapCode(boot: BootData): string {
  return `globalThis[${JSON.stringify(BOOT_GLOBAL)}]=${JSON.stringify(boot)};`;
}

/**
 * 构造 `userScripts.register` / `update` 的注册对象。
 * isolated：USER_SCRIPT world（按插件 worldId）+ 三段 js（启动数据、运行时文件、插件源码）；
 * main：页面 world，单段插件源码，无 worldId、无桥（页面可伪造消息，不给令牌）。
 */
export function buildRegistration(
  entry: PageScriptIndexEntry,
  token: string,
  code: string,
): Browser.userScripts.RegisteredUserScript {
  const base = {
    id: registrationId(entry.identity, entry.id),
    matches: [...entry.matches],
    ...(entry.excludeMatches.length > 0 ? { excludeMatches: [...entry.excludeMatches] } : {}),
    allFrames: entry.allFrames,
    runAt: entry.runAt,
  };
  if (entry.world === "main") {
    return { ...base, world: "MAIN", js: [{ code }] };
  }
  const boot: BootData = { identity: entry.identity, scriptId: entry.id, version: entry.pluginVersion, token };
  return {
    ...base,
    world: "USER_SCRIPT",
    worldId: worldIdOf(entry.identity),
    js: [{ code: bootstrapCode(boot) }, { file: RUNTIME_FILE }, { code }],
  };
}

/** 元数据指纹：任何影响注册对象的字段变化都要重新注册（sha256 只覆盖源码）。 */
export function fingerprintOf(entry: PageScriptIndexEntry): string {
  return JSON.stringify([
    entry.sha256,
    entry.pluginVersion,
    entry.matches,
    entry.excludeMatches,
    entry.runAt,
    entry.world,
    entry.allFrames,
  ]);
}
