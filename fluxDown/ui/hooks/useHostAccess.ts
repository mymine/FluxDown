/**
 * 全站访问权限状态（popup / options 共用）。
 *
 * `granted === null` 表示尚未检测完；`false` 才应展示提示。权限变化（含在浏览器的附加组件页里手动
 * 授予 / 撤销）经 `permissions.onAdded / onRemoved` 实时同步，页面重新可见时再校正一次。
 */
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import { HOST_ACCESS_APPLICABLE, hasHostAccess, requestHostAccess } from "@/utils/host-access";

export interface HostAccess {
  granted: boolean | null;
  /** 在 click 回调中同步调用（用户手势内申请权限）。 */
  request: () => void;
  /** 最近一次申请被拒绝（用户取消或浏览器拒绝）。 */
  denied: boolean;
}

export function useHostAccess(): HostAccess {
  const [granted, setGranted] = useState<boolean | null>(HOST_ACCESS_APPLICABLE ? null : true);
  const [denied, setDenied] = useState(false);

  useEffect(() => {
    if (!HOST_ACCESS_APPLICABLE) return;
    let alive = true;
    const refresh = (): void => {
      void hasHostAccess().then((ok) => {
        if (!alive) return;
        setGranted(ok);
        if (ok) setDenied(false);
      });
    };
    const onChange = (permissions: { origins?: string[] }): void => {
      if (permissions.origins?.includes("<all_urls>")) refresh();
    };
    const onVisible = (): void => {
      if (document.visibilityState === "visible") refresh();
    };
    refresh();
    browser.permissions.onAdded?.addListener(onChange);
    browser.permissions.onRemoved?.addListener(onChange);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      alive = false;
      browser.permissions.onAdded?.removeListener(onChange);
      browser.permissions.onRemoved?.removeListener(onChange);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);

  const request = (): void => {
    void requestHostAccess().then((ok) => {
      setGranted(ok);
      setDenied(!ok);
    });
  };

  return { granted, request, denied };
}
