import { describe, expect, test } from "bun:test";
import { issueText } from "./rule-issue";

// 假翻译函数：回显 key 与参数，便于断言映射路径。
const fakeT = ((key: string, params?: Record<string, string>) => `${key}|${params?.param ?? ""}`) as Parameters<typeof issueText>[0];

describe("issueText", () => {
  test("已知 code 走 i18n 并带参数", () => {
    expect(issueText(fakeT, { code: "unsafe_regex", param: "pathRegex", detail: "原文" })).toBe("rules.validate.unsafe_regex|pathRegex");
  });

  test("未知 code 回退到原文", () => {
    expect(issueText(fakeT, { code: "no_such_code", detail: "原文" })).toBe("原文");
  });

  test("没有 code 回退到原文", () => {
    expect(issueText(fakeT, { detail: "invalid rule" })).toBe("invalid rule");
  });
});
