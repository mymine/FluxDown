/** 规则校验问题：稳定代码 + 参数 → i18n（`rules.validate.<code>`），没有对应文案时回退到引擎给的原文。 */
import zhCN, { type MessageKey } from "@/utils/locales/zh-CN";
import type { t as Translate } from "@/utils/i18n";

export interface RuleIssue {
  /** validate.ts 的稳定 snake_case 代码。 */
  code?: string;
  param?: string;
  /** 引擎给出的原文（兜底，可能只有中文）。 */
  detail: string;
}

export function issueText(t: typeof Translate, issue: RuleIssue): string {
  if (issue.code) {
    const key = `rules.validate.${issue.code}`;
    if (key in zhCN) return t(key as MessageKey, { param: issue.param ?? "" });
  }
  return issue.detail;
}
