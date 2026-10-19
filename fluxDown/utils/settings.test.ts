import { beforeEach, describe, expect, mock, test } from "bun:test";
import type { SiteProfile } from "./site-profiles";
import { createStorageArea } from "./storage-mock.test-util";

const local = createStorageArea();
const sync = createStorageArea();
mock.module("wxt/browser", () => ({
  browser: { storage: { local, sync } },
}));
// wxt/browser 必须先 mock 再加载被测模块（与 i18n.test.ts 同一约定），故此处动态导入。
const { DEFAULT_SETTINGS, loadSettings, migrateSettings, shouldIntercept } = await import(
  "./settings"
);
const { DEFAULT_SITE_PROFILE } = await import("./site-profiles");

function profile(intercept: SiteProfile["intercept"]): SiteProfile {
  return { ...DEFAULT_SITE_PROFILE, intercept, updatedAt: 1 };
}

beforeEach(() => {
  local.data = {};
  sync.data = {};
});

describe("shouldIntercept 站点配置", () => {
  const settings = { ...DEFAULT_SETTINGS };

  test("never：URL 主机命中即不拦截", () => {
    const profiles = { "example.com": profile("never") };
    expect(
      shouldIntercept({ url: "https://dl.example.com/a.zip" }, settings, profiles),
    ).toBe(false);
  });

  test("never：来源页主机命中同样不拦截（CDN 托管下载）", () => {
    const profiles = { "example.com": profile("never") };
    expect(
      shouldIntercept(
        { url: "https://cdn.other.net/a.zip", referrerUrl: "https://www.example.com/page" },
        settings,
        profiles,
      ),
    ).toBe(false);
  });

  test("always：跳过 smart 判定（无扩展名的 HTML 也接管）", () => {
    const url = "https://example.com/page";
    expect(shouldIntercept({ url, mime: "text/html", fileSize: 10 }, settings, {})).toBe(false);
    expect(
      shouldIntercept({ url, mime: "text/html", fileSize: 10 }, settings, {
        "example.com": profile("always"),
      }),
    ).toBe(true);
  });

  test("always：最小大小与排除扩展名仍生效", () => {
    const profiles = { "example.com": profile("always") };
    expect(
      shouldIntercept(
        { url: "https://example.com/a.zip", fileSize: 10 },
        { ...settings, minFileSize: 1000 },
        profiles,
      ),
    ).toBe(false);
    expect(
      shouldIntercept(
        { url: "https://example.com/a.zip" },
        { ...settings, excludeExtensions: [".zip"] },
        profiles,
      ),
    ).toBe(false);
  });

  test("never 优先于另一侧的 always", () => {
    const profiles = { "a.com": profile("never"), "b.com": profile("always") };
    expect(
      shouldIntercept(
        { url: "https://b.com/x.zip", referrerUrl: "https://a.com/" },
        settings,
        profiles,
      ),
    ).toBe(false);
  });
});

describe("设置迁移", () => {
  test("迁移后 sync 中不含旧字段，悬浮球可见性并入 showPageDot", async () => {
    sync.data.settings = {
      enabled: false,
      excludeDomains: ["a.com"],
      showResourcePanel: true,
      interceptMode: "extension",
    };
    local.data.fluxdown_dot_visible = false;

    const loaded = await loadSettings();
    expect(loaded.enabled).toBe(false);
    expect(loaded.showPageDot).toBe(false);
    expect(loaded.interceptMode).toBe("smart");
    expect("excludeDomains" in loaded).toBe(false);

    const stored = sync.data.settings as Record<string, unknown>;
    expect("excludeDomains" in stored).toBe(false);
    expect("showResourcePanel" in stored).toBe(false);
    expect(stored.showPageDot).toBe(false);
    expect("fluxdown_dot_visible" in local.data).toBe(false);
  });

  test("migrateSettings 合并默认值并去掉旧字段", () => {
    const migrated = migrateSettings({ excludeDomains: ["x.com"], sniffImages: true });
    expect(migrated.sniffImages).toBe(true);
    expect(migrated.fallbackOnReject).toBe(true);
    expect("excludeDomains" in migrated).toBe(false);
    expect(migrateSettings(null)).toEqual(DEFAULT_SETTINGS);
  });
});
