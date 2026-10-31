/**
 * /api/release 组件选取：统一 vX.Y.Z release（带 SHA256SUMS-<组件>.txt 哨兵）与历史拆分
 * release（server-v* / mobile-v* …，无哨兵）并存时，每个组件都要拿到完整且最新的一套资产。
 */
import { describe, expect, test } from "bun:test";
import {
  desktopAssets,
  pickComponentRelease,
  stripReleaseHeader,
  type GitHubRelease,
} from "../src/lib/release-assets";

function release(tag: string, names: string[], prerelease = false): GitHubRelease {
  return {
    tag_name: tag,
    name: tag,
    published_at: "2026-09-29T00:00:00Z",
    draft: false,
    prerelease,
    assets: names.map((name) => ({
      name,
      size: 1,
      download_count: 0,
      url: "",
      browser_download_url: "",
    })),
  };
}

const DESKTOP_050 = [
  "FluxDown-0.5.0-windows-x64-setup.exe",
  "FluxDown-0.5.0-linux-x64.tar.gz",
  "FluxDown-0.5.0-macos-arm64.tar.gz",
];

// GitHub 列表顺序：created_at 倒序
const legacy = [
  release("mobile-v0.4.8", ["FluxDown-0.4.8-android-universal.apk", "SHA256SUMS.txt"]),
  release("v0.4.8", ["FluxDown-0.4.8-windows-x64-setup.exe", "SHA256SUMS.txt"]),
  release("server-v0.4.8", ["FluxDown-Server-0.4.8-linux-x64.tar.gz", "SHA256SUMS.txt"]),
];

describe("pickComponentRelease", () => {
  test("统一 release 按哨兵归属组件，缺哨兵的组件回落到上一个完整 release", () => {
    // Android 打包失败：APK 半途上传但没有 SHA256SUMS-mobile.txt
    const unified = release("v0.5.0", [
      ...DESKTOP_050,
      "FluxDown-Server-0.5.0-linux-x64.tar.gz",
      "FluxDown-0.5.0-android-universal.apk",
      "SHA256SUMS-app.txt",
      "SHA256SUMS-server.txt",
      "SHA256SUMS.txt",
    ]);
    const pool = [unified, ...legacy];
    expect(pickComponentRelease(pool, "app", false)?.tag_name).toBe("v0.5.0");
    expect(pickComponentRelease(pool, "server", false)?.tag_name).toBe("v0.5.0");
    expect(pickComponentRelease(pool, "mobile", false)?.tag_name).toBe("mobile-v0.4.8");
    expect(pickComponentRelease(pool, "cli", false)).toBeUndefined();
  });

  test("桌面端缺失时 app 回落旧版，而同一统一 release 的服务器照常选中", () => {
    const unified = release("v0.5.0", [
      "FluxDown-Server-0.5.0-linux-x64.tar.gz",
      "SHA256SUMS-server.txt",
    ]);
    const pool = [unified, ...legacy];
    expect(pickComponentRelease(pool, "app", false)?.tag_name).toBe("v0.4.8");
    expect(pickComponentRelease(pool, "server", false)?.tag_name).toBe("v0.5.0");
  });

  test("frontier 取 SemVer 最大：统一 rc 盖过历史组件稳定版，稳定版不看预发布", () => {
    const rc = release(
      "v0.5.0-rc.2",
      ["FluxDown-0.5.0-rc.2-android-universal.apk", "SHA256SUMS-mobile.txt"],
      true,
    );
    const pool = [...legacy, rc];
    expect(pickComponentRelease(pool, "mobile", true)?.tag_name).toBe("v0.5.0-rc.2");
    expect(
      pickComponentRelease(pool.filter((r) => !r.prerelease), "mobile", false)?.tag_name,
    ).toBe("mobile-v0.4.8");
  });

  test("iOS：并入哨兵前挂在统一 release 上的 IPA 按资产名识别，哨兵版本出现后以哨兵为准", () => {
    // 旧版本：IPA 由独立 workflow 挂到已发布的统一 release，没有 SHA256SUMS-ios.txt
    const attached = release("v0.6.3", [
      "FluxDown-0.6.3-windows-x64-setup.exe",
      "FluxDown-0.6.3-ios-sideload-unsigned.ipa",
      "SHA256SUMS-app.txt",
    ]);
    // iOS 未改动的新版本：不含 IPA，回落到上一个带 IPA 的版本
    const unchanged = release("v0.6.4", ["FluxDown-0.6.4-windows-x64-setup.exe", "SHA256SUMS-app.txt"]);
    expect(pickComponentRelease([unchanged, attached], "ios", false)?.tag_name).toBe("v0.6.3");
    const uploaded = release("v0.6.5", [
      "FluxDown-0.6.5-ios-sideload-unsigned.ipa",
      "SHA256SUMS-ios.txt",
    ]);
    expect(pickComponentRelease([uploaded, unchanged, attached], "ios", false)?.tag_name).toBe("v0.6.5");
  });
});

test("desktopAssets 排除同后缀的服务器与 CLI 压缩包", () => {
  const unified = release("v0.5.0", [
    "FluxDown-CLI-0.5.0-linux-x64.tar.gz",
    "FluxDown-Server-0.5.0-macos-arm64.tar.gz",
    ...DESKTOP_050,
  ]);
  const assets = desktopAssets(unified);
  expect(assets.find((a) => a.name.endsWith("-linux-x64.tar.gz"))?.name).toBe(
    "FluxDown-0.5.0-linux-x64.tar.gz",
  );
  expect(assets.find((a) => a.name.endsWith("-macos-arm64.tar.gz"))?.name).toBe(
    "FluxDown-0.5.0-macos-arm64.tar.gz",
  );
});

test("stripReleaseHeader 只剥掉发布头部，保留双语更新说明", () => {
  const notes = "<!-- fluxdown:lang:zh -->\n## 0.5.0\n- 新功能\n<!-- fluxdown:lang:en -->\n## 0.5.0\n- feat\n";
  const body =
    "<!-- fluxdown:release:begin components=server,cli -->\n<details>\n<summary>📦</summary>\n\n" +
    "## 📦 Headless 服务器\n\n```bash\ndocker pull x\n```\n\n</details>\n<!-- fluxdown:release:end -->\n\n" +
    notes;
  expect(stripReleaseHeader(body)).toBe(notes);
  expect(stripReleaseHeader(notes)).toBe(notes);
});
