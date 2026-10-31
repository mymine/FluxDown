/**
 * GET /api/release
 *
 * 代理 GitHub Release API，安全获取私有仓库的最新发布信息。
 * 服务端持有 GITHUB_TOKEN，前端无需暴露凭据。
 *
 * 下载量计算：
 *   total_downloads = 历史基础下载量 + GitHub 当前仓库所有 release 的 asset download_count 之和。
 *   历史基础量（DOWNLOADS_BASELINE）来自不再统计的旧渠道：
 *     - 已归档旧仓库 zerx-lab/fluxdown-archive 全量 release 下载：55,318
 *     - Cloudflare R2 flux-down 桶下载（B 类操作累计 GET）：58,320
 *   两者相加固定为 113,638，叠加当前仓库分页拉取的全量真实下载数据。
 *
 * 返回格式:
 * {
 *   version: "1.0.0",
 *   published_at: "2025-01-01T00:00:00Z",
 *   total_downloads: 12345,
 *   extension_version: "0.2.3",   // 浏览器扩展版本（取自所在 release 的 tag），无扩展 release 时为 undefined
 *   extension_crx_version: "0.2.2", // 商店签名 CRX 的版本（审核通过后才补传，可能落后于 extension_version）
 *   assets: {
 *     setup: { name, size, download_url },
 *     portable: { name, size, download_url },
 *     extension: { name, size, download_url },        // Chrome 解压包 zip（开发者模式加载，不自动更新）
 *     firefox_extension: { name, size, download_url }, // Firefox XPI
 *     chrome_crx: { name, size, download_url },       // 商店签名 CRX 原件（.github/workflows/cws-crx.yml 补传）
 *   },
 *   server: {
 *     version: "0.1.51",
 *     tag: "v0.1.51",  // 统一 release 为 vX.Y.Z；历史拆分 release 为 server-vX.Y.Z
 *     assets: { windows_x64, windows_arm64, linux_x64, linux_arm64, macos_x64, macos_arm64,
 *               openwrt_x64, openwrt_arm64, openwrt_luci, openwrt_apk_x64, openwrt_apk_arm64,
 *               openwrt_apk_luci, qnap_x64, qnap_arm64,
 *               synology_dsm7_x64, synology_dsm7_arm64, synology_dsm6_x64, synology_dsm6_arm64,
 *               fnos_x64, fnos_arm64 }
 *   } | null,  // FluxDown Server（headless Web 版），无对应 release 时为 null
 *   cli: { version, tag, assets:{ windows_x64, windows_arm64, linux_x64, linux_arm64, macos_x64, macos_arm64 } } | null,
 *   mobile: { version, tag, assets:{ android_arm64, android_armv7, android_x64, android_universal } } | null,
 *   ios: { version, tag, assets:{ sideload_ipa } } | null  // 未签名侧载 IPA（full flavor）；商店版走 TestFlight，不在此列
 * }
 */

import type { APIRoute } from "astro";
import { GITHUB_TOKEN, GITHUB_REPO } from "astro:env/server";
import { getCached, setCached } from "../../lib/api-cache";
import { withBase } from "@/lib/base";
import { getClientIp } from "@/lib/client-ip";
import { markReleaseViewer, recordUpdateAssets } from "@/lib/download-gate";
import {
  desktopAssets,
  IOS_SIDELOAD_SUFFIX,
  pickComponentRelease,
  releaseVersion,
  type GitHubAsset,
  type GitHubRelease,
} from "@/lib/release-assets";

export const prerender = false;

// ── 缓存：避免每次请求都打 GitHub API（12 小时；release webhook 会主动清除）──
const CACHE_KEY = "release";
const CACHE_TTL = 12 * 60 * 60 * 1000;

// ── 历史基础下载量 ──
// 已归档旧仓库 zerx-lab/fluxdown-archive（55,318）+ 已停用的 Cloudflare R2
// flux-down 桶累计下载（B 类 GET 操作 58,320）。两个旧渠道均不再产生新增量，
// 因此作为固定基数叠加到当前仓库的动态下载量之上。
const DOWNLOADS_BASELINE = 55_318 + 58_320;

export const GET: APIRoute = async ({ url, request, clientAddress }) => {
  // 读过发布清单的访客才能拿 CDN 下载链接（见 download-gate.ts）。
  markReleaseViewer(getClientIp(request, clientAddress));
  // 渠道：缺省 stable（官网/存量客户端不带 channel 参数 → 永远稳定版）；
  // frontier 放行 prerelease，供客户端"预览版"更新通道使用。
  const channel =
    url.searchParams.get("channel") === "frontier" ? "frontier" : "stable";
  const includePrerelease = channel === "frontier";
  const cacheKey = `${CACHE_KEY}:${channel}`;

  // 检查缓存（按渠道分键，避免 stable/frontier 互相污染）
  const cached = getCached<unknown>(cacheKey, CACHE_TTL);
  if (cached) {
    return new Response(JSON.stringify(cached), {
      status: 200,
      headers: {
        "Content-Type": "application/json",
        "Cache-Control": "public, s-maxage=300, stale-while-revalidate=600",
      },
    });
  }

  if (!GITHUB_TOKEN) {
    return new Response(
      JSON.stringify({ error: "Server misconfigured: missing GITHUB_TOKEN" }),
      { status: 500, headers: { "Content-Type": "application/json" } },
    );
  }

  try {
    // 拉取全部 release（自动分页），用于计算总下载量
    const allReleases: GitHubRelease[] = [];
    let url: string | null =
      `https://api.github.com/repos/${GITHUB_REPO}/releases?per_page=100`;

    while (url) {
      const res: Response = await fetch(url, {
        headers: {
          Authorization: `Bearer ${GITHUB_TOKEN}`,
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
        },
      });

      if (!res.ok) {
        const text = await res.text();
        return new Response(
          JSON.stringify({
            error: `GitHub API error: ${res.status}`,
            detail: text,
          }),
          { status: 502, headers: { "Content-Type": "application/json" } },
        );
      }

      const page: GitHubRelease[] = await res.json();
      allReleases.push(...page);

      const link: string | null = res.headers.get("Link");
      const next: RegExpMatchArray | null =
        link?.match(/<([^>]+)>;\s*rel="next"/) ?? null;
      url = next ? next[1] : null;
    }

    const releases = allReleases;
    const published = releases.filter((r) => !r.draft && !r.prerelease);
    // 渠道感知候选池：frontier 放行 prerelease（含稳定版）。桌面客户端、
    // 移动端、服务器与 CLI 均按渠道选取（预发布 tag 会打包这四类组件，
    // 组件无改动时不重建 → frontier 取 SemVer 最大自然回落稳定版）；
    // 唯浏览器扩展恒取稳定版——商店版本号不可回退，预发布 tag 的扩展只是附在
    // release 上的预览包（Firefox 未签名），不推商店，也不作为官网下载。
    // 每个组件独立选取「完整包含该组件」的最新 release（统一 vX.Y.Z release
    // 或历史组件 release，见 lib/release-assets.ts）：某组件本次打包失败时
    // 自然回落到它上一个完整版本，不会拿到半套资产。
    const appPool = includePrerelease
      ? releases.filter((r) => !r.draft)
      : published;

    // 桌面客户端 release：frontier 取 SemVer 最大，防止旧线 hotfix 盖过 RC。
    const latest = pickComponentRelease(appPool, "app", includePrerelease);

    if (!latest) {
      return new Response(
        JSON.stringify({ error: "No published release found" }),
        { status: 404, headers: { "Content-Type": "application/json" } },
      );
    }

    const version = releaseVersion(latest.tag_name);
    // frontier 的 latest 是 prerelease，资产必须带 tag 定位（/api/download 的
    // 无 tag "最新"路径只认稳定版）；stable 保持无 tag（官网下载与旧行为一致）。
    const appTag = includePrerelease ? latest.tag_name : undefined;

    const extensionRelease = pickComponentRelease(published, "extension", false);
    const serverRelease = pickComponentRelease(appPool, "server", includePrerelease);
    const cliRelease = pickComponentRelease(appPool, "cli", includePrerelease);
    const mobileRelease = pickComponentRelease(appPool, "mobile", includePrerelease);
    const iosRelease = pickComponentRelease(appPool, "ios", includePrerelease);

    // 统一 release 同时含服务器/CLI 的同后缀压缩包，桌面资产只在过滤后的集合里匹配
    const appAssets = desktopAssets(latest);
    // 匹配资产文件（兼容旧命名：-windows-setup.exe / 新命名：-windows-x64-setup.exe）
    const setupAsset = appAssets.find(
      (a) =>
        a.name.endsWith("-windows-x64-setup.exe") ||
        a.name.endsWith("-windows-setup.exe"),
    );
    const portableAsset = appAssets.find(
      (a) =>
        a.name.endsWith("-windows-x64-portable.zip") ||
        a.name.endsWith("-windows-portable.zip"),
    );
    // ARM64 资产（仅新版 Release 包含）
    const setupArm64Asset = appAssets.find((a) =>
      a.name.endsWith("-windows-arm64-setup.exe"),
    );
    const portableArm64Asset = appAssets.find((a) =>
      a.name.endsWith("-windows-arm64-portable.zip"),
    );
    const extensionAsset = extensionRelease?.assets.find(
      (a) =>
        a.name.endsWith("-chrome.zip") || a.name.endsWith("-extension.zip"),
    );
    const firefoxExtensionAsset = extensionRelease?.assets.find((a) =>
      a.name.endsWith("-firefox.xpi"),
    );
    // 扩展版本号 = 所在 release 的版本（统一 vX.Y.Z / 历史 extension-vX.Y.Z / 更早的合并 release）
    const extensionVersion = extensionRelease
      ? releaseVersion(extensionRelease.tag_name)
      : undefined;
    // 商店签名 CRX 在审核通过后才由 cws-crx.yml 补传到同版本 release：取最近一个带它的
    // 稳定 release，不跟 extensionRelease 绑定（新版审核期间仍提供上一版，装好后由商店自动更新）。
    const crxRelease = published.find((r) =>
      r.assets.some((a) => a.name.endsWith("-chrome.crx")),
    );
    const chromeCrxAsset = crxRelease?.assets.find((a) =>
      a.name.endsWith("-chrome.crx"),
    );
    // macOS 资产
    const macosDmgArm64Asset = appAssets.find((a) =>
      a.name.endsWith("-macos-arm64.dmg"),
    );
    const macosDmgX64Asset = appAssets.find((a) =>
      a.name.endsWith("-macos-x64.dmg"),
    );
    const macosTarballArm64Asset = appAssets.find((a) =>
      a.name.endsWith("-macos-arm64.tar.gz"),
    );
    const macosTarballX64Asset = appAssets.find((a) =>
      a.name.endsWith("-macos-x64.tar.gz"),
    );
    // Linux 资产
    const linuxAppImageAsset = appAssets.find((a) =>
      a.name.endsWith("-linux-x64.AppImage"),
    );
    const linuxDebAsset = appAssets.find((a) =>
      a.name.endsWith("-linux-x64.deb"),
    );
    const linuxArchAsset = appAssets.find((a) =>
      a.name.endsWith("-linux-x64.pkg.tar.zst"),
    );
    const linuxTarballAsset = appAssets.find((a) =>
      a.name.endsWith("-linux-x64.tar.gz"),
    );
    // FluxDown Server 资产（命名：FluxDown-Server-<ver>-<os>-<arch>.<ext>）
    const findServerAsset = (suffix: string) =>
      serverRelease?.assets.find(
        (a) =>
          a.name.startsWith("FluxDown-Server-") && a.name.endsWith(suffix),
      );
    const serverWindowsX64Asset = findServerAsset("-windows-x64.zip");
    const serverWindowsArm64Asset = findServerAsset("-windows-arm64.zip");
    const serverLinuxX64Asset = findServerAsset("-linux-x64.tar.gz");
    const serverLinuxArm64Asset = findServerAsset("-linux-arm64.tar.gz");
    const serverMacosX64Asset = findServerAsset("-macos-x64.tar.gz");
    const serverMacosArm64Asset = findServerAsset("-macos-arm64.tar.gz");
    // OpenWrt（命名：fluxdown-server_<ver>_<arch>.<ipk|apk> / luci-app-fluxdown_<ver>_all.<ipk|apk>）；
    // ipk = 24.10 及更早（opkg），apk = 25.12 起（apk-tools v3）。
    // aarch64 有多个子架构标签的包，官网只挂 aarch64_generic，其余在 release 页可取
    const findOpenwrtAsset = (prefix: string, suffix: string) =>
      serverRelease?.assets.find((a) => a.name.startsWith(prefix) && a.name.endsWith(suffix));
    const serverOpenwrtX64Asset = findOpenwrtAsset("fluxdown-server_", "_x86_64.ipk");
    const serverOpenwrtArm64Asset = findOpenwrtAsset("fluxdown-server_", "_aarch64_generic.ipk");
    const serverOpenwrtLuciAsset = findOpenwrtAsset("luci-app-fluxdown_", "_all.ipk");
    const serverOpenwrtApkX64Asset = findOpenwrtAsset("fluxdown-server_", "_x86_64.apk");
    const serverOpenwrtApkArm64Asset = findOpenwrtAsset("fluxdown-server_", "_aarch64_generic.apk");
    const serverOpenwrtApkLuciAsset = findOpenwrtAsset("luci-app-fluxdown_", "_all.apk");
    // QNAP qpkg（命名：FluxDown-Server-<ver>-qnap-<arch>.qpkg）
    const serverQnapX64Asset = findServerAsset("-qnap-x64.qpkg");
    const serverQnapArm64Asset = findServerAsset("-qnap-arm64.qpkg");
    // 群晖 spk（命名：FluxDown-Server-<ver>-synology-<dsm6|dsm7>-<arch>.spk）
    const serverSynoDsm7X64Asset = findServerAsset("-synology-dsm7-x64.spk");
    const serverSynoDsm7Arm64Asset = findServerAsset("-synology-dsm7-arm64.spk");
    const serverSynoDsm6X64Asset = findServerAsset("-synology-dsm6-x64.spk");
    const serverSynoDsm6Arm64Asset = findServerAsset("-synology-dsm6-arm64.spk");
    // 飞牛 fnOS fpk（命名：FluxDown-Server-<ver>-fnos-<arch>.fpk）
    const serverFnosX64Asset = findServerAsset("-fnos-x64.fpk");
    const serverFnosArm64Asset = findServerAsset("-fnos-arm64.fpk");
    // FluxDown CLI 资产（命名：FluxDown-CLI-<ver>-<os>-<arch>.<ext>）
    const findCliAsset = (suffix: string) =>
      cliRelease?.assets.find(
        (a) => a.name.startsWith("FluxDown-CLI-") && a.name.endsWith(suffix),
      );
    const cliWindowsX64Asset = findCliAsset("-windows-x64.zip");
    const cliWindowsArm64Asset = findCliAsset("-windows-arm64.zip");
    const cliLinuxX64Asset = findCliAsset("-linux-x64.tar.gz");
    const cliLinuxArm64Asset = findCliAsset("-linux-arm64.tar.gz");
    const cliMacosX64Asset = findCliAsset("-macos-x64.tar.gz");
    const cliMacosArm64Asset = findCliAsset("-macos-arm64.tar.gz");
    // 移动端 Android 资产（命名：FluxDown-<ver>-android-<abi>.apk）
    const findMobileAsset = (suffix: string) =>
      mobileRelease?.assets.find(
        (a) => a.name.includes("-android-") && a.name.endsWith(suffix),
      );
    const mobileArm64Asset = findMobileAsset("-android-arm64-v8a.apk");
    const mobileArmv7Asset = findMobileAsset("-android-armeabi-v7a.apk");
    const mobileX64Asset = findMobileAsset("-android-x86_64.apk");
    const mobileUniversalAsset = findMobileAsset("-android-universal.apk");
    // iOS 侧载 IPA（命名：FluxDown-<ver>-ios-sideload-unsigned.ipa）
    const iosSideloadAsset = iosRelease?.assets.find((a) =>
      a.name.endsWith(IOS_SIDELOAD_SUFFIX),
    );

    const formatAsset = (asset: GitHubAsset | undefined, tag?: string) => {
      if (!asset) return null;
      return {
        name: asset.name,
        size: asset.size,
        github_download_url: asset.browser_download_url,
        // 保留旧客户端下载契约；官网显式 source=auto，备用入口使用 GitHub 直链。
        // 资产不在最新客户端 release 中时（如独立扩展 release）带 tag 定位
        download_url: tag
          ? withBase(`/api/download/${asset.name}?tag=${encodeURIComponent(tag)}`)
          : withBase(`/api/download/${asset.name}`),
      };
    };

    // ── 下载量计算 ──
    // 历史基础量（旧仓库 + R2，见 DOWNLOADS_BASELINE）叠加当前仓库
    // 所有 release 的 asset download_count（GitHub 全量真实下载数据）。
    let totalDownloads = DOWNLOADS_BASELINE;
    for (const release of releases) {
      for (const asset of release.assets) {
        totalDownloads += asset.download_count;
      }
    }

    const data = {
      version,
      tag: latest.tag_name,
      published_at: latest.published_at,
      total_downloads: totalDownloads,
      extension_version: extensionVersion,
      extension_crx_version: crxRelease
        ? releaseVersion(crxRelease.tag_name)
        : undefined,
      assets: {
        setup: formatAsset(setupAsset, appTag),
        portable: formatAsset(portableAsset, appTag),
        setup_arm64: formatAsset(setupArm64Asset, appTag),
        portable_arm64: formatAsset(portableArm64Asset, appTag),
        extension: formatAsset(extensionAsset, extensionRelease?.tag_name),
        firefox_extension: formatAsset(
          firefoxExtensionAsset,
          extensionRelease?.tag_name,
        ),
        chrome_crx: formatAsset(chromeCrxAsset, crxRelease?.tag_name),
        macos_dmg_arm64: formatAsset(macosDmgArm64Asset, appTag),
        macos_dmg_x64: formatAsset(macosDmgX64Asset, appTag),
        macos_tarball_arm64: formatAsset(macosTarballArm64Asset, appTag),
        macos_tarball_x64: formatAsset(macosTarballX64Asset, appTag),
        linux_appimage: formatAsset(linuxAppImageAsset, appTag),
        linux_deb: formatAsset(linuxDebAsset, appTag),
        linux_arch: formatAsset(linuxArchAsset, appTag),
        linux_tarball: formatAsset(linuxTarballAsset, appTag),
      },
      server: serverRelease
        ? {
            version: releaseVersion(serverRelease.tag_name),
            tag: serverRelease.tag_name,
            assets: {
              windows_x64: formatAsset(
                serverWindowsX64Asset,
                serverRelease.tag_name,
              ),
              windows_arm64: formatAsset(
                serverWindowsArm64Asset,
                serverRelease.tag_name,
              ),
              linux_x64: formatAsset(
                serverLinuxX64Asset,
                serverRelease.tag_name,
              ),
              linux_arm64: formatAsset(
                serverLinuxArm64Asset,
                serverRelease.tag_name,
              ),
              macos_x64: formatAsset(
                serverMacosX64Asset,
                serverRelease.tag_name,
              ),
              macos_arm64: formatAsset(
                serverMacosArm64Asset,
                serverRelease.tag_name,
              ),
              openwrt_x64: formatAsset(
                serverOpenwrtX64Asset,
                serverRelease.tag_name,
              ),
              openwrt_arm64: formatAsset(
                serverOpenwrtArm64Asset,
                serverRelease.tag_name,
              ),
              openwrt_luci: formatAsset(
                serverOpenwrtLuciAsset,
                serverRelease.tag_name,
              ),
              openwrt_apk_x64: formatAsset(
                serverOpenwrtApkX64Asset,
                serverRelease.tag_name,
              ),
              openwrt_apk_arm64: formatAsset(
                serverOpenwrtApkArm64Asset,
                serverRelease.tag_name,
              ),
              openwrt_apk_luci: formatAsset(
                serverOpenwrtApkLuciAsset,
                serverRelease.tag_name,
              ),
              qnap_x64: formatAsset(
                serverQnapX64Asset,
                serverRelease.tag_name,
              ),
              qnap_arm64: formatAsset(
                serverQnapArm64Asset,
                serverRelease.tag_name,
              ),
              synology_dsm7_x64: formatAsset(
                serverSynoDsm7X64Asset,
                serverRelease.tag_name,
              ),
              synology_dsm7_arm64: formatAsset(
                serverSynoDsm7Arm64Asset,
                serverRelease.tag_name,
              ),
              synology_dsm6_x64: formatAsset(
                serverSynoDsm6X64Asset,
                serverRelease.tag_name,
              ),
              synology_dsm6_arm64: formatAsset(
                serverSynoDsm6Arm64Asset,
                serverRelease.tag_name,
              ),
              fnos_x64: formatAsset(
                serverFnosX64Asset,
                serverRelease.tag_name,
              ),
              fnos_arm64: formatAsset(
                serverFnosArm64Asset,
                serverRelease.tag_name,
              ),
            },
          }
        : null,
      cli: cliRelease
        ? {
            version: releaseVersion(cliRelease.tag_name),
            tag: cliRelease.tag_name,
            assets: {
              windows_x64: formatAsset(cliWindowsX64Asset, cliRelease.tag_name),
              windows_arm64: formatAsset(
                cliWindowsArm64Asset,
                cliRelease.tag_name,
              ),
              linux_x64: formatAsset(cliLinuxX64Asset, cliRelease.tag_name),
              linux_arm64: formatAsset(cliLinuxArm64Asset, cliRelease.tag_name),
              macos_x64: formatAsset(cliMacosX64Asset, cliRelease.tag_name),
              macos_arm64: formatAsset(cliMacosArm64Asset, cliRelease.tag_name),
            },
          }
        : null,
      mobile: mobileRelease
        ? {
            version: releaseVersion(mobileRelease.tag_name),
            tag: mobileRelease.tag_name,
            assets: {
              android_arm64: formatAsset(mobileArm64Asset, mobileRelease.tag_name),
              android_armv7: formatAsset(mobileArmv7Asset, mobileRelease.tag_name),
              android_x64: formatAsset(mobileX64Asset, mobileRelease.tag_name),
              android_universal: formatAsset(
                mobileUniversalAsset,
                mobileRelease.tag_name,
              ),
            },
          }
        : null,
      ios: iosRelease
        ? {
            version: releaseVersion(iosRelease.tag_name),
            tag: iosRelease.tag_name,
            assets: {
              sideload_ipa: formatAsset(iosSideloadAsset, iosRelease.tag_name),
            },
          }
        : null,
    };

    // 应用内更新只取桌面 / 服务器 / Android 资产（native/agent/src/update/install）：只有当前清单里的
    // 这些资产能拿 123 直链，历史版本、扩展、CLI 与 iOS 侧载包不给。
    const updateEntries = (tag: string, assets: Record<string, { name: string } | null>, skip: string[] = []) =>
      Object.entries(assets).flatMap(([kind, asset]) => asset && !skip.includes(kind) ? [{ tag, name: asset.name }] : []);
    recordUpdateAssets(channel, [
      ...updateEntries(latest.tag_name, data.assets, ["extension", "firefox_extension", "chrome_crx"]),
      ...(serverRelease && data.server ? updateEntries(serverRelease.tag_name, data.server.assets) : []),
      ...(mobileRelease && data.mobile ? updateEntries(mobileRelease.tag_name, data.mobile.assets) : []),
    ]);

    // 更新缓存
    setCached(cacheKey, data);

    return new Response(JSON.stringify(data), {
      status: 200,
      headers: {
        "Content-Type": "application/json",
        "Cache-Control": "public, s-maxage=300, stale-while-revalidate=600",
      },
    });
  } catch (err) {
    return new Response(
      JSON.stringify({
        error: "Failed to fetch release info",
        detail: String(err),
      }),
      { status: 500, headers: { "Content-Type": "application/json" } },
    );
  }
};
