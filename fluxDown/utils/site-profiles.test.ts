import { beforeEach, describe, expect, mock, test } from "bun:test";
import type { SiteProfile } from "./site-profiles";
import { createStorageArea } from "./storage-mock.test-util";

const local = createStorageArea();
const sync = createStorageArea();
mock.module("wxt/browser", () => ({
  browser: { storage: { local, sync } },
}));
// wxt/browser 必须先 mock 再加载被测模块（与 i18n.test.ts 同一约定），故此处动态导入。
const { DEFAULT_SITE_PROFILE, loadSiteProfiles, profileFor, saveSiteProfile } = await import(
  "./site-profiles"
);
const { loadSettings } = await import("./settings");

function profile(overrides: Partial<SiteProfile> = {}): SiteProfile {
  return { ...DEFAULT_SITE_PROFILE, updatedAt: 1, ...overrides };
}

beforeEach(() => {
  local.data = {};
  sync.data = {};
});

describe("profileFor", () => {
  test("取最长的后缀命中项", () => {
    const profiles = {
      "example.com": profile({ intercept: "never" }),
      "video.example.com": profile({ intercept: "always" }),
    };
    expect(profileFor(profiles, "cdn.video.example.com").key).toBe("video.example.com");
    expect(profileFor(profiles, "www.example.com").key).toBe("example.com");
    expect(profileFor(profiles, "video.example.com").profile.intercept).toBe("always");
  });

  test("按标签边界匹配，notexample.com 不命中；无命中返回默认", () => {
    const profiles = { "example.com": profile({ intercept: "never" }) };
    const miss = profileFor(profiles, "notexample.com");
    expect(miss.key).toBeNull();
    expect(miss.profile).toEqual(DEFAULT_SITE_PROFILE);
  });

  test("key 不使用 = 前缀语义：=example.com 不会被当作仅本域", () => {
    const profiles = { "=example.com": profile({ intercept: "never" }) };
    expect(profileFor(profiles, "example.com").key).toBeNull();
  });
});

describe("存取", () => {
  test("host 经归一化存储，null 删除", async () => {
    await saveSiteProfile("https://Example.com/path", profile({ saveDir: " /data " }));
    const loaded = await loadSiteProfiles();
    expect(Object.keys(loaded)).toEqual(["example.com"]);
    expect(loaded["example.com"].saveDir).toBe("/data");
    await saveSiteProfile("example.com", null);
    expect(await loadSiteProfiles()).toEqual({});
  });
});

describe("旧 excludeDomains 迁移", () => {
  test("迁移幂等且不覆盖已有站点配置", async () => {
    sync.data.settings = { enabled: true, excludeDomains: ["A.com", "b.com"] };
    local.data.fluxdown_site_profiles = {
      "a.com": profile({ intercept: "always", updatedAt: 9 }),
    };

    await loadSettings();
    const first = await loadSiteProfiles();
    expect(first["a.com"].intercept).toBe("always");
    expect(first["b.com"].intercept).toBe("never");
    expect(first["b.com"].sniff).toBe(true);

    await loadSettings();
    expect(await loadSiteProfiles()).toEqual(first);
    expect("excludeDomains" in (sync.data.settings as object)).toBe(false);
  });
});
