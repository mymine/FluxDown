/**
 * 页面脚本的持久状态（storage.local）：批准记录（含令牌）、同步状态、按插件命名空间的桥存储。
 * 存储区经 {@link StorageLike} 注入，测试用内存实现。
 *
 * 读改写经同一把串行锁，避免并发的批准 / 撤销 / 同步状态写入互相覆盖。
 */
import { emptySyncState } from "./model";
import type { PageScriptsSyncState, PluginApproval, PluginApprovals } from "./model";
import { worldIdOf } from "./registration";
import { MAX_STORAGE_BYTES } from "./wire";

export const APPROVALS_KEY = "fluxdown_page_script_approvals";
export const SYNC_KEY = "fluxdown_page_script_sync";
export const PLUGIN_STORAGE_PREFIX = "fluxdown_ps:";

export interface StorageLike {
  get(keys: string | string[]): Promise<Record<string, unknown>>;
  set(values: Record<string, unknown>): Promise<void>;
  remove(keys: string | string[]): Promise<void>;
}

const MIN_TOKEN_LENGTH = 32;

function isApproval(value: unknown): value is PluginApproval {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    v.approved === true &&
    typeof v.approvedAt === "number" &&
    typeof v.token === "string" &&
    v.token.length >= MIN_TOKEN_LENGTH &&
    typeof v.pluginName === "string"
  );
}

function newToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function pluginStorageKey(identity: string): string {
  return `${PLUGIN_STORAGE_PREFIX}${identity}`;
}

/** 值是否可 JSON 往返（桥存储只收可序列化数据）。 */
function jsonSize(value: unknown): number | null {
  let text: string | undefined;
  try {
    text = JSON.stringify(value);
  } catch {
    return null;
  }
  return text === undefined ? null : new TextEncoder().encode(text).length;
}

export interface PageScriptStore {
  loadApprovals(): Promise<PluginApprovals>;
  /** 批准（已批准则保留原令牌）。 */
  approve(identity: string, pluginName: string, now: number): Promise<PluginApproval>;
  /** 撤销批准并清掉该插件的同步记录与桥存储；返回之前是否已批准。 */
  revoke(identity: string): Promise<boolean>;
  /** 令牌 → 插件 identity；未批准 / 令牌不符为 null。 */
  identityForToken(token: string): Promise<string | null>;
  loadSync(): Promise<PageScriptsSyncState>;
  /** 读改写同步状态。 */
  updateSync(change: (state: PageScriptsSyncState) => void): Promise<PageScriptsSyncState>;
  storageGet(identity: string, key: string): Promise<unknown>;
  storageSet(identity: string, key: string, value: unknown): Promise<void>;
  storageRemove(identity: string, key: string): Promise<void>;
}

export class StorageQuotaError extends Error {
  constructor() {
    super("storage quota exceeded");
  }
}

export class StorageValueError extends Error {
  constructor() {
    super("value is not JSON-serializable");
  }
}

export function createPageScriptStore(area: StorageLike): PageScriptStore {
  let tail: Promise<unknown> = Promise.resolve();
  function exclusive<T>(fn: () => Promise<T>): Promise<T> {
    const run = tail.then(fn, fn);
    tail = run.catch(() => undefined);
    return run;
  }

  async function readApprovals(): Promise<PluginApprovals> {
    const raw = (await area.get(APPROVALS_KEY))[APPROVALS_KEY];
    const out: PluginApprovals = {};
    if (typeof raw !== "object" || raw === null) return out;
    for (const [identity, value] of Object.entries(raw)) {
      if (isApproval(value)) out[identity] = value;
    }
    return out;
  }

  async function readSync(): Promise<PageScriptsSyncState> {
    const raw = (await area.get(SYNC_KEY))[SYNC_KEY];
    if (typeof raw !== "object" || raw === null) return emptySyncState();
    const r = raw as Partial<PageScriptsSyncState>;
    return {
      applied: r.applied,
      index: Array.isArray(r.index) ? r.index : [],
      registered: r.registered && typeof r.registered === "object" ? r.registered : {},
      errors: r.errors && typeof r.errors === "object" ? r.errors : {},
      lastAttemptAt: r.lastAttemptAt,
      lastSyncAt: r.lastSyncAt,
      outcome: r.outcome,
    };
  }

  async function readPluginData(identity: string): Promise<Record<string, unknown>> {
    const raw = (await area.get(pluginStorageKey(identity)))[pluginStorageKey(identity)];
    return typeof raw === "object" && raw !== null && !Array.isArray(raw) ? { ...raw } : {};
  }

  return {
    loadApprovals: () => exclusive(readApprovals),

    approve(identity, pluginName, now) {
      return exclusive(async () => {
        const approvals = await readApprovals();
        const existing = approvals[identity];
        const next: PluginApproval = existing
          ? { ...existing, pluginName }
          : { approved: true, approvedAt: now, token: newToken(), pluginName };
        approvals[identity] = next;
        await area.set({ [APPROVALS_KEY]: approvals });
        return next;
      });
    },

    revoke(identity) {
      return exclusive(async () => {
        const approvals = await readApprovals();
        const was = identity in approvals;
        if (was) {
          delete approvals[identity];
          await area.set({ [APPROVALS_KEY]: approvals });
        }
        await area.remove(pluginStorageKey(identity));
        const state = await readSync();
        delete state.errors[identity];
        const prefix = `${worldIdOf(identity)}.`;
        state.registered = Object.fromEntries(
          Object.entries(state.registered).filter(([id]) => !id.startsWith(prefix)),
        );
        await area.set({ [SYNC_KEY]: state });
        return was;
      });
    },

    identityForToken(token) {
      return exclusive(async () => {
        const approvals = await readApprovals();
        for (const [identity, approval] of Object.entries(approvals)) {
          if (approval.token === token) return identity;
        }
        return null;
      });
    },

    loadSync: () => exclusive(readSync),

    updateSync(change) {
      return exclusive(async () => {
        const state = await readSync();
        change(state);
        await area.set({ [SYNC_KEY]: state });
        return state;
      });
    },

    storageGet(identity, key) {
      return exclusive(async () => {
        const data = await readPluginData(identity);
        return Object.hasOwn(data, key) ? data[key] : undefined;
      });
    },

    storageSet(identity, key, value) {
      return exclusive(async () => {
        if (jsonSize(value) === null) throw new StorageValueError();
        const data = await readPluginData(identity);
        data[key] = value;
        const total = jsonSize(data);
        if (total === null) throw new StorageValueError();
        if (total > MAX_STORAGE_BYTES) throw new StorageQuotaError();
        await area.set({ [pluginStorageKey(identity)]: data });
      });
    },

    storageRemove(identity, key) {
      return exclusive(async () => {
        const data = await readPluginData(identity);
        if (!Object.hasOwn(data, key)) return;
        delete data[key];
        await area.set({ [pluginStorageKey(identity)]: data });
      });
    },
  };
}
