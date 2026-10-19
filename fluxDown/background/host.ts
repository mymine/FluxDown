/**
 * 桌面端（NMH → agent）能力探测与协议 v2 请求封装。
 *
 * 协议 1（旧桌面端）：`capabilities` 返回 `unknown action`，此时不做规则同步、
 * 不跟踪 capture、不判断解析插件。远程 always 模式不走 NMH，直接按协议 1 处理。
 */
import type { RevocationDoc, RulePack } from "@/rules/schema";
import {
  checkFluxDownAvailable,
  effectiveRemoteMode,
  isUnreachableFailure,
} from "@/utils/download-dispatch";
import { nmhCall } from "@/utils/native-messaging";
import type { CaptureState } from "./capture-logic";
import type { HostCapabilities, HostSyncStatus, ResolverInfo } from "./protocol";

const CAPABILITIES_TTL_MS = 60_000;
/** 未连接时的结果只缓存很短时间，App 一启动就能被感知。 */
const DISCONNECTED_TTL_MS = 5_000;
/** capture_status 单次请求的 id 数量上限（桌面端同口径）。 */
const CAPTURE_STATUS_MAX_IDS = 256;

export interface SniffRulesHave {
  official?: number;
  community?: number;
  plugin?: number;
  /** 持有的 plugin 包所属 epoch；与桌面端当前 epoch 不同则桌面端无视 `plugin` 序号。 */
  pluginEpoch?: string;
  revocations?: number;
}

export interface SniffRulesReply {
  official: RulePack | null;
  community: RulePack | null;
  revocations: RevocationDoc | null;
  plugin: RulePack | null;
  /** 桌面端 plugin 层当前序号 / epoch；桌面端未答复 plugin 查询时为 null（此时不得改动 plugin 层）。 */
  pluginSequence: number | null;
  pluginEpoch: string | null;
  /** 因应答体积上限本轮被推迟的文档（置 null）；处理完已收到的部分后再次查询。 */
  deferred?: Array<"official" | "community" | "revocations" | "plugin">;
  /** `pluginOnly` 请求时为 null。 */
  status: HostSyncStatus | null;
}

interface CapabilitiesData {
  protocol?: number;
  pluginsSupported?: boolean;
  resolvers?: ResolverInfo[];
  pluginRulesSequence?: number;
  pluginRulesEpoch?: string | null;
}

const PROTOCOL_ONE_OFFLINE: HostCapabilities = {
  protocol: 1,
  pluginsSupported: false,
  resolvers: [],
  connected: false,
  pluginRulesSequence: 0,
};

let cached: { value: HostCapabilities; expiresAt: number } | null = null;
let inflight: Promise<HostCapabilities> | null = null;
const capabilityListeners: Array<(caps: HostCapabilities) => void> = [];

/** 每次探测到新能力后回调（rules-store 据此发现插件规则序号变化；避免 host ↔ rules-store 循环依赖）。 */
export function onCapabilities(listener: (caps: HostCapabilities) => void): void {
  capabilityListeners.push(listener);
}

function isUnknownAction(message: string | undefined): boolean {
  return typeof message === "string" && message.startsWith("unknown action");
}

async function probeCapabilities(): Promise<HostCapabilities> {
  if ((await effectiveRemoteMode()) === "always") {
    const connected = await checkFluxDownAvailable().catch(() => false);
    return { ...PROTOCOL_ONE_OFFLINE, connected };
  }

  const reply = await nmhCall<CapabilitiesData>("capabilities", {});
  if (reply.success) {
    const data = reply.data ?? {};
    const caps: HostCapabilities = {
      protocol: data.protocol !== undefined && data.protocol >= 2 ? 2 : 1,
      pluginsSupported: data.pluginsSupported === true,
      resolvers: Array.isArray(data.resolvers) ? data.resolvers : [],
      connected: true,
      pluginRulesSequence:
        typeof data.pluginRulesSequence === "number" && data.pluginRulesSequence > 0
          ? data.pluginRulesSequence
          : 0,
      pluginRulesEpoch:
        typeof data.pluginRulesEpoch === "string" && data.pluginRulesEpoch !== ""
          ? data.pluginRulesEpoch
          : null,
    };
    return caps;
  }
  if (isUnknownAction(reply.message)) {
    // 旧桌面端：已连通，但不认识 v2 动作。
    return { ...PROTOCOL_ONE_OFFLINE, connected: true };
  }
  if (isUnreachableFailure({ success: false, message: reply.message })) {
    return PROTOCOL_ONE_OFFLINE;
  }
  // 桌面端 v2 但 daemon 侧失败（如 plugin.list 报错）：已连接，解析能力未知。
  return { protocol: 2, pluginsSupported: false, resolvers: [], connected: true, pluginRulesSequence: 0 };
}

/** 取桌面端能力（缓存 60 s；`force` 跳过缓存）。 */
export function getCapabilities(force = false): Promise<HostCapabilities> {
  const now = Date.now();
  if (!force && cached && cached.expiresAt > now) return Promise.resolve(cached.value);
  if (inflight) return inflight;
  inflight = probeCapabilities()
    .catch(() => PROTOCOL_ONE_OFFLINE)
    .then((value) => {
      cached = {
        value,
        expiresAt: Date.now() + (value.connected ? CAPABILITIES_TTL_MS : DISCONNECTED_TTL_MS),
      };
      for (const listener of capabilityListeners) {
        try {
          listener(value);
        } catch (e) {
          console.warn("[FluxDown] capabilities listener failed:", e);
        }
      }
      return value;
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

/** 供 App 已确认可达（ping 成功）时使旧的「未连接」缓存立刻失效。 */
export function invalidateCapabilities(): void {
  cached = null;
}

/** 查询 capture 事务状态；任何失败返回空表（调用方视为「暂无结论」）。 */
export async function captureStatus(ids: string[]): Promise<Record<string, CaptureState>> {
  const out: Record<string, CaptureState> = {};
  for (let i = 0; i < ids.length; i += CAPTURE_STATUS_MAX_IDS) {
    const chunk = ids.slice(i, i + CAPTURE_STATUS_MAX_IDS);
    const reply = await nmhCall<{ statuses?: Record<string, CaptureState> }>("capture_status", {
      transactionIds: chunk,
    });
    if (!reply.success) continue;
    Object.assign(out, reply.data?.statuses ?? {});
  }
  return out;
}

/** `force` 时桌面端最多等 15 s 刷新完成，请求超时必须更长。 */
const FORCED_SNIFF_RULES_TIMEOUT_MS = 20_000;

export interface SniffRulesOptions {
  /** 用户手动同步：忽略桌面端退避并等它拉取完成再应答。 */
  force?: boolean;
  /** 只取 plugin 层（本机聚合，桌面端不会联网拉远程包）。 */
  pluginOnly?: boolean;
}

/** 向桌面端拉取规则包增量；失败返回 null。 */
export async function fetchSniffRules(
  have: SniffRulesHave,
  options: SniffRulesOptions = {},
): Promise<SniffRulesReply | null> {
  const reply = await nmhCall<SniffRulesReply>(
    "sniff_rules",
    { have, force: options.force === true, pluginOnly: options.pluginOnly === true },
    options.force ? FORCED_SNIFF_RULES_TIMEOUT_MS : undefined,
  );
  if (!reply.success || !reply.data) return null;
  return reply.data;
}
