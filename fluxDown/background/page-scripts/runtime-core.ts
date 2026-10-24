/**
 * USER_SCRIPT world 里的 `fluxdown` 桥运行时（逻辑核心；入口见 entrypoints/page-script-runtime.ts）。
 *
 * - 读取并删除启动全局，令牌与 `runtime.sendMessage` 只存在于闭包，页面脚本拿不到；
 * - 暴露 `Object.freeze` 的 `fluxdown`，形状固定（契约 §4），不提供任何扩展自身能力的修改入口；
 * - 不含 eval / new Function。
 */
import { BOOT_GLOBAL, BRIDGE_VERSION } from "./wire";
import type { BootData, BridgeCall, BridgeMediaItem, BridgeRequest, BridgeResponse } from "./wire";

export type SendFn = (message: BridgeRequest) => Promise<unknown>;

export interface FluxdownInfo {
  readonly identity: string;
  readonly scriptId: string;
  readonly version: string;
}

export interface FluxdownStorage {
  get<T = unknown>(key: string): Promise<T | undefined>;
  set(key: string, value: unknown): Promise<void>;
  remove(key: string): Promise<void>;
}

export interface Fluxdown {
  readonly info: FluxdownInfo;
  download(request: {
    url: string;
    filename?: string;
    referrer?: string;
    headers?: Record<string, string>;
  }): Promise<{ ok: boolean; message?: string }>;
  addMedia(items: BridgeMediaItem[]): Promise<number>;
  invoke<T = unknown>(name: string, args?: unknown): Promise<T>;
  readonly storage: FluxdownStorage;
}

function isBootData(value: unknown): value is BootData {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.identity === "string" &&
    typeof v.scriptId === "string" &&
    typeof v.version === "string" &&
    typeof v.token === "string" &&
    v.token !== ""
  );
}

function isResponse(value: unknown): value is BridgeResponse {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return v.ok === true || (v.ok === false && typeof v.error === "string");
}

/** 用启动数据与发送函数构造冻结的 `fluxdown`。令牌只被下面的闭包引用。 */
export function createFluxdown(boot: BootData, send: SendFn): Readonly<Fluxdown> {
  const token = boot.token;

  async function call(message: BridgeCall): Promise<unknown> {
    let reply: unknown;
    try {
      reply = await send({ v: BRIDGE_VERSION, token, ...message });
    } catch (e) {
      throw new Error(`FluxDown: ${e instanceof Error ? e.message : String(e)}`);
    }
    if (!isResponse(reply)) throw new Error("FluxDown: no response from the extension");
    if (!reply.ok) throw new Error(reply.error);
    return reply.value;
  }

  const storage: FluxdownStorage = Object.freeze({
    async get<T = unknown>(key: string): Promise<T | undefined> {
      return (await call({ op: "storage.get", key: String(key) })) as T | undefined;
    },
    async set(key: string, value: unknown): Promise<void> {
      await call({ op: "storage.set", key: String(key), value });
    },
    async remove(key: string): Promise<void> {
      await call({ op: "storage.remove", key: String(key) });
    },
  });

  const info: FluxdownInfo = Object.freeze({
    identity: boot.identity,
    scriptId: boot.scriptId,
    version: boot.version,
  });

  const api: Fluxdown = {
    info,
    async download(request) {
      if (typeof request !== "object" || request === null) throw new TypeError("fluxdown.download: request must be an object");
      const reply = await call({ op: "download", request });
      const ok = typeof reply === "object" && reply !== null && "ok" in reply && reply.ok === true;
      const message =
        typeof reply === "object" && reply !== null && "message" in reply && typeof reply.message === "string"
          ? reply.message
          : undefined;
      return { ok, ...(message !== undefined ? { message } : {}) };
    },
    async addMedia(items) {
      if (!Array.isArray(items)) throw new TypeError("fluxdown.addMedia: items must be an array");
      const accepted = await call({ op: "addMedia", items });
      return typeof accepted === "number" ? accepted : 0;
    },
    async invoke<T = unknown>(name: string, args?: unknown): Promise<T> {
      return (await call({ op: "invoke", name: String(name), args })) as T;
    },
    storage,
  };
  return Object.freeze(api);
}

/** 运行时的 runtime 对象形状（Chrome `chrome.runtime` / Firefox `browser.runtime`）。 */
interface RuntimeLike {
  sendMessage?: (message: unknown) => Promise<unknown>;
}

/**
 * 在给定全局对象上安装 `fluxdown`：读取并删除启动全局，把 `sendMessage` 绑进闭包。
 * 启动数据缺失 / 非法或没有 `runtime.sendMessage`（world 未开 messaging）时不安装并返回 false。
 * `fluxdown` 可重定义（同一插件的多个脚本共用一个 world，后者覆盖前者），但值本身已冻结。
 */
export function installRuntime(target: Record<string, unknown>): boolean {
  const boot = target[BOOT_GLOBAL];
  delete target[BOOT_GLOBAL];
  if (!isBootData(boot)) return false;

  const api = (target.browser ?? target.chrome) as { runtime?: RuntimeLike } | undefined;
  const runtime = api?.runtime;
  const rawSend = runtime?.sendMessage;
  if (!runtime || typeof rawSend !== "function") return false;
  const send: SendFn = (message) => rawSend.call(runtime, message);

  Object.defineProperty(target, "fluxdown", {
    value: createFluxdown(boot, send),
    enumerable: true,
    writable: false,
    configurable: true,
  });
  return true;
}
