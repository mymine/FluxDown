/**
 * `/api/release` 与 `/api/changelog` 的前端共享类型与格式化工具(下载页、更新日志页共用)。
 * 字段形状以 `src/pages/api/release.ts` / `changelog.ts` 的返回为准。
 */

export interface ReleaseAsset {
  name: string;
  size: number;
  download_url: string;
  github_download_url?: string;
}

type Assets<K extends string> = Record<K, ReleaseAsset | null>;

export interface ReleaseInfo {
  version: string;
  tag: string;
  published_at: string;
  total_downloads: number;
  extension_version?: string;
  extension_crx_version?: string;
  assets: Assets<
    | "setup"
    | "portable"
    | "setup_arm64"
    | "portable_arm64"
    | "extension"
    | "firefox_extension"
    | "chrome_crx"
    | "macos_dmg_arm64"
    | "macos_dmg_x64"
    | "macos_tarball_arm64"
    | "macos_tarball_x64"
    | "linux_appimage"
    | "linux_deb"
    | "linux_arch"
    | "linux_tarball"
  >;
  server: {
    version: string;
    tag: string;
    assets: Assets<
      | "windows_x64"
      | "windows_arm64"
      | "linux_x64"
      | "linux_arm64"
      | "macos_x64"
      | "macos_arm64"
      | "openwrt_x64"
      | "openwrt_arm64"
      | "openwrt_luci"
      | "openwrt_apk_x64"
      | "openwrt_apk_arm64"
      | "openwrt_apk_luci"
      | "qnap_x64"
      | "qnap_arm64"
      | "synology_dsm7_x64"
      | "synology_dsm7_arm64"
      | "synology_dsm6_x64"
      | "synology_dsm6_arm64"
      | "fnos_x64"
      | "fnos_arm64"
    >;
  } | null;
  cli: {
    version: string;
    tag: string;
    assets: Assets<"windows_x64" | "windows_arm64" | "linux_x64" | "linux_arm64" | "macos_x64" | "macos_arm64">;
  } | null;
  mobile: {
    version: string;
    tag: string;
    assets: Assets<"android_arm64" | "android_armv7" | "android_x64" | "android_universal">;
  } | null;
  ios: {
    version: string;
    tag: string;
    assets: Assets<"sideload_ipa">;
  } | null;
}

export interface ChangelogRelease {
  tag: string;
  version: string;
  published_at: string;
  body: string;
  prerelease?: boolean;
  assets: ReleaseAsset[];
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** 版本锚点:`v1.2.3` → `v1-2-3`,`v1.2.3-rc.1` → `v1-2-3-rc-1`(更新日志页的 id 与下载页的链接共用)。 */
export function versionAnchor(tagOrVersion: string): string {
  const bare = tagOrVersion.replace(/^v/i, "");
  return `v${bare.replace(/[^0-9a-z]+/gi, "-").toLowerCase()}`;
}

/** 按 `a.b.c` 路径从 release JSON 取值(下载页用 data-asset 声明资产位置)。 */
export function pickPath(data: unknown, path: string): unknown {
  let cur: unknown = data;
  for (const key of path.split(".")) {
    if (cur === null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[key];
  }
  return cur;
}
