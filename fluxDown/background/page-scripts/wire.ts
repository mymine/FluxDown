/**
 * 页面脚本桥的线路约定：background（bridge.ts）与随扩展打包的运行时（page-script-runtime）共用。
 * 只放常量与类型，不依赖 `browser.*`。
 */

/** 启动数据全局：registration 的第一段 `code` 写入，运行时读取后立刻删除。 */
export const BOOT_GLOBAL = "__fluxdown_boot__";
/** 随扩展打包的运行时文件（WXT unlisted script `page-script-runtime`）。 */
export const RUNTIME_FILE = "page-script-runtime.js";
/** 桥消息版本。 */
export const BRIDGE_VERSION = 1;

/** USER_SCRIPT world 的 CSP：禁止脚本在该 world 内 eval / 加载二次代码。 */
export const WORLD_CSP = "script-src 'self'; object-src 'none'";

/** 单条桥消息序列化后的上限（字节）。 */
export const MAX_MESSAGE_BYTES = 256 * 1024;
/** 单个插件桥存储总量上限（字节）。 */
export const MAX_STORAGE_BYTES = 1024 * 1024;
/** addMedia 单次条数上限。 */
export const MAX_MEDIA_ITEMS = 100;

/** download 允许携带的请求头（小写）。 */
export const DOWNLOAD_HEADER_ALLOWLIST: readonly string[] = [
  "referer",
  "origin",
  "user-agent",
  "accept",
  "accept-language",
  "x-requested-with",
];

/** 运行时启动数据（background 生成，经 `code` 注入）。 */
export interface BootData {
  identity: string;
  scriptId: string;
  version: string;
  token: string;
}

export interface BridgeDownloadRequest {
  url: string;
  filename?: string;
  referrer?: string;
  headers?: Record<string, string>;
}

export interface BridgeMediaItem {
  url: string;
  title?: string;
  mime?: string;
}

export type BridgeRequest =
  | { v: typeof BRIDGE_VERSION; token: string; op: "download"; request: BridgeDownloadRequest }
  | { v: typeof BRIDGE_VERSION; token: string; op: "addMedia"; items: BridgeMediaItem[] }
  | { v: typeof BRIDGE_VERSION; token: string; op: "invoke"; name: string; args?: unknown }
  | { v: typeof BRIDGE_VERSION; token: string; op: "storage.get"; key: string }
  | { v: typeof BRIDGE_VERSION; token: string; op: "storage.set"; key: string; value: unknown }
  | { v: typeof BRIDGE_VERSION; token: string; op: "storage.remove"; key: string };

export type BridgeOp = BridgeRequest["op"];

/** 去掉信封字段（版本 / 令牌）后的调用体；对联合逐成员 Omit，保留各 op 的字段。 */
export type BridgeCall = BridgeRequest extends infer R ? (R extends BridgeRequest ? Omit<R, "v" | "token"> : never) : never;

export type BridgeResponse = { ok: true; value?: unknown } | { ok: false; error: string };
