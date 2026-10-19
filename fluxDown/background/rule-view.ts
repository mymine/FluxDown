/**
 * 规则列表的精简视图：`rulesList` 只带展示 / 搜索所需字段，规则全文按需经 `ruleGet` 获取。
 * 纯函数，无副作用。
 */
import type { RuleListEntry, RuleStatus } from "@/rules/engine";
import type { SniffRuleV1 } from "@/rules/schema";

/** 命中统计（background 内存 + storage.session，见 rule-hits.ts）。 */
export interface RuleHit {
  count: number;
  lastAt: number;
}

export type RulesListRule = Pick<SniffRuleV1, "id" | "title" | "tier" | "stage" | "version" | "priority" | "enabled">;

/** 吊销是内部下架机制，不对用户展示：被吊销的规则直接从列表消失，等同下架。 */
export type ListedRuleStatus = Exclude<RuleStatus, "soft-revoked" | "hard-revoked">;
export type ListedRuleEntry = RuleListEntry & { status: ListedRuleStatus };

export function isListed(entry: RuleListEntry): entry is ListedRuleEntry {
  return entry.status !== "soft-revoked" && entry.status !== "hard-revoked";
}

export type RulesListItem = Omit<ListedRuleEntry, "rule"> & {
  rule: RulesListRule;
  /** 小写、预拼接的可搜索文本（id / 标题 / host / pageHost / ext / mime / url）。 */
  searchText: string;
  hits?: RuleHit;
};

/** 规则可搜索文本：id、标题与 match 里的 host / pageHost / ext / mime / url。 */
export function buildSearchText(rule: SniffRuleV1): string {
  const m = rule.match;
  const parts: string[] = [rule.id, rule.title ?? ""];
  if (m) {
    for (const list of [m.host, m.pageHost, m.ext, m.mime, m.url]) {
      if (Array.isArray(list)) for (const v of list) if (typeof v === "string") parts.push(v);
    }
  }
  return parts.join("\n").toLowerCase();
}

/** 空白分词，所有词都需出现（AND）。`needle` 需已小写。 */
export function matchesQuery(searchText: string, needle: string): boolean {
  const words = needle.split(/\s+/).filter(Boolean);
  return words.every((w) => searchText.includes(w));
}

/** `redactBody` = 发布构建里的内置规则：搜索文本只含 id / 标题，不泄露 match 内容。 */
export function toListItem(entry: ListedRuleEntry, hit?: RuleHit, redactBody = false): RulesListItem {
  const { rule, ...rest } = entry;
  return {
    ...rest,
    rule: {
      id: rule.id,
      title: rule.title,
      tier: rule.tier,
      stage: rule.stage,
      version: rule.version,
      priority: rule.priority,
      enabled: rule.enabled,
    },
    searchText: redactBody ? `${rule.id}\n${rule.title ?? ""}`.toLowerCase() : buildSearchText(rule),
    ...(hit ? { hits: hit } : {}),
  };
}
