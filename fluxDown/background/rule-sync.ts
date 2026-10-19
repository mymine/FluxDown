/**
 * 规则同步的纯逻辑：应答应用（校验 / 回滚 / plugin epoch）、单飞互斥与结论归类。
 *
 * 不依赖 `browser.*` 与 host：存储、NMH、设置、定时器都经 {@link RuleSyncDeps} 注入，
 * rules-store.ts 负责接线，本模块可直接用假依赖测试。
 */
import { effectiveOfficialPack } from "@/rules/engine";
import type { RevocationDoc, RulePack } from "@/rules/schema";
import { validatePack } from "@/rules/validate";
import type { SniffRulesHave, SniffRulesOptions, SniffRulesReply } from "./host";
import type {
  HostSyncStatus,
  RuleSyncOutcome,
  RuleSyncProblem,
  RuleSyncResult,
  RuleSyncStatus,
  StoredRulePacks,
  StoredSyncMeta,
} from "./protocol";

/** 手动 = 用户点「立即同步」（强制桌面端拉取并等待）；自动 = 定时 / 启动 / 插件规则变化；followup = 自动同步后的补查。 */
export type RuleSyncTrigger = "manual" | "auto" | "followup";

/** 桌面端仍在后台拉取时，补查一次的等待时间。 */
export const FOLLOW_UP_DELAY_MS = 10_000;
/** 应答因体积被拆分时最多连续补取的轮数（官方 / 社区 / 吊销 / plugin 各一轮足够）。 */
const MAX_ROUNDS = 4;
/** 启动同步：最近一次成功同步距今不足此时长且已有远程包则跳过。 */
export const STARTUP_SYNC_MIN_AGE_MS = 60 * 60 * 1000;

export interface RuleSyncDeps {
  remoteEnabled(): Promise<boolean>;
  capabilities(force: boolean): Promise<{ protocol: 1 | 2 }>;
  loadPacks(): Promise<StoredRulePacks>;
  savePacks(packs: StoredRulePacks): Promise<void>;
  loadMeta(): Promise<StoredSyncMeta>;
  saveMeta(meta: StoredSyncMeta): Promise<void>;
  fetch(have: SniffRulesHave, options: SniffRulesOptions): Promise<SniffRulesReply | null>;
  status(): Promise<RuleSyncStatus>;
  now(): number;
  /** 延迟执行一次补查。 */
  later(run: () => void, delayMs: number): void;
}

/** 吊销条目的形状：来自签名文档，但扩展侧仍按畸形输入防御，避免一条坏条目让整个引擎编译失败。 */
function isRevocationEntry(entry: unknown): boolean {
  if (!entry || typeof entry !== "object") return false;
  if (!("ruleId" in entry) || typeof entry.ruleId !== "string") return false;
  if (!("action" in entry) || (entry.action !== "soft" && entry.action !== "hard")) return false;
  if ("reason" in entry && entry.reason !== undefined && typeof entry.reason !== "string") return false;
  if (!("versions" in entry)) return false;
  const { versions } = entry;
  if (versions === "*") return true;
  return (
    typeof versions === "object" &&
    versions !== null &&
    "max" in versions &&
    typeof versions.max === "number" &&
    Number.isFinite(versions.max)
  );
}

export function isRevocationDoc(value: unknown): value is RevocationDoc {
  if (!value || typeof value !== "object") return false;
  if (!("format" in value) || value.format !== "fluxdown-sniff-revocations") return false;
  if (!("sequence" in value) || !Number.isInteger(value.sequence)) return false;
  if (!("issuedAt" in value) || typeof value.issuedAt !== "string") return false;
  if (!("expiresAt" in value) || typeof value.expiresAt !== "string") return false;
  return "entries" in value && Array.isArray(value.entries) && value.entries.every(isRevocationEntry);
}

export function haveOf(stored: StoredRulePacks): SniffRulesHave {
  return {
    official: effectiveOfficialPack(stored.official).sequence,
    community: stored.community?.sequence,
    plugin: stored.plugin?.sequence,
    pluginEpoch: stored.pluginEpoch,
    revocations: stored.revocations?.sequence,
  };
}

/** 启动同步是否可以跳过：近期同步过且本地已有远程包（否则新装 / 清空后要尽快取到）。 */
export function canSkipStartupSync(meta: StoredSyncMeta, stored: StoredRulePacks, now: number): boolean {
  const age = meta.lastSyncAt === undefined ? Number.POSITIVE_INFINITY : now - meta.lastSyncAt;
  const hasRemote = stored.official !== undefined || stored.community !== undefined || stored.revocations !== undefined;
  return age >= 0 && age < STARTUP_SYNC_MIN_AGE_MS && hasRemote;
}

export function normalizeHostStatus(raw: Partial<HostSyncStatus> | null | undefined): HostSyncStatus | null {
  if (!raw || typeof raw !== "object") return null;
  return {
    lastFetchUnix: typeof raw.lastFetchUnix === "number" ? raw.lastFetchUnix : null,
    lastAttemptUnix: typeof raw.lastAttemptUnix === "number" ? raw.lastAttemptUnix : null,
    lastError: typeof raw.lastError === "string" ? raw.lastError : null,
    errors: raw.errors && typeof raw.errors === "object" ? raw.errors : {},
    refreshing: raw.refreshing === true,
  };
}

/** 校验收到的规则包；不合格返回 false 并记录问题。剔除的规则只计数：存原始包，扩展升级后由编译重新校验恢复。 */
function acceptPack(
  incoming: RulePack,
  tier: "official" | "community" | "plugin",
  problems: RuleSyncProblem[],
): boolean {
  try {
    const { rejected } = validatePack(incoming, tier);
    if (rejected.length > 0) problems.push({ code: "rules_rejected", tier, count: rejected.length });
    return true;
  } catch (e) {
    problems.push({ code: "pack_invalid", tier, detail: e instanceof Error ? e.message : String(e) });
    return false;
  }
}

function applyPlugin(next: StoredRulePacks, reply: SniffRulesReply, problems: RuleSyncProblem[]): boolean {
  // 本轮因体积被推迟，或桌面端没答复 plugin 查询：不改动 plugin 层（不把「查不到」当作「已清空」）。
  if (reply.deferred?.includes("plugin") || typeof reply.pluginEpoch !== "string") {
    return false;
  }
  if (next.pluginEpoch !== reply.pluginEpoch) {
    // 桌面端数据库被重置：序号重新计数，倒退不是回滚，整体替换（或清空）plugin 层。
    if (reply.plugin) {
      if (!acceptPack(reply.plugin, "plugin", problems)) return false;
      next.plugin = reply.plugin;
    } else if (reply.pluginSequence === 0) {
      delete next.plugin;
    } else {
      return false;
    }
    next.pluginEpoch = reply.pluginEpoch;
    return true;
  }
  const incoming = reply.plugin;
  if (!incoming) return false;
  const current = next.plugin;
  if (current && incoming.sequence === current.sequence) return false;
  if (current && incoming.sequence < current.sequence) {
    problems.push({ code: "rollback", tier: "plugin" });
    return false;
  }
  if (!acceptPack(incoming, "plugin", problems)) return false;
  next.plugin = incoming;
  return true;
}

export interface AppliedReply {
  next: StoredRulePacks;
  /** 规则包存储内容是否发生变化（决定要不要写 storage、触发重编译）。 */
  changed: boolean;
  problems: RuleSyncProblem[];
}

/** 把一次应答合进已存规则包；输入不变，只在真正有变化时 `changed`。`remote=false` 只处理 plugin 层。 */
export function applyReply(stored: StoredRulePacks, reply: SniffRulesReply, remote: boolean): AppliedReply {
  const next: StoredRulePacks = { ...stored };
  const problems: RuleSyncProblem[] = [];
  let changed = false;
  if (remote) {
    for (const tier of ["official", "community"] as const) {
      const incoming = reply[tier];
      if (!incoming) continue;
      const current = tier === "official" ? effectiveOfficialPack(stored.official) : stored.community;
      if (current && incoming.sequence === current.sequence) continue;
      if (current && incoming.sequence < current.sequence) {
        problems.push({ code: "rollback", tier });
        continue;
      }
      if (!acceptPack(incoming, tier, problems)) continue;
      next[tier] = incoming;
      changed = true;
    }
    const revocations = reply.revocations;
    if (revocations) {
      if (!isRevocationDoc(revocations)) {
        problems.push({ code: "revocations_invalid", tier: "revocations" });
      } else if (!stored.revocations || revocations.sequence > stored.revocations.sequence) {
        next.revocations = revocations;
        changed = true;
      }
    }
  }
  if (applyPlugin(next, reply, problems)) changed = true;
  return { next, changed, problems };
}

function outcomeOf(remote: boolean, problems: RuleSyncProblem[], host: HostSyncStatus | null): RuleSyncOutcome {
  if (problems.length > 0) return "partial";
  if (!remote) return "disabled";
  if (host?.lastError) return "fetch-failed";
  return "ok";
}

export interface RuleSync {
  /** 同步一次；同一时刻只有一个在跑（见 {@link createRuleSync}）。默认手动。 */
  sync(trigger?: RuleSyncTrigger): Promise<RuleSyncResult>;
}

/**
 * 创建同步协调器。
 * - 单飞：任意时刻至多一个同步在跑；自动 / 补查遇到进行中的同步直接共用其结果。
 * - 手动同步遇到进行中的非手动同步：等它结束后再强制跑一轮（不能搭载在一次没有 force 的同步上）。
 */
export function createRuleSync(deps: RuleSyncDeps): RuleSync {
  let current: { manual: boolean; promise: Promise<RuleSyncResult> } | null = null;
  let queuedManual: Promise<RuleSyncResult> | null = null;
  let followUpScheduled = false;

  async function finish(
    meta: StoredSyncMeta,
    patch: Partial<StoredSyncMeta>,
    outcome: RuleSyncOutcome,
  ): Promise<RuleSyncResult> {
    await deps.saveMeta({ ...meta, ...patch, lastAttemptAt: deps.now(), lastOutcome: outcome });
    return { outcome, status: await deps.status() };
  }

  async function run(trigger: RuleSyncTrigger): Promise<RuleSyncResult> {
    const manual = trigger === "manual";
    const remote = await deps.remoteEnabled();
    const caps = await deps.capabilities(manual);
    const meta = await deps.loadMeta();
    if (caps.protocol !== 2) return finish(meta, { problems: [] }, "offline");

    let stored = await deps.loadPacks();
    let packsChanged = false;
    let answered = false;
    let host: HostSyncStatus | null = null;
    const problems: RuleSyncProblem[] = [];
    for (let round = 0; round < MAX_ROUNDS; round++) {
      const reply = await deps.fetch(haveOf(stored), {
        force: manual && remote && round === 0,
        pluginOnly: !remote,
      });
      if (!reply) break;
      answered = true;
      const applied = applyReply(stored, reply, remote);
      stored = applied.next;
      problems.push(...applied.problems);
      if (applied.changed) packsChanged = true;
      if (remote && reply.status) host = normalizeHostStatus(reply.status);
      // 只有本轮有进展才继续补取被推迟的文档，避免死循环。
      if (!reply.deferred?.length || !applied.changed) break;
    }
    if (packsChanged) await deps.savePacks(stored);
    if (!answered) return finish(meta, { problems: [{ code: "unavailable" }] }, "unavailable");

    const outcome = outcomeOf(remote, problems, host);
    const patch: Partial<StoredSyncMeta> = { problems };
    if (remote) {
      patch.lastSyncAt = deps.now();
      if (host) patch.host = host;
    }
    const result = await finish(meta, patch, outcome);
    if (remote && trigger === "auto" && host?.refreshing && !followUpScheduled) {
      followUpScheduled = true;
      deps.later(() => {
        followUpScheduled = false;
        void sync("followup").catch(() => {});
      }, FOLLOW_UP_DELAY_MS);
    }
    return result;
  }

  function start(trigger: RuleSyncTrigger): Promise<RuleSyncResult> {
    const promise: Promise<RuleSyncResult> = run(trigger).finally(() => {
      if (current?.promise === promise) current = null;
    });
    current = { manual: trigger === "manual", promise };
    return promise;
  }

  function sync(trigger: RuleSyncTrigger = "manual"): Promise<RuleSyncResult> {
    if (!current) return start(trigger);
    if (trigger !== "manual" || current.manual) return current.promise;
    queuedManual ??= current.promise
      .catch(() => undefined)
      .then(() => {
        queuedManual = null;
        return sync("manual");
      });
    return queuedManual;
  }

  return { sync };
}
