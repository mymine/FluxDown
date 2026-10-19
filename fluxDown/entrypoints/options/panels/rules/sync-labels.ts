import type { RuleSyncOutcome, RuleSyncProblem } from "@/background/protocol";
import { t } from "@/utils/i18n";

type Tier = NonNullable<RuleSyncProblem["tier"]>;

function tierLabel(tier: Tier | undefined): string {
  switch (tier) {
    case "official":
      return t("rules.sync.official");
    case "community":
      return t("rules.sync.community");
    case "plugin":
      return t("rules.sync.plugin");
    default:
      return "";
  }
}

/** 吊销列表是内部下架机制，相关问题不展示给用户。 */
export function userSyncProblems(problems: RuleSyncProblem[]): RuleSyncProblem[] {
  return problems.filter((problem) => problem.code !== "revocations_invalid" && problem.tier !== "revocations");
}

/**
 * 同步问题的本地化文案：选项页与诊断页共用，调用方先经 `userSyncProblems` 过滤。`code` 是稳定契约，`detail` 仅作补充。
 * 在渲染中调用（组件已经 useT() 订阅语言切换）。
 */
export function syncProblemLabel(problem: RuleSyncProblem): string {
  const tier = tierLabel(problem.tier);
  switch (problem.code) {
    case "unavailable":
      return t("rules.sync.problem.unavailable");
    case "rollback":
      return t("rules.sync.problem.rollback", { tier });
    case "pack_invalid":
      return t("rules.sync.problem.packInvalid", { tier, detail: problem.detail ?? "" });
    case "rules_rejected":
      return t("rules.sync.problem.rulesRejected", { tier, count: String(problem.count ?? 0) });
    case "revocations_invalid":
      return "";
  }
}

/** 同步结论的文案与 toast 类型；Record 保证新增结论时编译期补齐。 */
export function syncOutcomeLabels(): Record<RuleSyncOutcome, [string, "success" | "error" | "info"]> {
  return {
    ok: [t("rules.sync.done"), "success"],
    partial: [t("rules.sync.partial"), "error"],
    "fetch-failed": [t("rules.sync.fetchFailed"), "error"],
    disabled: [t("rules.sync.disabledHint"), "info"],
    offline: [t("rules.sync.offline"), "error"],
    unavailable: [t("rules.sync.unavailable"), "error"],
  };
}
