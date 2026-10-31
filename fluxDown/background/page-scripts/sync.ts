/**
 * 页面脚本同步（reconcile）：拉索引 → 已批准插件逐条比对 → 变化 / 缺失才拉源码并 register / update →
 * 不在索引或未批准的注销。
 *
 * 约束（契约 §5 / 用户决策 2）：
 * - 源码只在注册那一刻经内存传给浏览器，不写入扩展 storage；
 * - 桌面不可达保留现有注册；拉到的源码 sha256 与索引不符不注册并记录错误；
 * - 注册 / 更新以单插件为原子单位，失败不影响其它插件。
 *
 * 浏览器 API、桌面端、存储、时钟全部经 {@link SyncDeps} 注入，本模块可直接用假依赖测试。
 */
import type { Browser } from "wxt/browser";
import type {
  PageScriptSource,
  PageScriptsAvailability,
  PageScriptsIndex,
  PageScriptsSyncState,
  PluginError,
  RegisteredRecord,
  SyncOutcome,
} from "./model";
import { planSync, registrationsOfPlugin } from "./plan";
import type { PluginPlan } from "./plan";
import { buildRegistration, fingerprintOf, registrationId, worldIdOf, worldProperties } from "./registration";
import type { PageScriptStore } from "./store";
import { logWarn } from "@/utils/error-log";

/** 触发来源：用户操作与定时 / 启动优先于能力探测（后者只在序号 / epoch 变化时才真正跑）。 */
export type SyncTrigger = "capabilities" | "startup" | "installed" | "permission" | "alarm" | "user" | "view";

/** 同步用到的桌面端能力快照。 */
export interface SyncCaps {
  protocol: 1 | 2;
  connected: boolean;
  actions: string[];
  pageScriptsSequence: number | null;
  pageScriptsEpoch: string | null;
}

export type SourceResult = { ok: true; source: PageScriptSource } | { ok: false; message?: string };

/** `browser.userScripts` 的窄封装。 */
export interface UserScriptsApi {
  getIds(): Promise<string[]>;
  register(scripts: Browser.userScripts.RegisteredUserScript[]): Promise<void>;
  update(scripts: Browser.userScripts.RegisteredUserScript[]): Promise<void>;
  unregister(ids: string[]): Promise<void>;
  configureWorld(properties: Browser.userScripts.WorldProperties): Promise<void>;
  resetWorld(worldId: string): Promise<void>;
}

export interface SyncDeps {
  store: PageScriptStore;
  availability(): Promise<PageScriptsAvailability>;
  capabilities(force: boolean): Promise<SyncCaps>;
  /** 远程 always 模式：功能不可用。 */
  remoteMode(): Promise<boolean>;
  fetchIndex(): Promise<PageScriptsIndex | null>;
  fetchSource(identity: string, id: string): Promise<SourceResult>;
  sha256(text: string): Promise<string>;
  scripts: UserScriptsApi;
  now(): number;
}

export interface PageScriptSync {
  /** 同步一次；等待中的同步会被合并（用户触发优先于能力探测）。 */
  reconcile(trigger: SyncTrigger, caps?: SyncCaps): Promise<SyncOutcome>;
  /** 注销某插件全部已注册脚本并重置其 world（撤销批准时用，不依赖桌面端可达）。 */
  removePlugin(identity: string): Promise<void>;
}

const TOO_LARGE_MESSAGE = "page script too large";

type PluginResult =
  | { ok: true; identity: string; records: Record<string, RegisteredRecord> }
  | { ok: false; identity: string; error: PluginError; transient: boolean };

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function createPageScriptSync(deps: SyncDeps): PageScriptSync {
  const { store, scripts } = deps;

  let tail: Promise<unknown> = Promise.resolve();
  function exclusive<T>(fn: () => Promise<T>): Promise<T> {
    const run = tail.then(fn, fn);
    tail = run.catch(() => undefined);
    return run;
  }

  async function finish(outcome: SyncOutcome, patch?: (state: PageScriptsSyncState) => void): Promise<SyncOutcome> {
    await store.updateSync((state) => {
      state.outcome = outcome;
      state.lastAttemptAt = deps.now();
      patch?.(state);
    });
    return outcome;
  }

  async function applyPlugin(
    plan: PluginPlan,
    token: string,
    registeredIds: ReadonlySet<string>,
    records: Readonly<Record<string, RegisteredRecord>>,
  ): Promise<PluginResult> {
    const { identity, upsert } = plan;
    if (upsert.length === 0) return { ok: true, identity, records: {} };

    const fail = (error: PluginError, transient = false): PluginResult => ({ ok: false, identity, error, transient });

    // 先全部拉取并校验，任何一条失败整个插件不动（原子单位）。
    const codes = new Map<string, string>();
    for (const entry of upsert) {
      const res = await deps.fetchSource(entry.identity, entry.id);
      if (!res.ok) {
        const tooLarge = res.message === TOO_LARGE_MESSAGE;
        return fail(
          { code: tooLarge ? "source_too_large" : "fetch_failed", scriptId: entry.id, detail: res.message },
          !tooLarge,
        );
      }
      const { source } = res;
      const actual = await deps.sha256(source.code);
      if (
        source.identity !== entry.identity ||
        source.id !== entry.id ||
        source.sha256 !== entry.sha256 ||
        actual !== entry.sha256
      ) {
        return fail({ code: "sha_mismatch", scriptId: entry.id });
      }
      codes.set(entry.id, source.code);
    }

    const toRegister: Browser.userScripts.RegisteredUserScript[] = [];
    const toUpdate: Browser.userScripts.RegisteredUserScript[] = [];
    const replaceIds: string[] = [];
    for (const entry of upsert) {
      const id = registrationId(identity, entry.id);
      const registration = buildRegistration(entry, token, codes.get(entry.id) ?? "");
      if (!registeredIds.has(id)) {
        toRegister.push(registration);
      } else if (records[id]?.world === entry.world) {
        toUpdate.push(registration);
      } else {
        // 世界变化（isolated ↔ main）或没有记录：整条替换。
        replaceIds.push(id);
        toRegister.push(registration);
      }
    }

    try {
      if (upsert.some((entry) => entry.world === "isolated")) {
        await scripts.configureWorld(worldProperties(worldIdOf(identity)));
      }
      if (replaceIds.length > 0) await scripts.unregister(replaceIds);
      if (toRegister.length > 0) await scripts.register(toRegister);
      if (toUpdate.length > 0) {
        try {
          await scripts.update(toUpdate);
        } catch (e) {
          logWarn("[FluxDown] userScripts.update failed, re-registering:", e);
          await scripts.unregister(toUpdate.map((s) => s.id));
          await scripts.register(toUpdate);
        }
      }
    } catch (e) {
      return fail({ code: "register_failed", detail: errorText(e) });
    }

    const out: Record<string, RegisteredRecord> = {};
    for (const entry of upsert) {
      out[registrationId(identity, entry.id)] = {
        sha256: entry.sha256,
        fingerprint: fingerprintOf(entry),
        world: entry.world,
      };
    }
    return { ok: true, identity, records: out };
  }

  async function run(trigger: SyncTrigger, hint?: SyncCaps): Promise<SyncOutcome> {
    if (await deps.remoteMode()) return finish("remote-mode");

    const caps = hint ?? (await deps.capabilities(trigger === "user" || trigger === "alarm"));
    if (!caps.connected) return finish("offline");
    if (caps.protocol < 2 || !caps.actions.includes("page_scripts")) return finish("host-unsupported");

    if (trigger === "capabilities") {
      const state = await store.loadSync();
      const known = caps.pageScriptsSequence !== null && caps.pageScriptsEpoch !== null;
      // 桌面端没报序号（daemon 失败 / 无该方法）= 无信息；序号与 epoch 都没变 = 无事可做。
      if (!known) return state.outcome ?? "ok";
      if (
        state.outcome === "ok" &&
        state.applied?.sequence === caps.pageScriptsSequence &&
        state.applied.epoch === caps.pageScriptsEpoch
      ) {
        return "ok";
      }
    }

    const index = await deps.fetchIndex();
    if (!index) return finish("offline");

    const availability = await deps.availability();
    if (availability !== "ok") {
      return finish("browser-unavailable", (state) => {
        state.index = index.scripts;
      });
    }

    let registeredIds: string[];
    try {
      registeredIds = await scripts.getIds();
    } catch (e) {
      logWarn("[FluxDown] userScripts.getScripts failed:", e);
      return finish("browser-unavailable", (state) => {
        state.index = index.scripts;
      });
    }

    const approvals = await store.loadApprovals();
    const before = await store.loadSync();
    const plan = planSync(index.scripts, new Set(Object.keys(approvals)), registeredIds, before.registered);
    const registeredSet = new Set(registeredIds);

    const results: PluginResult[] = [];
    for (const pluginPlan of plan.plugins) {
      const approval = approvals[pluginPlan.identity];
      if (!approval) continue;
      results.push(await applyPlugin(pluginPlan, approval.token, registeredSet, before.registered));
    }

    if (plan.orphans.length > 0) {
      try {
        await scripts.unregister(plan.orphans);
      } catch (e) {
        logWarn("[FluxDown] userScripts.unregister failed:", e);
      }
    }

    const transientFailure = results.some((r) => !r.ok && r.transient);
    return finish("ok", (state) => {
      const live = new Set(registeredIds.filter((id) => !plan.orphans.includes(id)));
      state.index = index.scripts;
      state.registered = Object.fromEntries(Object.entries(state.registered).filter(([id]) => live.has(id)));
      const wantedIdentities = new Set(plan.plugins.map((p) => p.identity));
      for (const identity of Object.keys(state.errors)) {
        if (!wantedIdentities.has(identity)) delete state.errors[identity];
      }
      for (const result of results) {
        if (result.ok) {
          delete state.errors[result.identity];
          Object.assign(state.registered, result.records);
        } else {
          state.errors[result.identity] = result.error;
        }
      }
      state.lastSyncAt = deps.now();
      if (!transientFailure) state.applied = { sequence: index.sequence, epoch: index.epoch };
      else delete state.applied;
    });
  }

  interface Waiting {
    trigger: SyncTrigger;
    caps?: SyncCaps;
    started: boolean;
    promise: Promise<SyncOutcome>;
  }
  let waiting: Waiting | null = null;

  return {
    reconcile(trigger, caps) {
      if (waiting && !waiting.started) {
        if (trigger !== "capabilities") {
          waiting.trigger = trigger;
          waiting.caps = caps;
        }
        return waiting.promise;
      }
      const entry: Waiting = { trigger, caps, started: false, promise: Promise.resolve("ok") };
      entry.promise = exclusive(() => {
        entry.started = true;
        if (waiting === entry) waiting = null;
        return run(entry.trigger, entry.caps);
      });
      waiting = entry;
      return entry.promise;
    },

    removePlugin(identity) {
      return exclusive(async () => {
        let ids: string[];
        try {
          ids = registrationsOfPlugin(identity, await scripts.getIds());
        } catch (e) {
          // userScripts 不可用 = 浏览器侧本来就没有注册，无需注销。
          console.debug("[FluxDown] removePlugin: userScripts unavailable:", e);
          return;
        }
        if (ids.length > 0) await scripts.unregister(ids);
        try {
          await scripts.resetWorld(worldIdOf(identity));
        } catch (e) {
          console.debug("[FluxDown] resetWorldConfiguration skipped:", e);
        }
      });
    },
  };
}
