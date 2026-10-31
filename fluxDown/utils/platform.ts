/**
 * 运行平台事实：Android 版浏览器（Firefox for Android、Chromium 系 Android 分支）没有 Native Messaging，
 * 本机通道改为经 Android Intent 唤起 FluxDown App（见 background/android-handoff.ts）；
 * Firefox for Android 79+ 另外整个移除了 downloads 命名空间，commands / menus 等也不存在。
 */
import { browser } from "wxt/browser";
import { logWarn } from "./error-log";

let androidProbe: Promise<boolean> | null = null;
let androidKnown: boolean | null = null;

/** 当前浏览器是否运行在 Android 上（`runtime.getPlatformInfo().os`，结果进程内缓存）。 */
export function isAndroidPlatform(): Promise<boolean> {
  androidProbe ??= browser.runtime
    .getPlatformInfo()
    .then((info) => info.os === "android")
    .catch((e: unknown) => {
      logWarn("[FluxDown] getPlatformInfo failed, assuming desktop:", e);
      return false;
    })
    .then((android) => {
      androidKnown = android;
      return android;
    });
  return androidProbe;
}

/** 同步读取已完成的探测结果；尚未探测完成时为 null（调用方按桌面处理）。 */
export function peekAndroidPlatform(): boolean | null {
  return androidKnown;
}

/** 浏览器是否提供 downloads API（Firefox for Android 79+ 没有该命名空间）。 */
export function hasDownloadsApi(): boolean {
  return "downloads" in browser && browser.downloads !== undefined;
}

/** 浏览器是否提供 commands API（移动端浏览器没有快捷键）。 */
export function hasCommandsApi(): boolean {
  return "commands" in browser && browser.commands !== undefined;
}

/** 浏览器是否提供右键菜单 API（Firefox for Android 没有 menus / contextMenus）。 */
export function hasContextMenusApi(): boolean {
  return "contextMenus" in browser && browser.contextMenus !== undefined;
}
