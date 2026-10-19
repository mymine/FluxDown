import { describe, expect, test } from "bun:test";
import type { SniffRuleV1 } from "@/rules/schema";
import { buildSearchText, type ListedRuleEntry, matchesQuery, toListItem } from "./rule-view";

const rule: SniffRuleV1 = {
  id: "community.misc.bili",
  version: 2,
  tier: "community",
  title: "哔哩哔哩",
  stage: "response",
  priority: 5,
  match: {
    host: ["bilivideo.com"],
    pageHost: ["bilibili.com"],
    ext: ["m4s"],
    mime: ["video/*"],
    url: ["*/upgcxcode/*"],
    pathRegex: "secret-body",
  },
  action: { kind: "resource", type: "video" },
};

describe("buildSearchText", () => {
  const text = buildSearchText(rule);
  test.each(["bilivideo.com", "bilibili.com", "m4s", "video/*", "upgcxcode", "哔哩", "community.misc.bili"])("包含 %s", (word) => {
    expect(matchesQuery(text, word.toLowerCase())).toBe(true);
  });

  test("不包含未列入的字段", () => {
    expect(matchesQuery(text, "secret-body")).toBe(false);
  });

  test("多个词需同时命中", () => {
    expect(matchesQuery(text, "m4s bilibili")).toBe(true);
    expect(matchesQuery(text, "m4s youtube")).toBe(false);
  });

  test("空查询命中全部", () => {
    expect(matchesQuery(text, "")).toBe(true);
  });
});

describe("toListItem", () => {
  const entry: ListedRuleEntry = { rule, status: "active" };

  test("精简字段，不带 match / action", () => {
    const item = toListItem(entry);
    expect(item.rule).toEqual({
      id: rule.id,
      title: rule.title,
      tier: "community",
      stage: "response",
      version: 2,
      priority: 5,
      enabled: undefined,
    });
    expect("match" in item.rule).toBe(false);
  });

  test("redact 时搜索文本不含 match 内容", () => {
    const item = toListItem(entry, undefined, true);
    expect(matchesQuery(item.searchText, "bilivideo")).toBe(false);
    expect(matchesQuery(item.searchText, "bili")).toBe(true);
  });

  test("携带命中统计", () => {
    expect(toListItem(entry, { count: 3, lastAt: 9 }).hits).toEqual({ count: 3, lastAt: 9 });
  });
});
