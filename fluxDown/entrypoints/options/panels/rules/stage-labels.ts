import type { RuleStage } from "@/rules/schema";
import { t } from "@/utils/i18n";

/**
 * 规则阶段显示名：列表、本地规则与编辑器共用，Record 保证新增 stage 时编译期补齐。
 * 在渲染中调用（组件已经 useT() 订阅语言切换）。
 */
export function stageLabels(): Record<RuleStage, string> {
  return {
    response: t("rules.stage.response"),
    normalize: t("rules.stage.normalize"),
    track: t("rules.stage.track"),
    body: t("rules.stage.body"),
    dom: t("rules.stage.dom"),
    page: t("rules.stage.page"),
    request: t("rules.stage.request"),
  };
}
