/**
 * 扩展页面（popup / options / 侧边栏）按运行平台裁剪入口。
 *
 * Android 版浏览器没有 Native Messaging（任务面板、页面脚本、交给桌面解析、规则同步等依赖桌面 App 的功能
 * 都不可用），Firefox for Android 另缺 commands / contextMenus。内容脚本拿不到 `runtime.getPlatformInfo`，
 * 那边按 `(pointer: coarse)` 媒体查询适配触屏，不用本 hook。
 */
import { useEffect, useState } from "preact/hooks";
import { hasCommandsApi, hasContextMenusApi, isAndroidPlatform, peekAndroidPlatform } from "@/utils/platform";

export interface PlatformFeatures {
  /** 平台探测已完成。未完成时 `desktopApp` 为 false：依赖桌面 App 的入口先不渲染，避免在手机上闪现。 */
  ready: boolean;
  /** 运行在 Android 版浏览器。 */
  android: boolean;
  /** 本机桌面 App 通道（NMH）可能存在：探测完成且不是 Android。 */
  desktopApp: boolean;
  /** 浏览器有快捷键 API。 */
  shortcuts: boolean;
  /** 浏览器有右键菜单 API。 */
  contextMenus: boolean;
}

function featuresOf(android: boolean | null): PlatformFeatures {
  return {
    ready: android !== null,
    android: android === true,
    desktopApp: android === false,
    shortcuts: hasCommandsApi(),
    contextMenus: hasContextMenusApi(),
  };
}

export function usePlatform(): PlatformFeatures {
  const [android, setAndroid] = useState<boolean | null>(peekAndroidPlatform);
  useEffect(() => {
    if (android !== null) return;
    let alive = true;
    void isAndroidPlatform().then((value) => {
      if (alive) setAndroid(value);
    });
    return () => {
      alive = false;
    };
  }, [android]);
  return featuresOf(android);
}
