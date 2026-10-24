import type { JSX } from "preact";
import { useHostAccess } from "@/ui/hooks/useHostAccess";
import { useT } from "@/ui/i18n";
import { Button } from "./Button";
import "./components.css";

interface HostAccessNoticeProps {
  /** 放在已有内边距的内容区顶部：去掉左右外边距，只留底部间距。 */
  flush?: boolean;
}

/**
 * 全站访问权限被撤销时的提示条（仅 Firefox 构建会出现）：没有该权限，资源嗅探、
 * 下载拦截与页内面板都不会工作。「授予」按钮在点击回调里同步申请权限。
 */
export function HostAccessNotice(props: HostAccessNoticeProps): JSX.Element | null {
  const t = useT();
  const access = useHostAccess();
  if (access.granted !== false) return null;
  return (
    <div class={`fd-notice fd-notice-warning${props.flush ? " fd-notice-flush" : ""}`} role="alert">
      <div class="fd-notice-text">
        <strong>{t("hostAccess.title")}</strong>
        <span>{t("hostAccess.desc")}</span>
        {access.denied ? <span class="fd-notice-error">{t("hostAccess.denied")}</span> : null}
      </div>
      <Button variant="primary" size="sm" onClick={access.request}>
        {t("hostAccess.grant")}
      </Button>
    </div>
  );
}
