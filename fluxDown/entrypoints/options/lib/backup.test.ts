import { beforeEach, describe, expect, mock, test } from "bun:test";
import { createStorageArea } from "@/utils/storage-mock.test-util";

const local = createStorageArea();
const sync = createStorageArea();
mock.module("wxt/browser", () => ({ browser: { storage: { local, sync } } }));
// wxt/browser 必须先 mock 再加载被测模块（与 utils/i18n.test.ts 同一约定）。
const { applyBackup, backupFileName, collectBackup, parseBackup, sanitizeSettings } = await import("./backup");
const { loadSettings, saveSettings } = await import("@/utils/settings");

const rule = {
  id: "local.demo-1",
  version: 2,
  tier: "local",
  stage: "response",
  match: { host: ["example.com"] },
  action: { kind: "ignore" },
};

function backupText(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    format: "fluxdown-extension-backup",
    version: 1,
    exportedAt: "2026-10-15T00:00:00.000Z",
    settings: { enabled: false, minFileSize: 1024, excludeDomains: ["x.com"], bogus: 1, remoteToken: "stolen" },
    siteProfiles: { "Example.com": { intercept: "never", sniff: false, pageDot: true, floatButton: true, saveDir: "/d", updatedAt: 1 } },
    localRules: [rule, { ...rule, id: "local.bad-1", match: { pathRegex: "(a+)+" } }],
    ruleOverrides: { "builtin.ext.video": { enabled: false }, broken: { enabled: "yes" } },
    dotPos: { x: 1, y: 2 },
    theme: "dark",
    locale: "en",
    ...overrides,
  });
}

beforeEach(() => {
  local.data = {};
  sync.data = {};
});

describe("parseBackup", () => {
  test("拒绝非 JSON / 错误 format / 错误 version", () => {
    expect(parseBackup("{")).toEqual({ ok: false, error: "json" });
    expect(parseBackup(JSON.stringify({ format: "other" }))).toEqual({ ok: false, error: "format" });
    expect(parseBackup(JSON.stringify({ format: "fluxdown-extension-backup", version: 2 }))).toEqual({
      ok: false,
      error: "version",
    });
  });

  test("丢弃未知 / 旧 / 密钥字段，剔除不合格规则与覆盖项", () => {
    const parsed = parseBackup(backupText());
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    const { backup } = parsed;
    expect(backup.settings).toEqual({ enabled: false, minFileSize: 1024 });
    expect(Object.keys(backup.siteProfiles)).toEqual(["example.com"]);
    expect(backup.localRules.map((r) => r.id)).toEqual(["local.demo-1"]);
    expect(backup.rejectedRules.map((r) => r.id)).toEqual(["local.bad-1"]);
    expect(backup.ruleOverrides).toEqual({ "builtin.ext.video": { enabled: false } });
    expect(backup.theme).toBe("dark");
    expect(backup.locale).toBe("en");
  });
});

describe("sanitizeSettings", () => {
  test("类型 / 枚举不符的字段被丢弃", () => {
    expect(sanitizeSettings({ enabled: "yes", interceptMode: "weird", floatButtonTrigger: "always", minFileSize: -1 })).toEqual({
      floatButtonTrigger: "always",
    });
  });
});

describe("collectBackup / applyBackup", () => {
  test("导出不含 remoteToken / remoteVerified，并带出 theme / locale / dotPos", async () => {
    await saveSettings({ remoteUrl: "http://h:1", remoteToken: "secret" });
    local.data.theme = "light";
    local.data.fluxdown_locale = "zh-TW";
    local.data.fluxdown_dot_pos = { x: 5, y: 6 };
    const backup = await collectBackup(new Date("2026-10-15T08:00:00Z"));
    expect("remoteToken" in backup.settings).toBe(false);
    expect("remoteVerified" in backup.settings).toBe(false);
    expect(backup.settings.remoteUrl).toBe("http://h:1");
    expect(backup).toMatchObject({ theme: "light", locale: "zh-TW", dotPos: { x: 5, y: 6 }, version: 1 });
  });

  test("整体覆盖：保留本机 remoteToken，清除备份里缺失的 theme / locale / dotPos", async () => {
    await saveSettings({ remoteUrl: "http://h:1", remoteToken: "secret", enabled: true });
    local.data.theme = "light";
    local.data.fluxdown_locale = "zh-CN";
    local.data.fluxdown_dot_pos = { x: 9, y: 9 };
    const parsed = parseBackup(backupText({ theme: null, locale: null, dotPos: null, settings: { enabled: false, remoteUrl: "http://h:1" } }));
    if (!parsed.ok) throw new Error("parse failed");
    await applyBackup(parsed.backup);

    const settings = await loadSettings();
    expect(settings.enabled).toBe(false);
    expect(settings.remoteToken).toBe("secret");
    expect(local.data.fluxdown_local_rules).toHaveLength(1);
    expect(local.data.fluxdown_rule_overrides).toEqual({ "builtin.ext.video": { enabled: false } });
    expect("theme" in local.data).toBe(false);
    expect("fluxdown_locale" in local.data).toBe(false);
    expect("fluxdown_dot_pos" in local.data).toBe(false);
    expect(Object.keys(local.data.fluxdown_site_profiles as object)).toEqual(["example.com"]);
  });
});

describe("backupFileName", () => {
  test("含本地日期", () => {
    expect(backupFileName(new Date(2026, 9, 5))).toBe("fluxdown-extension-backup-2026-10-05.json");
  });
});
