/**
 * 内置规则是否可查看原文。开发与本地构建默认可看；发布流水线（release.yml）构建时
 * 设 `WXT_HIDE_BUILTIN_RULES=1`，商店 / Release 包不展示内置规则的真实内容。
 * background 的 `ruleGet` 同样按此拒绝，保证隐藏不只是界面层面的。
 */
export const BUILTIN_RULE_PREVIEW = import.meta.env.WXT_HIDE_BUILTIN_RULES !== "1";
