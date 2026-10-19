import { describe, expect, test } from "bun:test";
import type { SniffRuleV1 } from "@/rules/schema";
import {
  emptyForm,
  finalizeLocalRule,
  formToRule,
  generateRuleId,
  normalizeHostPattern,
  ruleToForm,
} from "./rule-form";
import { parseHash } from "./route";

const NOW = Date.UTC(2026, 9, 15);

describe("generateRuleId", () => {
  test("slug 仅含 [a-z0-9-]，时间戳为 base36", () => {
    expect(generateRuleId("My Site: Video!", NOW)).toBe(`local.my-site-video-${NOW.toString(36)}`);
  });

  test("纯中文标题回落为 rule", () => {
    expect(generateRuleId("视频", NOW)).toBe(`local.rule-${NOW.toString(36)}`);
    expect(generateRuleId("", NOW)).toBe(`local.rule-${NOW.toString(36)}`);
  });
});

describe("normalizeHostPattern", () => {
  test("保留 = 前缀并归一化域名", () => {
    expect(normalizeHostPattern("=Example.COM")).toBe("=example.com");
    expect(normalizeHostPattern("https://Cdn.Example.com/x")).toBe("cdn.example.com");
    expect(normalizeHostPattern("   ")).toBe("");
  });
});

describe("formToRule / ruleToForm", () => {
  test("response 规则：MB 转字节，往返一致", () => {
    const form = { ...emptyForm(), title: "t", host: ["example.com"], ext: ["mp4"], minSizeMB: "1.5" };
    const result = formToRule(form, { id: "local.t-1", version: 1 });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.rule.match).toEqual({ host: ["example.com"], ext: ["mp4"], minSize: 1572864 });
    expect(result.rule.action).toEqual({ kind: "resource", type: "video" });
    expect(ruleToForm(result.rule)).toEqual(form);
  });

  test("priority / exclude 往返一致", () => {
    const form = { ...emptyForm(), host: ["example.com"], priority: "-3", excludeHost: ["ads.example.com"], excludeUrl: ["*/ad/*"] };
    const result = formToRule(form, { id: "local.t-1", version: 1 });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.rule.priority).toBe(-3);
    expect(result.rule.exclude).toEqual([{ host: ["ads.example.com"] }, { url: ["*/ad/*"] }]);
    expect(ruleToForm(result.rule)).toEqual(form);
  });

  test("priority 非整数时拒绝", () => {
    const form = { ...emptyForm(), host: ["example.com"], priority: "1.5" };
    expect(formToRule(form, { id: "local.t-1", version: 1 })).toEqual({ ok: false, errorKey: "rules.form.errPriority" });
  });

  test("多键 exclude 条目只能走 JSON", () => {
    const form = { ...emptyForm(), host: ["example.com"] };
    const result = formToRule(form, { id: "local.t-1", version: 1 });
    if (!result.ok) throw new Error("form should be valid");
    expect(ruleToForm({ ...result.rule, exclude: [{ host: ["a.com"], ext: ["js"] }] })).toBeNull();
  });

  test("没有任何匹配条件时拒绝", () => {
    const result = formToRule(emptyForm(), { id: "local.x-1", version: 1 });
    expect(result).toEqual({ ok: false, errorKey: "rules.form.errNoMatch" });
  });

  test("最小值大于最大值时拒绝", () => {
    const form = { ...emptyForm(), host: ["a.com"], minSizeMB: "5", maxSizeMB: "1" };
    expect(formToRule(form, { id: "local.x-1", version: 1 })).toEqual({ ok: false, errorKey: "rules.form.errSize" });
  });

  test("normalize 规则必须至少有一个参数", () => {
    const form = { ...emptyForm(), stage: "normalize" as const, action: "filename" as const, host: ["a.com"] };
    expect(formToRule(form, { id: "local.x-1", version: 1 })).toEqual({ ok: false, errorKey: "rules.form.errParams" });
    const ok = formToRule({ ...form, params: ["format"] }, { id: "local.x-1", version: 1 });
    expect(ok.ok && ok.rule.action).toEqual({ kind: "filename", extFromQuery: ["format"] });
  });

  test("含高级字段或 body / dom 的规则无法用表单表示", () => {
    const base: SniffRuleV1 = {
      id: "local.a-1",
      version: 1,
      tier: "local",
      stage: "response",
      match: { host: ["a.com"], pathRegex: "/v/" },
      action: { kind: "ignore" },
    };
    expect(ruleToForm(base)).toBeNull();
    expect(
      ruleToForm({ ...base, match: { host: ["a.com"] }, stage: "dom", action: { kind: "dom", selector: "a", attr: "href", emit: { mode: "resource", url: "${value}" } } }),
    ).toBeNull();
  });
});

describe("finalizeLocalRule", () => {
  const draft = {
    title: "demo",
    stage: "response",
    match: { host: ["example.com"] },
    action: { kind: "ignore" },
  };

  test("新建：tier 固定 local、version=1、自动生成 id", () => {
    const result = finalizeLocalRule({ ...draft, tier: "official" }, { editing: null, takenIds: new Set(), now: NOW });
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.rule.tier).toBe("local");
    expect(result.rule.version).toBe(1);
    expect(result.rule.id).toBe(`local.demo-${NOW.toString(36)}`);
  });

  test("编辑：沿用原 id，version+1，忽略草稿里的 id", () => {
    const editing: SniffRuleV1 = { ...(draft as unknown as SniffRuleV1), id: "local.demo-abc", version: 3, tier: "local" };
    const result = finalizeLocalRule({ ...draft, id: "local.other-1" }, { editing, takenIds: new Set(["local.demo-abc"]), now: NOW });
    expect(result.ok && [result.rule.id, result.rule.version]).toEqual(["local.demo-abc", 4]);
  });

  test("草稿里合法且未占用的 local id 在新建时保留；被占用则重新生成", () => {
    const kept = finalizeLocalRule({ ...draft, id: "local.mine-1" }, { editing: null, takenIds: new Set(), now: NOW });
    expect(kept.ok && kept.rule.id).toBe("local.mine-1");
    const regenerated = finalizeLocalRule({ ...draft, id: "local.mine-1" }, { editing: null, takenIds: new Set(["local.mine-1"]), now: NOW });
    expect(regenerated.ok && regenerated.rule.id).toBe(`local.demo-${NOW.toString(36)}`);
  });

  test("校验失败返回逐条错误", () => {
    const result = finalizeLocalRule(
      { ...draft, match: { pathRegex: "(a+)+" } },
      { editing: null, takenIds: new Set(), now: NOW },
    );
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.errors.length).toBeGreaterThan(0);
  });

  test("非对象草稿被拒绝", () => {
    expect(finalizeLocalRule(null, { editing: null, takenIds: new Set(), now: NOW }).ok).toBe(false);
    expect(finalizeLocalRule([], { editing: null, takenIds: new Set(), now: NOW }).ok).toBe(false);
  });
});

describe("parseHash", () => {
  test("面板与查询参数", () => {
    const route = parseHash("#sites?host=example.com");
    expect(route.panel).toBe("sites");
    expect(route.params.get("host")).toBe("example.com");
  });

  test("无效或为空回落 general", () => {
    expect(parseHash("").panel).toBe("general");
    expect(parseHash("#nope").panel).toBe("general");
  });
});
