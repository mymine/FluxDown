/**
 * 扩展快捷键（manifest `commands`）的录制与显示规则。
 *
 * 写入格式遵循 WebExtensions `commands.update` 的 shortcut 语法：`Mod+Mod+Key`，
 * 修饰键 Ctrl / Alt / Shift，macOS 另有 Command（⌘）与 MacCtrl（⌃）；Firefox 中
 * macOS 的 Ctrl 等同 ⌘。读取时还要兼容 Chrome macOS 的 `commands.getAll`
 * 直接返回符号串（如 `⌥⇧D`）。
 */

export interface KeyInput {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

export type Capture =
  /** 只按下了修饰键：继续等待主键，`modifiers` 供实时预览。 */
  | { kind: "pending"; modifiers: string[] }
  | { kind: "cancel" }
  | { kind: "clear" }
  /** `modifier`：缺少主修饰键；`key`：主键或修饰键不受支持。 */
  | { kind: "invalid"; reason: "modifier" | "key" }
  | { kind: "ok"; shortcut: string };

const MODIFIER_KEYS: Record<string, true> = { Control: true, Alt: true, AltGraph: true, Shift: true, Meta: true, OS: true };

const NAMED_CODES: Record<string, string> = {
  Comma: "Comma",
  Period: "Period",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Space: "Space",
  Insert: "Insert",
  Delete: "Delete",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
};

/** 媒体键可单独作为快捷键，无需修饰键。 */
const MEDIA_CODES: Record<string, string> = {
  MediaPlayPause: "MediaPlayPause",
  MediaStop: "MediaStop",
  MediaTrackNext: "MediaNextTrack",
  MediaTrackPrevious: "MediaPrevTrack",
};

const PRIMARY_MODIFIERS: Record<string, true> = { Ctrl: true, Command: true, MacCtrl: true, Alt: true };
const MODIFIER_ORDER = ["Ctrl", "Command", "MacCtrl", "Alt", "Shift"];

/** 按物理键位（`code`）取主键名，与键盘布局无关；不支持的键返回 null。 */
function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  if (/^F([1-9]|1[0-2])$/.test(code)) return code;
  return NAMED_CODES[code] ?? MEDIA_CODES[code] ?? null;
}

function modifiersOf(input: KeyInput, mac: boolean): string[] | null {
  const mods: string[] = [];
  if (mac) {
    if (input.metaKey) mods.push("Command");
    if (input.ctrlKey) mods.push("MacCtrl");
  } else {
    // Windows / Linux 的 Win / Super 键不能用于扩展快捷键。
    if (input.metaKey) return null;
    if (input.ctrlKey) mods.push("Ctrl");
  }
  if (input.altKey) mods.push("Alt");
  if (input.shiftKey) mods.push("Shift");
  return mods;
}

/** 把一次 keydown 解析为录制结果。 */
export function captureShortcut(input: KeyInput, mac: boolean): Capture {
  const mods = modifiersOf(input, mac);
  if (MODIFIER_KEYS[input.key]) {
    return mods ? { kind: "pending", modifiers: mods } : { kind: "invalid", reason: "key" };
  }
  if (mods === null) return { kind: "invalid", reason: "key" };
  if (mods.length === 0 && input.key === "Escape") return { kind: "cancel" };
  if (mods.length === 0 && (input.key === "Backspace" || input.key === "Delete")) return { kind: "clear" };

  const key = keyName(input.code);
  if (!key) return { kind: "invalid", reason: "key" };
  const media = input.code in MEDIA_CODES;
  if (media && mods.length > 0) return { kind: "invalid", reason: "key" };
  if (!media && !mods.some((m) => PRIMARY_MODIFIERS[m])) return { kind: "invalid", reason: "modifier" };
  return { kind: "ok", shortcut: [...mods, key].join("+") };
}

const MAC_SYMBOLS: Record<string, string> = { "⌃": "MacCtrl", "⌥": "Alt", "⇧": "Shift", "⌘": "Command" };

/** 拆成规范 token（修饰键 + 主键）；兼容 `Alt+Shift+D` 与 Chrome macOS 的 `⌥⇧D`。 */
function tokens(shortcut: string): string[] {
  const s = shortcut.trim();
  if (!s) return [];
  if (s.includes("+")) return s.split("+").map((part) => part.trim()).filter(Boolean);
  const out: string[] = [];
  let rest = s;
  while (rest && MAC_SYMBOLS[rest[0]]) {
    out.push(MAC_SYMBOLS[rest[0]]);
    rest = rest.slice(1);
  }
  if (rest) out.push(rest);
  return out;
}

/** 比较用的规范键：修饰键按固定顺序排列（macOS 上 Firefox 的 Ctrl 视同 Command）。 */
export function shortcutId(shortcut: string, mac: boolean): string {
  const parts = tokens(shortcut).map((p) => (mac && p === "Ctrl" ? "Command" : p));
  const mods = parts.filter((p) => MODIFIER_ORDER.includes(p));
  const keys = parts.filter((p) => !MODIFIER_ORDER.includes(p));
  mods.sort((a, b) => MODIFIER_ORDER.indexOf(a) - MODIFIER_ORDER.indexOf(b));
  return [...mods, ...keys].join("+");
}

const MAC_LABELS: Record<string, string> = { Ctrl: "⌘", Command: "⌘", MacCtrl: "⌃", Alt: "⌥", Shift: "⇧" };
const PC_LABELS: Record<string, string> = { Command: "Win", MacCtrl: "Ctrl" };
const KEY_LABELS: Record<string, string> = {
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
  Comma: ",",
  Period: ".",
  MediaPlayPause: "⏯",
  MediaStop: "⏹",
  MediaNextTrack: "⏭",
  MediaPrevTrack: "⏮",
};

/** macOS 菜单惯例的修饰键顺序：⌃ ⌥ ⇧ ⌘。 */
const MAC_DISPLAY_RANK: Record<string, number> = { MacCtrl: 0, Alt: 1, Shift: 2, Command: 3 };

/** 按平台习惯输出逐个键帽文本；空串（未设置）返回空数组。 */
export function shortcutKeys(shortcut: string, mac: boolean): string[] {
  const id = shortcutId(shortcut, mac);
  if (!id) return [];
  const parts = id.split("+");
  if (mac) parts.sort((a, b) => (MAC_DISPLAY_RANK[a] ?? 9) - (MAC_DISPLAY_RANK[b] ?? 9));
  const labels = mac ? MAC_LABELS : PC_LABELS;
  return parts.map((p) => labels[p] ?? KEY_LABELS[p] ?? p);
}

/** 不支持 `commands.update` 的浏览器（Chromium 系）只能在自带的扩展快捷键页修改。 */
export function shortcutSettingsUrl(userAgent: string): string {
  return /\bEdg\//.test(userAgent) ? "edge://extensions/shortcuts" : "chrome://extensions/shortcuts";
}
