import { describe, expect, test } from "bun:test";
import type { SniffRuleV1 } from "@/rules/schema";
import {
  SNIFF_RULES_REPO,
  applyOverride,
  buildTestCase,
  communitySubmission,
  planImport,
  pruneOverrides,
} from "./local-rules";

const rule: SniffRuleV1 = {
  id: "local.example-video",
  version: 4,
  tier: "local",
  title: "Example video",
  stage: "response",
  match: { host: ["example.com"], ext: ["xyz"] },
  action: { kind: "resource", type: "video" },
};

describe("communitySubmission", () => {
  test("改写为社区规则并预填 Issue 表单", () => {
    const result = communitySubmission(rule);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.prefilled).toBe(true);
    const url = new URL(result.url);
    expect(`${url.origin}${url.pathname}`).toBe(`${SNIFF_RULES_REPO}/issues/new`);
    expect(url.searchParams.get("template")).toBe("community-rule.yml");
    expect(url.searchParams.get("title")).toBe("[规则] Example video");
    const submitted = JSON.parse(url.searchParams.get("rule") ?? "");
    expect(submitted).toEqual({ ...rule, id: "community.misc.example-video", tier: "community", version: 1 });
  });

  test("不满足社区层约束的规则不生成提交", () => {
    const result = communitySubmission({ ...rule, match: { ext: ["xyz"] } });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toBe("社区规则必须限定 host 或 pageHost");
  });

  test("规则过长时不预填，改由剪贴板携带", () => {
    const long = { ...rule, exclude: [{ url: Array.from({ length: 60 }, (_, i) => `https://example.com/${"x".repeat(120)}/${i}*`) }] };
    const result = communitySubmission(long);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.prefilled).toBe(false);
    expect(new URL(result.url).searchParams.has("rule")).toBe(false);
    expect(JSON.parse(result.json).id).toBe("community.misc.example-video");
  });
});

describe("communitySubmission 附加信息", () => {
  const tests = [
    buildTestCase(
      { url: "https://cdn.example.com/a.xyz", pageUrl: "https://example.com/v/1", size: -1, attachment: false },
      { decision: "resource", type: "video", rule: { id: "x", version: 1, tier: "local" } },
      "a",
    ),
  ];

  test("预填紧凑 JSON、示例页面与测试用例", () => {
    const result = communitySubmission(rule, { pageUrl: "https://example.com/v/1", tests });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const params = new URL(result.url).searchParams;
    expect(params.get("rule")).not.toContain("\n");
    expect(params.get("page")).toBe("https://example.com/v/1");
    expect(JSON.parse(params.get("tests") ?? "")).toEqual(tests);
  });

  test("示例页面只接受 http(s)", () => {
    const result = communitySubmission(rule, { pageUrl: "chrome://extensions" });
    expect(result.ok && new URL(result.url).searchParams.has("page")).toBe(false);
  });

  test("规则过长走剪贴板时仍预填示例页面", () => {
    const long = { ...rule, exclude: [{ url: Array.from({ length: 60 }, (_, i) => `https://example.com/${"x".repeat(120)}/${i}*`) }] };
    const result = communitySubmission(long, { pageUrl: "https://example.com/v/1" });
    expect(result.ok && !result.prefilled && new URL(result.url).searchParams.get("page")).toBe("https://example.com/v/1");
  });
});

describe("buildTestCase", () => {
  const input = { url: "https://x/a.mp4", pageUrl: "https://x/", size: 10, attachment: false, mime: "", resourceType: "" };
  test("资源决定带类型，忽略 / 未命中只记决定", () => {
    const ref = { id: "r", version: 1, tier: "local" as const };
    expect(buildTestCase(input, { decision: "resource", type: "video", rule: ref }, "n").expect).toEqual({
      decision: "resource",
      type: "video",
    });
    expect(buildTestCase(input, { decision: "ignore", rule: ref }, "n").expect).toEqual({ decision: "ignore" });
    expect(buildTestCase(input, { decision: "none" }, "n").expect).toEqual({ decision: "none" });
  });

  test("空的 mime / resourceType 不写入 args", () => {
    expect(buildTestCase(input, { decision: "none" }, "n").args[0]).toEqual({
      url: "https://x/a.mp4",
      pageUrl: "https://x/",
      size: 10,
      attachment: false,
    });
  });
});

describe("applyOverride", () => {
  test("与默认值一致时删除覆盖", () => {
    expect(applyOverride({ a: { enabled: false } }, "a", true, true)).toEqual({});
  });

  test("与默认值不同则写入", () => {
    expect(applyOverride({}, "a", false, true)).toEqual({ a: { enabled: false } });
  });

  test("默认关闭的规则被打开时写入", () => {
    expect(applyOverride({}, "a", true, false)).toEqual({ a: { enabled: true } });
  });

  test("保留撤回确认，即使开关与默认一致", () => {
    expect(applyOverride({ a: { enabled: true, ackRevocation: 3 } }, "a", true, true)).toEqual({
      a: { enabled: true, ackRevocation: 3 },
    });
  });

  test("不改动其它规则的覆盖", () => {
    expect(applyOverride({ b: { enabled: false } }, "a", false, true)).toEqual({ a: { enabled: false }, b: { enabled: false } });
  });
});

describe("pruneOverrides", () => {
  const overrides = { keep: { enabled: false }, gone: { enabled: true } };
  test("删除指向不存在规则的覆盖", () => {
    expect(pruneOverrides(overrides, new Set(["keep"]))).toEqual({ keep: { enabled: false } });
  });

  test("无孤儿时返回原对象（避免无谓写入）", () => {
    expect(pruneOverrides(overrides, new Set(["keep", "gone"]))).toBe(overrides);
  });
});

describe("planImport", () => {
  test("区分新增与覆盖", () => {
    const existing = [rule];
    const incoming = [rule, { ...rule, id: "local.other" }];
    expect(planImport(existing, incoming)).toEqual({ added: ["local.other"], replaced: ["local.example-video"] });
  });
});
