import { describe, expect, test } from "bun:test";
import { captureShortcut, type KeyInput, shortcutId, shortcutKeys, shortcutSettingsUrl } from "./shortcuts";

function press(code: string, key: string, mods: Partial<KeyInput> = {}): KeyInput {
  return { code, key, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods };
}

describe("captureShortcut", () => {
  test("按物理键位生成 commands.update 语法，与键盘布局无关", () => {
    expect(captureShortcut(press("KeyD", "∂", { altKey: true, shiftKey: true }), true)).toEqual({
      kind: "ok",
      shortcut: "Alt+Shift+D",
    });
    expect(captureShortcut(press("Digit5", "%", { ctrlKey: true, shiftKey: true }), false)).toEqual({
      kind: "ok",
      shortcut: "Ctrl+Shift+5",
    });
    expect(captureShortcut(press("ArrowUp", "ArrowUp", { ctrlKey: true }), false)).toEqual({
      kind: "ok",
      shortcut: "Ctrl+Up",
    });
  });

  test("macOS 上 ⌘ 写作 Command、⌃ 写作 MacCtrl", () => {
    expect(captureShortcut(press("KeyK", "k", { metaKey: true, ctrlKey: true }), true)).toEqual({
      kind: "ok",
      shortcut: "Command+MacCtrl+K",
    });
  });

  test("只按修饰键时保持等待并回报已按下的修饰键", () => {
    expect(captureShortcut(press("AltLeft", "Alt", { altKey: true }), false)).toEqual({
      kind: "pending",
      modifiers: ["Alt"],
    });
  });

  test("缺少主修饰键（含仅 Shift）不可用", () => {
    expect(captureShortcut(press("KeyD", "D", { shiftKey: true }), false)).toEqual({
      kind: "invalid",
      reason: "modifier",
    });
    expect(captureShortcut(press("KeyD", "d"), false)).toEqual({ kind: "invalid", reason: "modifier" });
  });

  test("Windows / Linux 的 Win 键与不支持的主键被拒绝", () => {
    expect(captureShortcut(press("KeyD", "d", { metaKey: true }), false)).toEqual({ kind: "invalid", reason: "key" });
    expect(captureShortcut(press("Slash", "/", { ctrlKey: true }), false)).toEqual({ kind: "invalid", reason: "key" });
    expect(captureShortcut(press("F13", "F13", { ctrlKey: true }), false)).toEqual({ kind: "invalid", reason: "key" });
  });

  test("媒体键只能单独使用", () => {
    expect(captureShortcut(press("MediaTrackNext", "MediaTrackNext"), false)).toEqual({
      kind: "ok",
      shortcut: "MediaNextTrack",
    });
    expect(captureShortcut(press("MediaPlayPause", "MediaPlayPause", { ctrlKey: true }), false)).toEqual({
      kind: "invalid",
      reason: "key",
    });
  });

  test("无修饰键的 Esc 取消、Backspace / Delete 清除；带修饰键的 Delete 是普通主键", () => {
    expect(captureShortcut(press("Escape", "Escape"), false)).toEqual({ kind: "cancel" });
    expect(captureShortcut(press("Backspace", "Backspace"), false)).toEqual({ kind: "clear" });
    expect(captureShortcut(press("Delete", "Delete"), false)).toEqual({ kind: "clear" });
    expect(captureShortcut(press("Delete", "Delete", { ctrlKey: true, altKey: true }), false)).toEqual({
      kind: "ok",
      shortcut: "Ctrl+Alt+Delete",
    });
  });
});

describe("shortcutId", () => {
  test("修饰键顺序无关，Chrome macOS 符号串与文本写法等价", () => {
    expect(shortcutId("Shift+Alt+D", false)).toBe(shortcutId("Alt+Shift+D", false));
    expect(shortcutId("⌥⇧D", true)).toBe("Alt+Shift+D");
    expect(shortcutId("⇧⌘Y", true)).toBe("Command+Shift+Y");
  });

  test("macOS 上 Firefox 的 Ctrl 视同 ⌘，其它平台不合并", () => {
    expect(shortcutId("Ctrl+Shift+Y", true)).toBe(shortcutId("Command+Shift+Y", true));
    expect(shortcutId("Ctrl+Shift+Y", false)).not.toBe(shortcutId("MacCtrl+Shift+Y", false));
  });

  test("未设置为空串", () => {
    expect(shortcutId("", false)).toBe("");
    expect(shortcutId("  ", true)).toBe("");
  });
});

describe("shortcutKeys", () => {
  test("macOS 按 ⌃ ⌥ ⇧ ⌘ 顺序显示符号键帽", () => {
    expect(shortcutKeys("Alt+Shift+D", true)).toEqual(["⌥", "⇧", "D"]);
    expect(shortcutKeys("Command+MacCtrl+Up", true)).toEqual(["⌃", "⌘", "↑"]);
    expect(shortcutKeys("Ctrl+Shift+Y", true)).toEqual(["⇧", "⌘", "Y"]);
    expect(shortcutKeys("⌥⇧V", true)).toEqual(["⌥", "⇧", "V"]);
  });

  test("其它平台显示文本键帽", () => {
    expect(shortcutKeys("Ctrl+Shift+Comma", false)).toEqual(["Ctrl", "Shift", ","]);
    expect(shortcutKeys("", false)).toEqual([]);
  });
});

describe("shortcutSettingsUrl", () => {
  test("Edge 用 edge://，其余 Chromium 用 chrome://", () => {
    expect(
      shortcutSettingsUrl("Mozilla/5.0 AppleWebKit/537.36 Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0"),
    ).toBe("edge://extensions/shortcuts");
    expect(shortcutSettingsUrl("Mozilla/5.0 AppleWebKit/537.36 Chrome/141.0.0.0 Safari/537.36")).toBe(
      "chrome://extensions/shortcuts",
    );
  });
});
