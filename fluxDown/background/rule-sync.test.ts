import { describe, expect, test } from "bun:test";
import type { RevocationDoc, RulePack } from "@/rules/schema";
import type { SniffRulesHave, SniffRulesOptions, SniffRulesReply } from "./host";
import type { RuleSyncStatus, StoredRulePacks, StoredSyncMeta } from "./protocol";
import {
  applyReply,
  canSkipStartupSync,
  createRuleSync,
  FOLLOW_UP_DELAY_MS,
  isRevocationDoc,
} from "./rule-sync";
import type { RuleSyncDeps } from "./rule-sync";

function pack(packId: RulePack["packId"], sequence: number): RulePack {
  return {
    format: "fluxdown-sniff-pack",
    packId,
    sequence,
    engine: 1,
    issuedAt: "2026-10-01T00:00:00Z",
    rules: [],
  };
}

function revocations(sequence: number): RevocationDoc {
  return {
    format: "fluxdown-sniff-revocations",
    sequence,
    issuedAt: "2026-10-01T00:00:00Z",
    expiresAt: "2026-11-01T00:00:00Z",
    entries: [],
  };
}

function reply(patch: Partial<SniffRulesReply> = {}): SniffRulesReply {
  return {
    official: null,
    community: null,
    revocations: null,
    plugin: null,
    pluginSequence: null,
    pluginEpoch: null,
    status: { lastFetchUnix: 1, lastAttemptUnix: 1, lastError: null, errors: {}, refreshing: false },
    ...patch,
  };
}

describe("applyReply", () => {
  test("a reply with nothing new changes nothing (so nothing is written or rebuilt)", () => {
    const stored: StoredRulePacks = { community: pack("community", 4), revocations: revocations(2) };
    const applied = applyReply(stored, reply(), true);
    expect(applied.changed).toBe(false);
    expect(applied.problems).toEqual([]);
    expect(applied.next).toEqual(stored);
  });

  test("a newer community pack is stored and flagged as changed", () => {
    const applied = applyReply({ community: pack("community", 4) }, reply({ community: pack("community", 5) }), true);
    expect(applied.changed).toBe(true);
    expect(applied.next.community?.sequence).toBe(5);
  });

  test("a lower sequence within the same epoch is a rollback and is rejected", () => {
    const stored: StoredRulePacks = { plugin: pack("plugin", 5), pluginEpoch: "e1" };
    const applied = applyReply(
      stored,
      reply({ plugin: pack("plugin", 2), pluginSequence: 2, pluginEpoch: "e1" }),
      true,
    );
    expect(applied.changed).toBe(false);
    expect(applied.next.plugin?.sequence).toBe(5);
    expect(applied.problems).toEqual([{ code: "rollback", tier: "plugin" }]);
  });

  test("a new plugin epoch resets the layer even though the sequence went backwards", () => {
    const stored: StoredRulePacks = { plugin: pack("plugin", 9), pluginEpoch: "old" };
    const applied = applyReply(
      stored,
      reply({ plugin: pack("plugin", 1), pluginSequence: 1, pluginEpoch: "new" }),
      true,
    );
    expect(applied.changed).toBe(true);
    expect(applied.next.plugin?.sequence).toBe(1);
    expect(applied.next.pluginEpoch).toBe("new");
    expect(applied.problems).toEqual([]);
  });

  test("a new epoch with no plugin rules left clears the stored plugin pack", () => {
    const stored: StoredRulePacks = { plugin: pack("plugin", 9), pluginEpoch: "old" };
    const applied = applyReply(stored, reply({ pluginSequence: 0, pluginEpoch: "new" }), true);
    expect(applied.changed).toBe(true);
    expect(applied.next.plugin).toBeUndefined();
    expect(applied.next.pluginEpoch).toBe("new");
  });

  test("an unanswered plugin query or a deferred plugin pack never clears the layer", () => {
    const stored: StoredRulePacks = { plugin: pack("plugin", 9), pluginEpoch: "old" };
    expect(applyReply(stored, reply(), true).changed).toBe(false);
    const deferred = applyReply(stored, reply({ pluginSequence: 3, pluginEpoch: "new", deferred: ["plugin"] }), true);
    expect(deferred.changed).toBe(false);
    expect(deferred.next.plugin?.sequence).toBe(9);
  });

  test("with remote sync off only the plugin layer is applied", () => {
    const applied = applyReply(
      {},
      reply({
        official: pack("official", 7),
        revocations: revocations(3),
        plugin: pack("plugin", 1),
        pluginSequence: 1,
        pluginEpoch: "e1",
      }),
      false,
    );
    expect(applied.next.official).toBeUndefined();
    expect(applied.next.revocations).toBeUndefined();
    expect(applied.next.plugin?.sequence).toBe(1);
  });

  test("a malformed revocation list is rejected instead of poisoning compilation", () => {
    const bad = { ...revocations(3), entries: [{ ruleId: "x", action: "hard" }] } as unknown as RevocationDoc;
    const applied = applyReply({}, reply({ revocations: bad }), true);
    expect(applied.changed).toBe(false);
    expect(applied.problems).toEqual([{ code: "revocations_invalid", tier: "revocations" }]);
  });
});

describe("isRevocationDoc", () => {
  test("validates entry shapes", () => {
    const ok = {
      ...revocations(1),
      entries: [
        { ruleId: "a", versions: "*", action: "hard", reason: "r" },
        { ruleId: "b", versions: { max: 2 }, action: "soft", reason: "r" },
      ],
    };
    expect(isRevocationDoc(ok)).toBe(true);
    expect(isRevocationDoc({ ...ok, entries: [{ ruleId: "a", versions: null, action: "hard" }] })).toBe(false);
    expect(isRevocationDoc({ ...ok, entries: [{ ruleId: "a", versions: "*", action: "nuke" }] })).toBe(false);
    expect(isRevocationDoc({ ...ok, expiresAt: undefined })).toBe(false);
  });
});

describe("canSkipStartupSync", () => {
  const now = 10 * 60 * 60 * 1000;
  test("skips only when synced recently and remote packs are present", () => {
    const stored: StoredRulePacks = { official: pack("official", 2) };
    expect(canSkipStartupSync({ lastSyncAt: now - 30 * 60 * 1000 }, stored, now)).toBe(true);
    expect(canSkipStartupSync({ lastSyncAt: now - 2 * 60 * 60 * 1000 }, stored, now)).toBe(false);
    expect(canSkipStartupSync({ lastSyncAt: now - 1000 }, {}, now)).toBe(false);
    expect(canSkipStartupSync({}, stored, now)).toBe(false);
  });
});

interface Harness {
  deps: RuleSyncDeps;
  calls: Array<{ have: SniffRulesHave; options: SniffRulesOptions }>;
  saves: StoredRulePacks[];
  metas: StoredSyncMeta[];
  timers: Array<{ run: () => void; delayMs: number }>;
  maxConcurrent: () => number;
  release: () => void;
}

/** 假依赖：`fetch` 可挂起，用来观察并发；存储是内存对象。 */
function harness(options: {
  remote?: boolean;
  replies?: Array<SniffRulesReply | null>;
  hold?: boolean;
  stored?: StoredRulePacks;
}): Harness {
  let packs: StoredRulePacks = options.stored ?? {};
  let meta: StoredSyncMeta = {};
  const calls: Harness["calls"] = [];
  const saves: StoredRulePacks[] = [];
  const metas: StoredSyncMeta[] = [];
  const timers: Harness["timers"] = [];
  const queue = [...(options.replies ?? [])];
  let active = 0;
  let peak = 0;
  let gate: (() => void) | null = null;
  const status: RuleSyncStatus = {
    enabled: true,
    communityEnabled: true,
    official: null,
    community: null,
    plugin: null,
    revocations: null,
    lastSyncAt: null,
    lastAttemptAt: null,
    lastOutcome: null,
    problems: [],
    compileError: null,
    host: null,
  };
  const deps: RuleSyncDeps = {
    remoteEnabled: async () => options.remote ?? true,
    capabilities: async () => ({ protocol: 2 }),
    loadPacks: async () => packs,
    savePacks: async (next) => {
      packs = next;
      saves.push(next);
    },
    loadMeta: async () => meta,
    saveMeta: async (next) => {
      meta = next;
      metas.push(next);
    },
    fetch: async (have, opts) => {
      calls.push({ have, options: opts });
      active += 1;
      peak = Math.max(peak, active);
      if (options.hold) await new Promise<void>((resolve) => (gate = resolve));
      active -= 1;
      return queue.length > 0 ? (queue.shift() ?? null) : reply();
    },
    status: async () => status,
    now: () => 1_000,
    later: (run, delayMs) => {
      timers.push({ run, delayMs });
    },
  };
  return {
    deps,
    calls,
    saves,
    metas,
    timers,
    maxConcurrent: () => peak,
    release: () => gate?.(),
  };
}

/** 假依赖全是微任务：让出若干轮即可让挂起的链推进到下一个等待点（不依赖真实时间）。 */
async function tick(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

describe("createRuleSync", () => {
  test("manual sync forces the desktop to pull; automatic sync does not", async () => {
    const h = harness({});
    const sync = createRuleSync(h.deps);
    await sync.sync("auto");
    await sync.sync("manual");
    expect(h.calls.map((c) => c.options.force)).toEqual([false, true]);
  });

  test("with remote sync disabled only the local plugin layer is requested and the outcome says disabled", async () => {
    const h = harness({ remote: false });
    const result = await createRuleSync(h.deps).sync("manual");
    expect(h.calls[0].options).toEqual({ force: false, pluginOnly: true });
    expect(result.outcome).toBe("disabled");
  });

  test("a no-op sync does not rewrite the stored packs but records the attempt", async () => {
    const h = harness({ stored: { community: pack("community", 3) } });
    const result = await createRuleSync(h.deps).sync("auto");
    expect(result.outcome).toBe("ok");
    expect(h.saves).toEqual([]);
    expect(h.metas).toHaveLength(1);
    expect(h.metas[0].lastSyncAt).toBe(1_000);
  });

  test("an unanswered request is reported as unavailable with a structured problem", async () => {
    const h = harness({ replies: [null] });
    const result = await createRuleSync(h.deps).sync("manual");
    expect(result.outcome).toBe("unavailable");
    expect(h.metas[0].problems).toEqual([{ code: "unavailable" }]);
    expect(h.metas[0].lastSyncAt).toBeUndefined();
  });

  test("a desktop fetch error is its own outcome and is stored apart from the sync problems", async () => {
    const failing = reply({
      status: {
        lastFetchUnix: null,
        lastAttemptUnix: 5,
        lastError: "official.json: http error: status 404",
        errors: { official: "http error: status 404" },
        refreshing: false,
      },
    });
    const h = harness({ replies: [failing] });
    const result = await createRuleSync(h.deps).sync("manual");
    expect(result.outcome).toBe("fetch-failed");
    expect(h.metas[0].problems).toEqual([]);
    expect(h.metas[0].host?.lastError).toContain("404");
  });

  test("concurrent automatic syncs share one in-flight run", async () => {
    const h = harness({ hold: true });
    const sync = createRuleSync(h.deps);
    const first = sync.sync("auto");
    const second = sync.sync("auto");
    await tick();
    h.release();
    await Promise.all([first, second]);
    expect(h.calls).toHaveLength(1);
    expect(h.maxConcurrent()).toBe(1);
  });

  test("a manual sync arriving during an automatic one waits and then runs forced, never overlapping", async () => {
    const h = harness({ hold: true });
    const sync = createRuleSync(h.deps);
    const auto = sync.sync("auto");
    await tick();
    const manual = sync.sync("manual");
    await tick();
    expect(h.calls).toHaveLength(1);
    h.release();
    await auto;
    await tick();
    expect(h.calls).toHaveLength(2);
    expect(h.calls[1].options.force).toBe(true);
    h.release();
    await manual;
    expect(h.maxConcurrent()).toBe(1);
  });

  test("the plugin epoch is sent as part of have so the desktop can tell the sequence is stale", async () => {
    const h = harness({ stored: { plugin: pack("plugin", 4), pluginEpoch: "e1" } });
    await createRuleSync(h.deps).sync("auto");
    expect(h.calls[0].have.plugin).toBe(4);
    expect(h.calls[0].have.pluginEpoch).toBe("e1");
  });

  test("deferred documents are fetched in follow-up rounds while progress is made", async () => {
    const h = harness({
      replies: [
        reply({ revocations: revocations(1), deferred: ["official", "community"] }),
        reply({ community: pack("community", 2) }),
      ],
    });
    await createRuleSync(h.deps).sync("auto");
    expect(h.calls).toHaveLength(2);
    expect(h.calls[1].have.revocations).toBe(1);
    expect(h.saves.at(-1)?.community?.sequence).toBe(2);
  });

  test("a deferred reply that brings nothing new does not loop", async () => {
    const h = harness({ replies: [reply({ deferred: ["community"] })] });
    await createRuleSync(h.deps).sync("auto");
    expect(h.calls).toHaveLength(1);
  });

  test("an automatic sync that finds the desktop still refreshing schedules exactly one follow-up", async () => {
    const refreshing = () =>
      reply({
        status: { lastFetchUnix: null, lastAttemptUnix: null, lastError: null, errors: {}, refreshing: true },
      });
    const h = harness({ replies: [refreshing(), refreshing()] });
    const sync = createRuleSync(h.deps);
    await sync.sync("auto");
    await sync.sync("auto");
    expect(h.timers).toHaveLength(1);
    expect(h.timers[0].delayMs).toBe(FOLLOW_UP_DELAY_MS);
    h.timers[0].run();
    await tick();
    expect(h.calls).toHaveLength(3);
    // 补查本身不再续订补查。
    expect(h.timers).toHaveLength(1);
  });
});
