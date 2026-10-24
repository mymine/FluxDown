/**
 * 页面脚本的数据模型：桌面端索引 DTO（契约 §2）、扩展本地持久状态、给 UI 的状态视图，以及索引校验。
 * 纯数据，不依赖 `browser.*`。
 */

export type PageScriptRunAt = "document_start" | "document_end" | "document_idle";
export type PageScriptWorld = "isolated" | "main";

/** `daemon.plugin.pageScripts` 的一条（元数据 + 源码摘要，不含源码）。 */
export interface PageScriptIndexEntry {
  identity: string;
  pluginName: string;
  pluginVersion: string;
  id: string;
  title: string;
  matches: string[];
  excludeMatches: string[];
  runAt: PageScriptRunAt;
  world: PageScriptWorld;
  allFrames: boolean;
  /** 拼接后源码 UTF-8 字节的 SHA-256 小写十六进制。 */
  sha256: string;
  bytes: number;
}

export interface PageScriptsIndex {
  sequence: number;
  epoch: string;
  scripts: PageScriptIndexEntry[];
}

/** `daemon.plugin.pageScriptSource` 应答。 */
export interface PageScriptSource {
  identity: string;
  id: string;
  sha256: string;
  code: string;
}

/** 每插件首次启用的批准记录（storage.local）。 */
export interface PluginApproval {
  approved: true;
  approvedAt: number;
  /** 桥令牌：每插件随机，浏览器重启后已注册脚本仍带着它，所以必须持久。 */
  token: string;
  /** 启用时的插件名（桌面端不再提供该插件时仍可在 UI 里认出来）。 */
  pluginName: string;
}

export type PluginApprovals = Record<string, PluginApproval>;

/** 已注册脚本的记录：只存摘要，不存源码（用户决策 2）。 */
export interface RegisteredRecord {
  sha256: string;
  /** 元数据指纹（sha256 + 版本 + 匹配范围 + runAt + world + allFrames）：任何一项变化都要重新注册。 */
  fingerprint: string;
  world: PageScriptWorld;
}

export type PageScriptErrorCode =
  /** 拉取源码失败（桌面端不可达 / 应答失败）。 */
  | "fetch_failed"
  /** 源码 sha256 与索引不符。 */
  | "sha_mismatch"
  /** 脚本超出桥可传输的体积。 */
  | "source_too_large"
  /** 浏览器拒绝注册 / 更新。 */
  | "register_failed";

export interface PluginError {
  code: PageScriptErrorCode;
  scriptId?: string;
  detail?: string;
}

/** 同步结论（稳定代码，UI 本地化）。 */
export type SyncOutcome =
  | "ok"
  /** 桌面端不可达：保留现有注册。 */
  | "offline"
  /** 远程 always 模式：不走 NMH，功能不可用。 */
  | "remote-mode"
  /** 桌面端过旧，不认识页面脚本动作。 */
  | "host-unsupported"
  /** 浏览器侧不可用（权限 / 开关 / 版本）。 */
  | "browser-unavailable";

/** 浏览器侧可用性。 */
export type PageScriptsAvailability =
  | "ok"
  /** `userScripts` 可选权限尚未授予。 */
  | "permission-missing"
  /** 权限已授予但浏览器开关（Chrome 138+「允许用户脚本」/ <138 开发者模式）未开。 */
  | "switch-off"
  /** 浏览器太旧（Chrome <133 没有 per-world worldId）。 */
  | "browser-unsupported";

/** 同步状态（storage.local，独立于批准记录）。 */
export interface PageScriptsSyncState {
  applied?: { sequence: number; epoch: string };
  /** 最近一次拿到的索引元数据缓存（离线时 UI 仍可列出）。 */
  index: PageScriptIndexEntry[];
  registered: Record<string, RegisteredRecord>;
  errors: Record<string, PluginError>;
  lastAttemptAt?: number;
  lastSyncAt?: number;
  outcome?: SyncOutcome;
}

export function emptySyncState(): PageScriptsSyncState {
  return { index: [], registered: {}, errors: {} };
}

// ===== UI 视图 =====

export type PluginViewState =
  | "disabled"
  | "enabled"
  | "error"
  | "desktop-offline"
  | "permission-missing"
  | "switch-off"
  | "browser-unsupported";

export interface PageScriptView {
  id: string;
  title: string;
  matches: string[];
  excludeMatches: string[];
  world: PageScriptWorld;
  registered: boolean;
}

export interface PageScriptPluginView {
  identity: string;
  name: string;
  version: string;
  approved: boolean;
  /** 桌面端当前仍提供该插件的页面脚本。 */
  provided: boolean;
  state: PluginViewState;
  error?: PluginError;
  scripts: PageScriptView[];
}

export interface PageScriptsStatus {
  availability: PageScriptsAvailability;
  /** 全局性的不可用原因（远程 always / 桌面端过旧）；null = 无。 */
  blocked: "remote-mode" | "host-unsupported" | null;
  outcome: SyncOutcome | null;
  lastSyncAt: number | null;
  plugins: PageScriptPluginView[];
  /** 提示行：有待启用的插件（pending）或已启用但浏览器开关 / 权限未就绪（switch）。 */
  attention: "pending" | "switch" | null;
  pendingCount: number;
}

export interface PageScriptsActionResult {
  ok: boolean;
  message?: string;
  status: PageScriptsStatus;
}

export type PageScriptSourceResult =
  | { ok: true; code: string; sha256: string }
  | { ok: false; message: string };

// ===== 校验 =====

const SHA256_RE = /^[0-9a-f]{64}$/;
const SCRIPT_ID_RE = /^[a-z0-9][a-z0-9_-]{0,31}$/;
const RUN_AT: readonly string[] = ["document_start", "document_end", "document_idle"];
const MAX_PATTERNS = 32;
const MAX_ENTRIES = 512;

function isStringArray(value: unknown, max: number): value is string[] {
  return Array.isArray(value) && value.length <= max && value.every((item) => typeof item === "string");
}

/**
 * 扩展侧对 match pattern 再做一次基本防御（桌面端已校验）：只接受 http / https / `*` 协议，拒绝 `<all_urls>`。
 */
export function isAllowedMatchPattern(pattern: string): boolean {
  return /^(?:https?|\*):\/\/[^/]+\/.*$/.test(pattern) && !pattern.startsWith("*://*/");
}

function parseEntry(raw: unknown): PageScriptIndexEntry | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.identity !== "string" || r.identity === "" || r.identity.length > 128) return null;
  if (typeof r.id !== "string" || !SCRIPT_ID_RE.test(r.id)) return null;
  if (typeof r.sha256 !== "string" || !SHA256_RE.test(r.sha256)) return null;
  if (!isStringArray(r.matches, MAX_PATTERNS) || r.matches.length === 0) return null;
  const excludeMatches = r.excludeMatches === undefined ? [] : r.excludeMatches;
  if (!isStringArray(excludeMatches, MAX_PATTERNS)) return null;
  if (![...r.matches, ...excludeMatches].every(isAllowedMatchPattern)) return null;
  if (typeof r.runAt !== "string" || !RUN_AT.includes(r.runAt)) return null;
  if (r.world !== "isolated" && r.world !== "main") return null;
  return {
    identity: r.identity,
    pluginName: typeof r.pluginName === "string" ? r.pluginName : r.identity,
    pluginVersion: typeof r.pluginVersion === "string" ? r.pluginVersion : "",
    id: r.id,
    title: typeof r.title === "string" ? r.title : r.id,
    matches: r.matches,
    excludeMatches,
    runAt: r.runAt as PageScriptRunAt,
    world: r.world,
    allFrames: r.allFrames === true,
    sha256: r.sha256,
    bytes: typeof r.bytes === "number" && Number.isFinite(r.bytes) ? r.bytes : 0,
  };
}

/** 校验桌面端索引；非法条目被丢弃（不注册），整体形状错误返回 null。 */
export function parsePageScriptsIndex(raw: unknown): PageScriptsIndex | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.sequence !== "number" || !Number.isInteger(r.sequence) || r.sequence < 0) return null;
  if (typeof r.epoch !== "string") return null;
  if (!Array.isArray(r.scripts) || r.scripts.length > MAX_ENTRIES) return null;
  const seen = new Set<string>();
  const scripts: PageScriptIndexEntry[] = [];
  for (const item of r.scripts) {
    const entry = parseEntry(item);
    if (!entry) continue;
    const key = `${entry.identity}\n${entry.id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    scripts.push(entry);
  }
  return { sequence: r.sequence, epoch: r.epoch, scripts };
}

export function parsePageScriptSource(raw: unknown): PageScriptSource | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (
    typeof r.identity !== "string" ||
    typeof r.id !== "string" ||
    typeof r.sha256 !== "string" ||
    typeof r.code !== "string"
  ) {
    return null;
  }
  return { identity: r.identity, id: r.id, sha256: r.sha256, code: r.code };
}
