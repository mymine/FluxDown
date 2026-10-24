/**
 * 全站导航模型:Header 下拉、移动端菜单、Footer、命令面板共用这一份结构。
 * `label` / `desc` 是 `common.nav` 的键;路径为无语言前缀的规范路径,渲染时经 `navHref()` 本地化。
 */
import {
  Braces,
  HeartHandshake,
  Heart,
  History,
  Megaphone,
  MessageSquare,
  Palette,
  PenTool,
  Puzzle,
  Send,
  Sparkles,
  Users,
  Vote,
  type LucideIcon,
} from "lucide-react";
import type { common } from "@/i18n/messages/common";
import type { Lang } from "@/i18n/config";
import { CONTRIBUTE_OVERVIEW, MAIN_REPO, githubUrl } from "@/lib/ecosystem";
import { href } from "@/i18n/routing";

type NavKey = keyof (typeof common)["en"]["nav"];

export interface NavLink {
  label: NavKey;
  desc?: NavKey;
  /** 站点路径;`docs` 为真时是文档中心内的 slug 路径(`/contributing/overview`) */
  path: string;
  docs?: boolean;
  icon: LucideIcon;
}

export const GITHUB_URL = githubUrl(MAIN_REPO);
export const TELEGRAM_URL = "https://t.me/+tp8ie_FVjv02ZDNl";
export const TESTFLIGHT_URL = "https://testflight.apple.com/join/PqemSBgt";

export const PRODUCT_LINKS: NavLink[] = [
  { label: "features", desc: "featuresDesc", path: "/#features", icon: Sparkles },
  { label: "plugins", desc: "pluginsDesc", path: "/plugins", icon: Puzzle },
  { label: "themes", desc: "themesDesc", path: "/themes", icon: Palette },
  { label: "themeBuilder", desc: "themeBuilderDesc", path: "/theme-builder", icon: PenTool },
  { label: "apiDocs", desc: "apiDocsDesc", path: "/api-docs", icon: Braces },
  { label: "changelog", desc: "changelogDesc", path: "/changelog", icon: History },
];

export const COMMUNITY_LINKS: NavLink[] = [
  { label: "announcements", desc: "announcementsDesc", path: "/announcements", icon: Megaphone },
  { label: "feedback", desc: "feedbackDesc", path: "/feedback", icon: MessageSquare },
  { label: "featureVote", desc: "featureVoteDesc", path: "/feature-vote", icon: Vote },
  { label: "contribute", desc: "contributeDesc", path: `/${CONTRIBUTE_OVERVIEW}`, docs: true, icon: HeartHandshake },
  { label: "telegram", desc: "telegramDesc", path: "/telegram-group", icon: Send },
  { label: "qq", desc: "qqDesc", path: "/qq-group", icon: Users },
];

/** 赞助是一级导航(不进社区下拉),首页打开即可见。 */
export const SPONSOR_LINK: NavLink = { label: "sponsor", path: "/sponsor", icon: Heart };

/** 文档入口直接落到对应语言,避免 `/docs/` 的 302。 */
export const docsPath = (lang: Lang) => `/docs/${lang}/`;

/** 导航链接的本地化地址:文档链接落到当前语言的文档中心,其余经 `href()` 补语言前缀。 */
export const navHref = (link: NavLink, lang: Lang) => href(link.docs ? `/docs/${lang}${link.path}` : link.path, lang);
