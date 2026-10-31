/**
 * 规则存储与编译：合并 builtin、本地规则、官方 / 社区包（`fluxdown_rule_packs`）与
 * 覆盖（overrides），编译为 CompiledRules。任一来源变化时重编译并广播 PageConfig 失效。
 *
 * 远程包只经桌面端 agent 下发（agent 负责验签与吊销）；扩展侧再做一次 validatePack。
 * 官方包随扩展附带快照（BUNDLED_OFFICIAL_PACK），远程同步到更高 sequence 后以远程为准。
 *
 * 同步的纯逻辑（应答应用、单飞互斥、结论归类）在 rule-sync.ts；本模块只做存储 / 定时 / 编译的接线。
 * 同步元信息（时间、结论、桌面端状态）单独放 `fluxdown_rule_sync_meta`：写它不会触发引擎重编译。
 */
import { browser } from "wxt/browser";
import { BUILTIN_PACK, compileBundled, compileRules, effectiveOfficialPack } from "@/rules/engine";
import type { CompiledRules, PackSlot } from "@/rules/engine";
import type { RuleOverrides, RulePack, SniffRuleV1 } from "@/rules/schema";
import { RULE_ENGINE_VERSION } from "@/rules/schema";
import { fetchSniffRules, getCapabilities, onCapabilities } from "./host";
import type { RuleDocSummary, RuleSyncResult, RuleSyncStatus, StoredRulePacks, StoredSyncMeta } from "./protocol";
import { canSkipStartupSync, createRuleSync } from "./rule-sync";
import type { RuleSyncTrigger } from "./rule-sync";
import { getCachedSettings } from "./settings-cache";
import { logWarn } from "@/utils/error-log";

export const LOCAL_RULES_KEY = "fluxdown_local_rules";
export const RULE_OVERRIDES_KEY = "fluxdown_rule_overrides";
export const RULE_PACKS_KEY = "fluxdown_rule_packs";
export const RULE_SYNC_META_KEY = "fluxdown_rule_sync_meta";
/** 最近一次编译输入的指纹（storage.session）：SW 重启后输入没变就不再向所有标签页广播。 */
const FINGERPRINT_KEY = "fluxdown_rules_fingerprint";
const SYNC_ALARM = "fluxdown-rules-sync";
const SYNC_PERIOD_MIN = 360;
const FIRST_SYNC_DELAY_MS = 5_000;
/** community 包带过期时间；编译快照超过 1 h 就后台重编译一次（输入指纹未变则不会真的重编译）。 */
const COMPILE_MAX_AGE_MS = 60 * 60 * 1000;

let engine: CompiledRules = compileBundled();
let compiledAt = Date.now();
let building: Promise<void> | null = null;
/** 编译期间收到新的重编译请求。 */
let dirty = false;
/** 当前 `engine` 对应的编译输入指纹。 */
let lastFingerprint: string | null = null;
/** 自本次后台启动起是否有人取过引擎（取过之后不能再靠「指纹没变」省掉失效广播）。 */
let engineServed = false;
let compileError: string | null = null;
const changeListeners: Array<() => void> = [];

/** 当前规则引擎（同步可用：未加载完远程包前是仅含 builtin 的引擎）。 */
export function getEngine(): CompiledRules {
  engineServed = true;
  if (Date.now() - compiledAt > COMPILE_MAX_AGE_MS) void rebuildEngine();
  return engine;
}

/** 引擎重编译完成后通知（PageConfig 失效广播、视图刷新）。 */
export function onEngineChanged(listener: () => void): void {
  changeListeners.push(listener);
}

async function loadStored(): Promise<StoredRulePacks> {
  const raw = (await browser.storage.local.get(RULE_PACKS_KEY))[RULE_PACKS_KEY];
  return raw && typeof raw === "object" ? (raw as StoredRulePacks) : {};
}

/** 只写已知字段：旧版本遗留的 lastSyncAt / lastError 等在下一次保存时自然清除。 */
async function savePacks(packs: StoredRulePacks): Promise<void> {
  const clean: StoredRulePacks = {};
  if (packs.official) clean.official = packs.official;
  if (packs.community) clean.community = packs.community;
  if (packs.plugin) clean.plugin = packs.plugin;
  if (packs.pluginEpoch !== undefined) clean.pluginEpoch = packs.pluginEpoch;
  if (packs.revocations) clean.revocations = packs.revocations;
  await browser.storage.local.set({ [RULE_PACKS_KEY]: clean });
}

async function loadMeta(): Promise<StoredSyncMeta> {
  const raw = (await browser.storage.local.get(RULE_SYNC_META_KEY))[RULE_SYNC_META_KEY];
  return raw && typeof raw === "object" ? (raw as StoredSyncMeta) : {};
}

async function saveMeta(meta: StoredSyncMeta): Promise<void> {
  await browser.storage.local.set({ [RULE_SYNC_META_KEY]: meta });
}

function localPack(rules: SniffRuleV1[]): RulePack {
  return {
    format: "fluxdown-sniff-pack",
    packId: "local",
    sequence: 0,
    engine: RULE_ENGINE_VERSION,
    issuedAt: new Date(0).toISOString(),
    rules,
  };
}

interface CompileSources {
  localRules: SniffRuleV1[];
  overrides: RuleOverrides;
  stored: StoredRulePacks;
  sniffImages: boolean;
  communityEnabled: boolean;
}

async function loadSources(): Promise<CompileSources> {
  const [local, stored, settings] = await Promise.all([
    browser.storage.local.get([LOCAL_RULES_KEY, RULE_OVERRIDES_KEY]),
    loadStored(),
    getCachedSettings(),
  ]);
  return {
    localRules: Array.isArray(local[LOCAL_RULES_KEY]) ? (local[LOCAL_RULES_KEY] as SniffRuleV1[]) : [],
    overrides: (local[RULE_OVERRIDES_KEY] ?? {}) as RuleOverrides,
    stored,
    sniffImages: settings.sniffImages,
    communityEnabled: settings.communityRulesEnabled,
  };
}

function compileSources(sources: CompileSources, now: number): CompiledRules {
  const { localRules, overrides, stored } = sources;
  const packs: PackSlot[] = [
    { tier: "builtin", pack: BUILTIN_PACK },
    { tier: "official", pack: effectiveOfficialPack(stored.official) },
  ];
  if (stored.community) packs.push({ tier: "community", pack: stored.community });
  if (stored.plugin) packs.push({ tier: "plugin", pack: stored.plugin });
  if (localRules.length > 0) packs.push({ tier: "local", pack: localPack(localRules) });
  return compileRules({
    packs,
    overrides,
    revocations: stored.revocations ?? null,
    options: { sniffImages: sources.sniffImages, communityEnabled: sources.communityEnabled, now },
  });
}

/** 编译结果随时间变化的唯一来源是各文档「是否已过期」：把这几个布尔值并进指纹。 */
function expiryFlags(stored: StoredRulePacks, now: number): boolean[] {
  return [stored.official, stored.community, stored.plugin, stored.revocations].map(
    (doc) => doc?.expiresAt !== undefined && !(Date.parse(doc.expiresAt) > now),
  );
}

async function fingerprintOf(sources: CompileSources, now: number): Promise<string> {
  const text = JSON.stringify([
    sources.localRules,
    sources.overrides,
    sources.stored,
    sources.sniffImages,
    sources.communityEnabled,
    expiryFlags(sources.stored, now),
  ]);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function readSessionFingerprint(): Promise<string | null> {
  try {
    const raw = (await browser.storage.session?.get(FINGERPRINT_KEY))?.[FINGERPRINT_KEY];
    return typeof raw === "string" ? raw : null;
  } catch (e) {
    logWarn("[FluxDown] rule fingerprint read failed:", e);
    return null;
  }
}

async function writeSessionFingerprint(fingerprint: string): Promise<void> {
  try {
    await browser.storage.session?.set({ [FINGERPRINT_KEY]: fingerprint });
  } catch (e) {
    logWarn("[FluxDown] rule fingerprint write failed:", e);
  }
}

/**
 * 重新编译（并发合并为同一次）；完成后通知监听者。
 * 编译输入（本地规则 / 覆盖 / 规则包 / 影响编译的设置 / 各文档过期状态）的指纹没变就既不编译也不通知。
 * 编译期间又来了新的触发时置 `dirty`，本轮结束后再编译一轮，避免读取存储之后才发生的变更被吞掉。
 */
export function rebuildEngine(): Promise<void> {
  if (building) {
    dirty = true;
    return building;
  }
  building = (async () => {
    do {
      dirty = false;
      try {
        const sources = await loadSources();
        const now = Date.now();
        const fingerprint = await fingerprintOf(sources, now);
        compiledAt = now;
        if (fingerprint === lastFingerprint) continue;
        const firstBuild = lastFingerprint === null;
        // 后台刚启动、还没人取过引擎，且输入与上次启动时完全一致：页面里的配置仍然有效，不必广播。
        const unchangedSinceLastRun =
          firstBuild && !engineServed && (await readSessionFingerprint()) === fingerprint;
        engine = compileSources(sources, now);
        lastFingerprint = fingerprint;
        compileError = null;
        if (unchangedSinceLastRun) continue;
        await writeSessionFingerprint(fingerprint);
        for (const listener of changeListeners) listener();
      } catch (e) {
        // 失败也刷新时间戳：否则 getEngine() 每次调用都会再触发一次注定失败的全量编译。
        compiledAt = Date.now();
        compileError = e instanceof Error ? e.message : String(e);
        logWarn("[FluxDown] rule engine rebuild failed:", e);
      }
    } while (dirty);
  })().finally(() => {
    building = null;
  });
  return building;
}

/** 编译（若有）进行中则等它完成；供「列出规则」等需要最新引擎的读取方使用。 */
export function engineSettled(): Promise<void> {
  return building ?? Promise.resolve();
}

/**
 * 用「当前所有来源 + 一条草稿本地规则」编译一个临时引擎（规则测试台用，不影响正在生效的引擎）。
 * 草稿按 id 替换同 id 的本地规则，强制为 local 层。
 */
export async function compileWithDraft(draft: SniffRuleV1): Promise<CompiledRules> {
  const sources = await loadSources();
  const asLocal: SniffRuleV1 = { ...draft, tier: "local" };
  const localRules = [...sources.localRules.filter((rule) => rule.id !== draft.id), asLocal];
  return compileSources({ ...sources, localRules }, Date.now());
}

function summarize(
  doc: { sequence: number; issuedAt: string; expiresAt?: string; rules?: unknown[]; entries?: unknown[] } | undefined,
): RuleDocSummary | null {
  if (!doc) return null;
  const count = (doc.rules ?? doc.entries ?? []).length;
  return { sequence: doc.sequence, issuedAt: doc.issuedAt, expiresAt: doc.expiresAt, count };
}

/** 规则同步状态（options `#rules` 与诊断页展示）。 */
export async function getRuleSyncStatus(): Promise<RuleSyncStatus> {
  const [stored, meta, settings] = await Promise.all([loadStored(), loadMeta(), getCachedSettings()]);
  return {
    enabled: settings.ruleSyncEnabled,
    communityEnabled: settings.communityRulesEnabled,
    official: summarize(effectiveOfficialPack(stored.official)),
    community: summarize(stored.community),
    plugin: summarize(stored.plugin),
    revocations: summarize(stored.revocations),
    lastSyncAt: meta.lastSyncAt ?? null,
    lastAttemptAt: meta.lastAttemptAt ?? null,
    lastOutcome: meta.lastOutcome ?? null,
    problems: meta.problems ?? [],
    compileError,
    host: meta.host ?? null,
  };
}

const ruleSync = createRuleSync({
  remoteEnabled: async () => (await getCachedSettings()).ruleSyncEnabled,
  capabilities: getCapabilities,
  loadPacks: loadStored,
  savePacks,
  loadMeta,
  saveMeta,
  fetch: fetchSniffRules,
  status: getRuleSyncStatus,
  now: Date.now,
  later: (run, delayMs) => {
    setTimeout(run, delayMs);
  },
});

/**
 * 从桌面端拉取规则包（单飞）。默认手动：强制桌面端拉取并等待。`ruleSyncEnabled` 关闭时只同步本机 plugin 层。
 * `outcome` 区分「未同步」的原因，UI 不得把空操作当作成功。
 */
export function syncRemotePacks(trigger: RuleSyncTrigger = "manual"): Promise<RuleSyncResult> {
  return ruleSync.sync(trigger);
}

function autoSync(reason: string): void {
  syncRemotePacks("auto").catch((e) => logWarn(`[FluxDown] ${reason} failed:`, e));
}

/** 近期同步过且本地已有远程包就不在启动时再同步（SW 频繁唤醒不应每次都打到桌面端）。 */
async function startupSync(): Promise<void> {
  const [meta, stored] = await Promise.all([loadMeta(), loadStored()]);
  if (canSkipStartupSync(meta, stored, Date.now())) return;
  await syncRemotePacks("auto");
}

/** `settings` 里只有这两项会影响编译结果；其余设置变化不必碰规则引擎。 */
function compileFlagsChanged(before: unknown, after: unknown): boolean {
  const flags = (value: unknown): [unknown, unknown] | null =>
    value && typeof value === "object" && "sniffImages" in value && "communityRulesEnabled" in value
      ? [value.sniffImages, value.communityRulesEnabled]
      : null;
  const a = flags(before);
  const b = flags(after);
  // 任一侧缺字段（走默认值）时无法比较：保守地重编译，指纹会挡住无变化的情形。
  return a === null || b === null || a[0] !== b[0] || a[1] !== b[1];
}

/** 同步注册 storage 监听与 alarm；由 startBackground 调用一次。 */
export function installRulesStore(): void {
  browser.storage.onChanged.addListener((changes, area) => {
    if (area === "local") {
      if (changes[LOCAL_RULES_KEY] || changes[RULE_OVERRIDES_KEY] || changes[RULE_PACKS_KEY]) {
        void rebuildEngine();
      }
    } else if (area === "sync" && changes.settings) {
      if (compileFlagsChanged(changes.settings.oldValue, changes.settings.newValue)) void rebuildEngine();
    }
  });
  browser.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === SYNC_ALARM) autoSync("rule sync");
  });
  // 同名 alarm 重复 create 会重置计时：每次 SW 唤醒都 create 的话 6 h 定时永远走不到。
  browser.alarms
    .get(SYNC_ALARM)
    .then((existing) => {
      if (existing?.periodInMinutes === SYNC_PERIOD_MIN) return undefined;
      return browser.alarms.create(SYNC_ALARM, { periodInMinutes: SYNC_PERIOD_MIN });
    })
    .catch((e) => logWarn("[FluxDown] rule sync alarm setup failed:", e));
  onCapabilities((caps) => {
    if (!caps.connected) return;
    void loadStored().then((stored) => {
      const behind = caps.pluginRulesSequence > (stored.plugin?.sequence ?? 0);
      // 桌面端数据库被重置后序号会倒退：靠 epoch 识别，不能只比序号。
      const reset =
        typeof caps.pluginRulesEpoch === "string" &&
        stored.plugin !== undefined &&
        stored.pluginEpoch !== caps.pluginRulesEpoch;
      if (behind || reset) autoSync("plugin rule sync");
    });
  });
  void rebuildEngine();
  setTimeout(() => {
    startupSync().catch((e) => logWarn("[FluxDown] rule sync failed:", e));
  }, FIRST_SYNC_DELAY_MS);
}
