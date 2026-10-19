/** 本地规则（storage.local `fluxdown_local_rules`）的解析、导入 / 导出打包与合并。纯函数。 */
import type { ResponseDecision, ResponseInput } from "@/rules/engine";
import { RULE_ENGINE_VERSION, type RulePack, type RuleOverrides, type SniffRuleV1 } from "@/rules/schema";
import { PackRejectedError, validatePack } from "@/rules/validate";
import { isRecord } from "./guards";

export const LOCAL_RULES_KEY = "fluxdown_local_rules";
export const RULE_OVERRIDES_KEY = "fluxdown_rule_overrides";

/** 宽松读取存储值：非数组 → []；只保留 tier=local 且带字符串 id 的对象（字段合法性由 validatePack 把关）。 */
export function parseLocalRules(raw: unknown): SniffRuleV1[] {
  if (!Array.isArray(raw)) return [];
  const rules: SniffRuleV1[] = [];
  for (const item of raw) {
    if (isRecord(item) && item.tier === "local" && typeof item.id === "string") {
      // 形状已按 tier / id 收窄，其余字段交给 validatePack。
      const rule = item as unknown as SniffRuleV1;
      rules.push(rule);
    }
  }
  return rules;
}

export function parseOverrides(raw: unknown): RuleOverrides {
  const out: RuleOverrides = {};
  if (!isRecord(raw)) return out;
  for (const [id, value] of Object.entries(raw)) {
    if (!isRecord(value) || typeof value.enabled !== "boolean") continue;
    const ack = value.ackRevocation;
    out[id] =
      typeof ack === "number" && Number.isInteger(ack)
        ? { enabled: value.enabled, ackRevocation: ack }
        : { enabled: value.enabled };
  }
  return out;
}

export function buildLocalPack(rules: SniffRuleV1[], now: number): RulePack {
  return {
    format: "fluxdown-sniff-pack",
    packId: "local",
    sequence: 0,
    engine: RULE_ENGINE_VERSION,
    issuedAt: new Date(now).toISOString(),
    rules,
  };
}

/** 被剔除的规则：`reason` 为原文，`code`/`param` 供 i18n 映射。 */
export interface RuleIssueRef {
  id: string;
  reason: string;
  code?: string;
  param?: string;
}

export type ImportParse =
  | { ok: true; accepted: SniffRuleV1[]; rejected: RuleIssueRef[] }
  | { ok: false; error: string };

/** 解析并校验导入文件（local 包）；整包不合格返回 error，单条不合格进 rejected。 */
export function parseLocalPack(text: string): ImportParse {
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
  try {
    // validatePack 自身对非对象 / 缺字段整包拒绝，入参形状在其内部检查。
    const pack = json as RulePack;
    const checked = validatePack(pack, "local");
    const rejected = checked.rejected.map((r) => ({ id: r.id, reason: r.reason, code: r.code, param: r.param }));
    return { ok: true, accepted: checked.pack.rules, rejected };
  } catch (e) {
    if (e instanceof PackRejectedError) return { ok: false, error: e.message };
    throw e;
  }
}

/** 导入前预览：哪些 id 会被覆盖、哪些是新增。 */
export function planImport(existing: SniffRuleV1[], accepted: SniffRuleV1[]): { added: string[]; replaced: string[] } {
  const known = new Set(existing.map((r) => r.id));
  const added: string[] = [];
  const replaced: string[] = [];
  for (const rule of accepted) (known.has(rule.id) ? replaced : added).push(rule.id);
  return { added, replaced };
}

/** 同 id 覆盖（version 取 max(导入版本, 原版本+1)，保证后台能感知变化），新 id 追加。 */
export function mergeImported(
  existing: SniffRuleV1[],
  accepted: SniffRuleV1[],
): { rules: SniffRuleV1[]; added: number; replaced: number } {
  const rules = [...existing];
  let added = 0;
  let replaced = 0;
  for (const incoming of accepted) {
    const at = rules.findIndex((r) => r.id === incoming.id);
    if (at === -1) {
      rules.push(incoming);
      added++;
    } else {
      rules[at] = { ...incoming, version: Math.max(incoming.version, rules[at].version + 1) };
      replaced++;
    }
  }
  return { rules, added, replaced };
}

/**
 * 写入启用开关覆盖：与规则默认值一致且没有「仍然启用」确认时直接删除覆盖，避免存储里堆积无意义条目。
 * `ruleDefault` = `rule.enabled !== false`。
 */
export function applyOverride(overrides: RuleOverrides, id: string, enabled: boolean, ruleDefault: boolean): RuleOverrides {
  const ack = overrides[id]?.ackRevocation;
  const { [id]: _previous, ...rest } = overrides;
  if (enabled === ruleDefault && ack === undefined) return rest;
  return { ...rest, [id]: ack === undefined ? { enabled } : { enabled, ackRevocation: ack } };
}

/** 删除指向已不存在规则的覆盖（例如上游已移除的社区规则）。无变化时返回原对象。 */
export function pruneOverrides(overrides: RuleOverrides, knownIds: ReadonlySet<string>): RuleOverrides {
  const orphans = Object.keys(overrides).filter((id) => !knownIds.has(id));
  if (orphans.length === 0) return overrides;
  const next: RuleOverrides = {};
  for (const [id, value] of Object.entries(overrides)) if (knownIds.has(id)) next[id] = value;
  return next;
}

/** 嗅探规则仓（官方 / 社区规则源，Issue 表单提交入口）。 */
export const SNIFF_RULES_REPO = "https://github.com/zerx-lab/fluxdown-sniff-rules";
/** GitHub 预填地址的安全长度；超出时规则改走剪贴板，由用户粘贴进表单。 */
const MAX_PREFILL_URL = 8000;

/** 规则仓测试用例（与 engine-cases 同格式，见规则仓 README「tests」）。 */
export interface RuleTestCase {
  name: string;
  call: "classifyResponse";
  args: [ResponseInput];
  expect: { decision: "resource"; type: string } | { decision: "ignore" } | { decision: "none" };
}

/** 把一次测试台运行的输入与结果固化成规则仓测试用例。 */
export function buildTestCase(input: ResponseInput, outcome: ResponseDecision, name: string): RuleTestCase {
  const args: ResponseInput = {
    url: input.url,
    pageUrl: input.pageUrl,
    size: input.size,
    attachment: input.attachment,
    ...(input.resourceType ? { resourceType: input.resourceType } : {}),
    ...(input.mime ? { mime: input.mime } : {}),
  };
  const expected: RuleTestCase["expect"] =
    outcome.decision === "resource" ? { decision: "resource", type: outcome.type } : { decision: outcome.decision };
  return { name, call: "classifyResponse", args: [args], expect: expected };
}

export interface SubmissionExtras {
  /** 规则生效的示例页面（来自测试台的页面地址）；仅 http(s) 会被预填。 */
  pageUrl?: string;
  /** 测试台里附加的测试用例。 */
  tests?: RuleTestCase[];
}

export type CommunitySubmission =
  | { ok: true; url: string; prefilled: boolean; json: string }
  | { ok: false; reason: string; code?: string; param?: string };

/** Issue 标题沿用表单模板的「[规则] 」前缀。 */
export const ISSUE_TITLE_PREFIX = "[规则] ";

function isHttpUrl(raw: string | undefined): raw is string {
  return raw !== undefined && /^https?:\/\//i.test(raw);
}

/**
 * 本地规则 → 社区规则提交：`local.x` 改写为 `community.misc.x`（规则仓托管的 misc 命名空间；
 * 长期维护者改为在自己的仓库发布并登记）、version 从 1 开始，按社区层校验（必须限定站点、
 * 不能用 preempt 等），再生成预填好的「提交社区规则」Issue 表单地址。
 * 预填使用紧凑 JSON 以节省 URL 长度；`json`（剪贴板兜底）保留缩进便于阅读。
 * 规则仓的机器人会做同样的改写与校验，这里提前拦下注定失败的提交。
 */
export function communitySubmission(rule: SniffRuleV1, extras: SubmissionExtras = {}): CommunitySubmission {
  const candidate: SniffRuleV1 = {
    ...rule,
    id: `community.misc.${rule.id.slice(rule.id.indexOf(".") + 1)}`,
    tier: "community",
    version: 1,
  };
  try {
    const { rejected } = validatePack({ ...buildLocalPack([candidate], 0), packId: "community" }, "community");
    if (rejected.length > 0) return { ok: false, reason: rejected[0].reason, code: rejected[0].code, param: rejected[0].param };
  } catch (e) {
    if (e instanceof PackRejectedError) return { ok: false, reason: e.message };
    throw e;
  }
  const json = JSON.stringify(candidate, null, 2);
  const base = new URLSearchParams({
    template: "community-rule.yml",
    title: `${ISSUE_TITLE_PREFIX}${rule.title || candidate.id}`,
  });
  const optional = new URLSearchParams();
  if (isHttpUrl(extras.pageUrl)) optional.set("page", extras.pageUrl);
  if (extras.tests && extras.tests.length > 0) optional.set("tests", JSON.stringify(extras.tests));
  const extra = optional.toString() ? `&${optional}` : "";
  const head = `${SNIFF_RULES_REPO}/issues/new?${base}`;
  const full = `${head}&${new URLSearchParams({ rule: JSON.stringify(candidate) })}${extra}`;
  if (full.length <= MAX_PREFILL_URL) return { ok: true, url: full, prefilled: true, json };
  // 过长：规则走剪贴板，其余字段（页面 / 测试）仍尽量预填。
  const fallback = `${head}${extra}`;
  return { ok: true, url: fallback.length <= MAX_PREFILL_URL ? fallback : head, prefilled: false, json };
}

/** 嗅探规则字段参考文档（随界面语言：中文 / 英文）。 */
export function sniffRulesDocsUrl(locale: string): string {
  return `https://fluxdown.com/docs/${locale.startsWith("zh") ? "zh" : "en"}/browser-extension/sniff-rules/`;
}
