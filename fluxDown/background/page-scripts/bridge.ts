/**
 * 页面脚本桥（background 侧）：`runtime.onUserScriptMessage` 的消息校验与分发。
 *
 * 信任模型：
 * - `MessageSender` 没有 world 字段，来源插件靠每插件随机令牌认出；未批准 / 令牌错一律拒绝；
 * - `pageUrl` / 默认 referrer 取 `sender.url`，不信任脚本自报；
 * - 桥 API 窄且固定：download / addMedia / invoke / storage，不提供修改扩展自身能力的入口；
 * - payload 逐字段校验，单条消息 ≤ 256 KiB，按插件限流。
 *
 * 具体能力（下载、嗅探列表、桌面调用、存储）经 {@link BridgeDeps} 注入，测试用假实现。
 */
import {
  BRIDGE_VERSION,
  DOWNLOAD_HEADER_ALLOWLIST,
  MAX_MEDIA_ITEMS,
  MAX_MESSAGE_BYTES,
} from "./wire";
import type { BridgeMediaItem, BridgeOp, BridgeResponse } from "./wire";

/** 发送方（`runtime.MessageSender` 的子集）。 */
export interface BridgeSender {
  id?: string;
  url?: string;
  frameId?: number;
  tab?: { id?: number; url?: string };
}

export interface DownloadCall {
  identity: string;
  url: string;
  filename?: string;
  referrer: string;
  headers: Record<string, string>;
  tabId?: number;
}

export interface AddMediaCall {
  identity: string;
  tabId: number;
  frameId: number;
  /** 发送帧自己的 URL（来自 sender.url）。 */
  pageUrl: string;
  tabUrl?: string;
  items: BridgeMediaItem[];
}

export type InvokeOutcome = { ok: true; value: unknown } | { ok: false; message: string };

export interface BridgeStorage {
  get(identity: string, key: string): Promise<unknown>;
  set(identity: string, key: string, value: unknown): Promise<void>;
  remove(identity: string, key: string): Promise<void>;
}

export interface BridgeDeps {
  identityForToken(token: string): Promise<string | null>;
  now(): number;
  extensionId: string;
  download(call: DownloadCall): Promise<{ ok: boolean; message?: string }>;
  addMedia(call: AddMediaCall): Promise<number>;
  invoke(identity: string, name: string, args: unknown, pageUrl: string): Promise<InvokeOutcome>;
  storage: BridgeStorage;
}

export type BridgeHandler = (message: unknown, sender: BridgeSender) => Promise<BridgeResponse>;

// ===== 限制 =====

const MAX_URL_LENGTH = 8192;
const MAX_FILENAME_LENGTH = 255;
const MAX_TITLE_LENGTH = 256;
const MAX_MIME_LENGTH = 127;
const MAX_HEADER_VALUE_LENGTH = 2048;
const MAX_INVOKE_NAME_LENGTH = 128;
const MAX_STORAGE_KEY_LENGTH = 256;
const MAX_TOKEN_LENGTH = 128;
/** 每插件消息限流：窗口内总条数 / download 条数。 */
export const RATE_WINDOW_MS = 10_000;
export const RATE_LIMIT_ALL = 60;
export const RATE_LIMIT_DOWNLOAD = 10;
/** 每插件同时进行的 invoke 上限（每个在途调用占用一条专用 NMH 端口的队列位）。 */
export const MAX_INVOKES_IN_FLIGHT = 4;

const OPS: readonly BridgeOp[] = [
  "download",
  "addMedia",
  "invoke",
  "storage.get",
  "storage.set",
  "storage.remove",
];
// eslint-disable-next-line no-control-regex
const CONTROL_CHARS = /[\u0000-\u001f\u007f]/;
const MIME_RE = /^[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*\/[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*$/;

type Checked<T> = { ok: true; value: T } | { ok: false; error: string };

const reject = (error: string): { ok: false; error: string } => ({ ok: false, error });

/** 只接受 http / https 的绝对 URL；返回规范化后的字符串。 */
export function checkHttpUrl(value: unknown, what: string): Checked<string> {
  if (typeof value !== "string" || value === "" || value.length > MAX_URL_LENGTH) return reject(`${what} must be an http(s) URL`);
  try {
    const url = new URL(value);
    if (url.protocol !== "http:" && url.protocol !== "https:") return reject(`${what} must be an http(s) URL`);
    return { ok: true, value };
  } catch {
    return reject(`${what} must be an http(s) URL`);
  }
}

export interface ValidatedDownload {
  url: string;
  filename?: string;
  referrer: string;
  headers: Record<string, string>;
}

/** 校验 `download` 请求；referrer 缺省取 `defaultReferrer`（发送页 URL）。 */
export function validateDownload(request: unknown, defaultReferrer: string): Checked<ValidatedDownload> {
  if (typeof request !== "object" || request === null || Array.isArray(request)) return reject("download request must be an object");
  const r = request as Record<string, unknown>;
  const url = checkHttpUrl(r.url, "url");
  if (!url.ok) return url;

  let filename: string | undefined;
  if (r.filename !== undefined) {
    if (typeof r.filename !== "string" || r.filename.length > MAX_FILENAME_LENGTH || CONTROL_CHARS.test(r.filename)) {
      return reject("invalid filename");
    }
    filename = r.filename.trim() || undefined;
  }

  let referrer = defaultReferrer;
  if (r.referrer !== undefined) {
    const checked = checkHttpUrl(r.referrer, "referrer");
    if (!checked.ok) return checked;
    referrer = checked.value;
  }

  const headers: Record<string, string> = {};
  if (r.headers !== undefined) {
    if (typeof r.headers !== "object" || r.headers === null || Array.isArray(r.headers)) return reject("headers must be an object");
    const entries = Object.entries(r.headers);
    if (entries.length > DOWNLOAD_HEADER_ALLOWLIST.length) return reject("too many headers");
    for (const [name, value] of entries) {
      const lower = name.toLowerCase();
      if (!DOWNLOAD_HEADER_ALLOWLIST.includes(lower)) return reject(`header not allowed: ${name}`);
      if (typeof value !== "string" || value.length > MAX_HEADER_VALUE_LENGTH || CONTROL_CHARS.test(value)) {
        return reject(`invalid header value: ${name}`);
      }
      headers[lower] = value;
    }
  }
  return { ok: true, value: { url: url.value, ...(filename ? { filename } : {}), referrer, headers } };
}

/**
 * 校验 `addMedia` 条目：整体必须是数组且 ≤ 100 条；单条非法（非 http(s) URL / 字段类型错）被跳过，
 * 返回的是通过校验的条目。
 */
export function validateMediaItems(items: unknown): Checked<BridgeMediaItem[]> {
  if (!Array.isArray(items)) return reject("items must be an array");
  if (items.length > MAX_MEDIA_ITEMS) return reject(`at most ${MAX_MEDIA_ITEMS} items per call`);
  const out: BridgeMediaItem[] = [];
  for (const raw of items) {
    if (typeof raw !== "object" || raw === null) continue;
    const item = raw as Record<string, unknown>;
    const url = checkHttpUrl(item.url, "url");
    if (!url.ok) continue;
    const title = typeof item.title === "string" ? item.title.slice(0, MAX_TITLE_LENGTH) : undefined;
    const mime =
      typeof item.mime === "string" && item.mime.length <= MAX_MIME_LENGTH && MIME_RE.test(item.mime.split(";")[0].trim())
        ? item.mime
        : undefined;
    out.push({ url: url.value, ...(title ? { title } : {}), ...(mime ? { mime } : {}) });
  }
  return { ok: true, value: out };
}

function checkName(value: unknown): Checked<string> {
  if (typeof value !== "string" || value === "" || value.length > MAX_INVOKE_NAME_LENGTH || CONTROL_CHARS.test(value)) {
    return reject("invalid handler name");
  }
  return { ok: true, value };
}

function checkStorageKey(value: unknown): Checked<string> {
  if (
    typeof value !== "string" ||
    value === "" ||
    value.length > MAX_STORAGE_KEY_LENGTH ||
    value === "__proto__"
  ) {
    return reject("invalid storage key");
  }
  return { ok: true, value };
}

/** 滑动窗口限流：按 key 记录窗口内的时间戳。 */
class RateLimiter {
  private readonly hits = new Map<string, number[]>();

  constructor(
    private readonly limit: number,
    private readonly windowMs: number,
  ) {}

  /** 记一次；超限返回 false（不计入）。 */
  take(key: string, now: number): boolean {
    const recent = (this.hits.get(key) ?? []).filter((at) => now - at < this.windowMs);
    if (recent.length >= this.limit) {
      this.hits.set(key, recent);
      return false;
    }
    recent.push(now);
    this.hits.set(key, recent);
    return true;
  }
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function createBridge(deps: BridgeDeps): BridgeHandler {
  const allLimiter = new RateLimiter(RATE_LIMIT_ALL, RATE_WINDOW_MS);
  const downloadLimiter = new RateLimiter(RATE_LIMIT_DOWNLOAD, RATE_WINDOW_MS);
  const invokesInFlight = new Map<string, number>();
  const encoder = new TextEncoder();

  return async (message, sender) => {
    if (typeof message !== "object" || message === null || Array.isArray(message)) {
      return { ok: false, error: "invalid message" };
    }
    let size: number;
    try {
      size = encoder.encode(JSON.stringify(message)).length;
    } catch {
      return { ok: false, error: "invalid message" };
    }
    if (size > MAX_MESSAGE_BYTES) return { ok: false, error: "message too large" };

    const m = message as Record<string, unknown>;
    if (m.v !== BRIDGE_VERSION || typeof m.token !== "string" || m.token === "" || m.token.length > MAX_TOKEN_LENGTH) {
      return { ok: false, error: "invalid message" };
    }
    if (typeof m.op !== "string" || !OPS.includes(m.op as BridgeOp)) return { ok: false, error: "unknown operation" };

    const identity = await deps.identityForToken(m.token);
    if (identity === null) return { ok: false, error: "page scripts are not enabled for this plugin" };

    if (sender.id !== undefined && sender.id !== deps.extensionId) return { ok: false, error: "invalid sender" };
    const pageUrl = checkHttpUrl(sender.url, "sender");
    if (!pageUrl.ok) return { ok: false, error: "invalid sender" };

    const now = deps.now();
    if (!allLimiter.take(identity, now)) return { ok: false, error: "rate limited" };

    switch (m.op as BridgeOp) {
      case "download": {
        const checked = validateDownload(m.request, pageUrl.value);
        if (!checked.ok) return { ok: false, error: checked.error };
        if (!downloadLimiter.take(identity, now)) return { ok: false, error: "rate limited" };
        try {
          const result = await deps.download({ identity, ...checked.value, tabId: sender.tab?.id });
          return { ok: true, value: result };
        } catch (e) {
          return { ok: false, error: errorText(e) };
        }
      }
      case "addMedia": {
        const checked = validateMediaItems(m.items);
        if (!checked.ok) return { ok: false, error: checked.error };
        const tabId = sender.tab?.id;
        if (tabId === undefined || tabId < 0) return { ok: false, error: "addMedia requires a tab" };
        if (checked.value.length === 0) return { ok: true, value: 0 };
        try {
          const accepted = await deps.addMedia({
            identity,
            tabId,
            frameId: sender.frameId ?? 0,
            pageUrl: pageUrl.value,
            tabUrl: sender.tab?.url,
            items: checked.value,
          });
          return { ok: true, value: accepted };
        } catch (e) {
          return { ok: false, error: errorText(e) };
        }
      }
      case "invoke": {
        const name = checkName(m.name);
        if (!name.ok) return { ok: false, error: name.error };
        const active = invokesInFlight.get(identity) ?? 0;
        if (active >= MAX_INVOKES_IN_FLIGHT) return { ok: false, error: "too many concurrent invocations" };
        invokesInFlight.set(identity, active + 1);
        try {
          const result = await deps.invoke(identity, name.value, m.args, pageUrl.value);
          return result.ok ? { ok: true, value: result.value } : { ok: false, error: result.message };
        } catch (e) {
          return { ok: false, error: errorText(e) };
        } finally {
          const left = (invokesInFlight.get(identity) ?? 1) - 1;
          if (left <= 0) invokesInFlight.delete(identity);
          else invokesInFlight.set(identity, left);
        }
      }
      case "storage.get": {
        const key = checkStorageKey(m.key);
        if (!key.ok) return { ok: false, error: key.error };
        try {
          return { ok: true, value: await deps.storage.get(identity, key.value) };
        } catch (e) {
          return { ok: false, error: errorText(e) };
        }
      }
      case "storage.set": {
        const key = checkStorageKey(m.key);
        if (!key.ok) return { ok: false, error: key.error };
        if (!("value" in m) || m.value === undefined) return { ok: false, error: "value is required" };
        try {
          await deps.storage.set(identity, key.value, m.value);
          return { ok: true };
        } catch (e) {
          return { ok: false, error: errorText(e) };
        }
      }
      case "storage.remove": {
        const key = checkStorageKey(m.key);
        if (!key.ok) return { ok: false, error: key.error };
        try {
          await deps.storage.remove(identity, key.value);
          return { ok: true };
        } catch (e) {
          return { ok: false, error: errorText(e) };
        }
      }
    }
  };
}
