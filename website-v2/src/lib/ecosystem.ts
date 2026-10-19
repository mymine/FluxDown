/**
 * FluxDown 开源生态:各公开仓库 / 发布包坐标与「参与贡献」分区。
 * 页脚、文档首页、市场页与 Header 共用这一份,仓库地址只在这里出现一次。
 * 文案见 `@/i18n/messages/contribute`(按 `ContributeArea.id` 取)。
 */
import { BookOpenText, Languages, Palette, Puzzle, Radar, type LucideIcon } from "lucide-react";
import type { Lang } from "@/i18n/config";

export const githubUrl = (repo: string) => `https://github.com/${repo}`;

/** 主仓库:应用、引擎、官网与文档、插件 SDK 源码。 */
export const MAIN_REPO = "zerx-lab/FluxDown";
/** 插件市场索引(作者仓库登记表 + 生成的 index.json)。 */
export const PLUGIN_INDEX_REPO = "zerx-lab/fluxdown-plugin-index";
/** 主题市场(主题目录 + 生成的 index.json)。 */
export const THEMES_REPO = "zerx-lab/fluxdown-themes";
/** 浏览器扩展嗅探规则:官方规则 + 社区规则仓库登记表。 */
export const SNIFF_RULES_REPO = "zerx-lab/fluxdown-sniff-rules";

export const PLUGIN_SDK_PACKAGE = "fluxdown-plugin-sdk";
export const PLUGIN_SDK_NPM_URL = `https://www.npmjs.com/package/${PLUGIN_SDK_PACKAGE}`;
export const PLUGIN_SDK_SOURCE_URL = `${githubUrl(MAIN_REPO)}/tree/main/packages/plugin-sdk`;

export type ContributeId = "docs" | "translations" | "plugins" | "themes" | "sniffRules";

export interface ContributeArea {
  id: ContributeId;
  icon: LucideIcon;
  /** 贡献落地的 GitHub 仓库(owner/name) */
  repo: string;
  /** 文档中心内的贡献指南 slug(`/docs/<lang>/<slug>/`) */
  guide: string;
}

export const CONTRIBUTE_AREAS: readonly ContributeArea[] = [
  { id: "plugins", icon: Puzzle, repo: PLUGIN_INDEX_REPO, guide: "contributing/plugins" },
  { id: "themes", icon: Palette, repo: THEMES_REPO, guide: "contributing/themes" },
  { id: "sniffRules", icon: Radar, repo: SNIFF_RULES_REPO, guide: "contributing/sniff-rules" },
  { id: "translations", icon: Languages, repo: MAIN_REPO, guide: "contributing/translations" },
  { id: "docs", icon: BookOpenText, repo: MAIN_REPO, guide: "contributing/docs" },
];

/** 贡献总览页 slug。 */
export const CONTRIBUTE_OVERVIEW = "contributing/overview";

/** 文档内指南路径(不含挂载前缀;站内链接经 `withBase` / `href`)。 */
export const contributeGuidePath = (lang: Lang, slug: string) => `/docs/${lang}/${slug}/`;
