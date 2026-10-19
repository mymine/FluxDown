// Zero-dependency manifest validator. Mirrors FluxDown
// native/engine/src/plugin/manifest.rs (`PluginManifest::validate` + `check_package_files`
// + `validate_sniff_pack`) rule by rule; shared fixtures:
// native/engine/src/plugin/fixtures/manifest_cases.json.
//
// This file is bundled into the single zero-dependency `dist/manifest.mjs`, which the
// plugin index vendors. Do not import anything here (no node: modules, no Buffer).

export const VALID_PERMISSIONS = [
  "ffmpeg",
  "ytdlp",
  "auth",
  "tasks",
  "browserRules",
  "desktop",
  "background",
  "intercept",
] as const;
export const VALID_INTERCEPT_STAGES = [
  "prepare",
  "probed",
  "refresh",
  "selectVariant",
  "selectPeriod",
  "segments",
  "liveSegments",
  "finalize",
  "failed",
] as const;
export type PluginPermission = (typeof VALID_PERMISSIONS)[number];

export const VALID_EVENTS = ["onStart", "onError", "onDone", "onMetaProbed", "onCancel"] as const;
export const VIEW_LOCATIONS = ["activity", "settings", "taskDetail", "window"] as const;

export const WINDOW_SIZE_MIN = 240;
export const WINDOW_SIZE_MAX = 4096;
export const NETWORK_HOSTS_MAX = 64;
export const VIEWS_MAX = 16;
export const COMMANDS_MAX = 32;
export const TITLE_MAX_CHARS = 40;
export const SNIFF_PACK_MAX_BYTES = 256 * 1024;
export const SNIFF_PACK_MAX_RULES = 500;
export const SNIFF_PACK_FORMAT = "fluxdown-sniff-pack";

const RSS_PROVIDER_ID = "rss";

export type PluginRisk = "standard" | "elevated" | "full";

export interface ValidateOptions {
  /**
   * Listing-grade checks, stricter than the client: version/minAppVersion must be a plain
   * three-part number, and every resolver/hooks/auth/subscription entry and `icon` must exist
   * in the package. The client only requires `main`, `desktop` and `sniffRules` files at install.
   */
  strict?: boolean;
}

export interface ValidationResult {
  ok: boolean;
  errors: string[];
}

type Obj = Record<string, unknown>;

const isObject = (value: unknown): value is Obj =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const nonEmptyString = (value: unknown): value is string =>
  typeof value === "string" && value.trim() !== "";
const stringArray = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((v) => typeof v === "string");
const charLength = (s: string) => [...s].length;

/** Rust `is_safe_relative_path`. */
export function isSafeRelativePath(path: unknown): path is string {
  if (typeof path !== "string" || path === "") return false;
  if (path.startsWith("/") || path.startsWith("\\")) return false;
  if (/^[A-Za-z]:/.test(path)) return false;
  for (const segment of path.split(/[/\\]/)) {
    if (segment === "" || segment === "." || segment === "..") return false;
  }
  // Rust `char::is_control`: Cc category.
  return !/[\u0000-\u001f\u007f-\u009f]/.test(path);
}

/** Rust `is_valid_identity`: `^[a-z0-9_-]+@[a-z0-9_-]+$`. */
export function isValidIdentity(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9_-]+@[a-z0-9_-]+$/.test(value);
}

/** Rust `is_valid_contribution_id`: `^[a-z][a-z0-9-]{0,31}$`. */
const isValidContributionId = (value: unknown): value is string =>
  typeof value === "string" && /^[a-z][a-z0-9-]{0,31}$/.test(value);

const hasExtension = (path: string, exts: readonly string[]) => {
  const lower = path.toLowerCase();
  return exts.some((e) => lower.endsWith(e));
};

/** Rust `parse_semver`: optional `v`, pre-release/build trimmed, three integer segments. */
function parseSemver(raw: string): [number, number, number] | null {
  let s = raw.trim();
  if (s.startsWith("v")) s = s.slice(1);
  const core = s.split(/[-+]/)[0] ?? "";
  const parts = core.split(".");
  if (parts.length !== 3 || !parts.every((p) => /^\d+$/.test(p))) return null;
  return parts.map(Number) as [number, number, number];
}

const STRICT_SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

/** IPv4 dotted quad. */
function isIpv4(host: string): boolean {
  const parts = host.split(".");
  return (
    parts.length === 4 &&
    parts.every((p) => /^(0|[1-9]\d{0,2})$/.test(p) && Number(p) <= 255)
  );
}

/** Loose-but-sound IPv6 literal check (no zone ids, as Rust `IpAddr::parse`). */
function isIpv6(host: string): boolean {
  if (!host.includes(":") || !/^[0-9a-fA-F:.]+$/.test(host)) return false;
  let rest = host;
  let groupsAllowed = 8;
  // Trailing embedded IPv4 counts as two groups.
  const lastColon = rest.lastIndexOf(":");
  const tail = rest.slice(lastColon + 1);
  if (tail.includes(".")) {
    if (!isIpv4(tail)) return false;
    rest = rest.slice(0, lastColon + 1) + "0:0";
  }
  const doubles = rest.split("::").length - 1;
  if (doubles > 1) return false;
  const groups = rest.split(":");
  if (doubles === 1) {
    const [head = "", back = ""] = rest.split("::");
    const h = head === "" ? [] : head.split(":");
    const b = back === "" ? [] : back.split(":");
    groupsAllowed = 7;
    return (
      h.length + b.length <= groupsAllowed &&
      [...h, ...b].every((g) => /^[0-9a-fA-F]{1,4}$/.test(g))
    );
  }
  return groups.length === 8 && groups.every((g) => /^[0-9a-fA-F]{1,4}$/.test(g));
}

/** Lowercase DNS hostname (RFC 1123 labels, ≤253) or IP literal; no scheme/port/path/wildcard. */
export function isValidNetworkHost(host: unknown): host is string {
  if (typeof host !== "string" || host === "" || host.length > 253) return false;
  if (isIpv4(host)) return true;
  if (isIpv6(host)) return host === host.toLowerCase();
  return host.split(".").every((label) => /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(label));
}

function unknownKeys(errors: string[], where: string, value: Obj, allowed: readonly string[]) {
  for (const key of Object.keys(value)) {
    if (!allowed.includes(key)) errors.push(`${where}: 未知字段 ${key}`);
  }
}

function checkOptionalString(errors: string[], where: string, value: unknown) {
  if (value !== undefined && value !== null && typeof value !== "string") {
    errors.push(`${where} 须为字符串`);
  }
}

function checkOptionalBool(errors: string[], where: string, value: unknown) {
  if (value !== undefined && value !== null && typeof value !== "boolean") {
    errors.push(`${where} 须为布尔`);
  }
}

/** Package file set; `null` = unknown, so existence checks are skipped. */
type Files = Set<string> | null;

const inPackage = (files: Files, path: string) => files === null || files.has(path.replaceAll("\\", "/"));

/** Entry path: safe relative path; in strict mode it must also exist in the package. */
function checkEntry(
  errors: string[],
  where: string,
  entry: unknown,
  files: Files,
  strict: boolean,
) {
  if (!isSafeRelativePath(entry)) {
    errors.push(`${where} 路径 '${String(entry)}' 非法`);
  } else if (strict && !inPackage(files, entry)) {
    errors.push(`${where} 指向的 ${entry} 不在包内`);
  }
}

function checkTimeout(errors: string[], where: string, value: unknown) {
  if (value === undefined || value === null) return;
  if (!Number.isSafeInteger(value) || (value as number) <= 0) errors.push(`${where} timeoutMs 须为正整数`);
}

function checkMatch(errors: string[], where: string, match: unknown) {
  if (!isObject(match)) return void errors.push(`${where} 须为对象`);
  unknownKeys(errors, where, match, ["urls"]);
  if (!Array.isArray(match.urls) || match.urls.length === 0 || !match.urls.every(nonEmptyString)) {
    errors.push(`${where}.urls 须为非空字符串数组`);
  }
}

const hasPermission = (permissions: unknown, name: string) =>
  Array.isArray(permissions) && permissions.includes(name);

const validTitle = (title: unknown) =>
  typeof title === "string" && charLength(title) >= 1 && charLength(title) <= TITLE_MAX_CHARS;

/** `main`: classic script run by the daemon's QuickJS host. */
function checkMain(errors: string[], manifest: Obj, files: Files, permissions: unknown) {
  const main = manifest.main;
  if (main === undefined || main === null) return;
  if (!isObject(main)) return void errors.push("main 须为对象");
  unknownKeys(errors, "main", main, ["entry", "events", "background"]);
  if (!isSafeRelativePath(main.entry)) {
    errors.push(`main entry 路径 '${String(main.entry)}' 非法`);
  } else {
    if (!hasExtension(main.entry, [".js"])) errors.push(`main entry '${main.entry}' 扩展名须为 .js`);
    if (!inPackage(files, main.entry)) errors.push(`包内缺少文件 '${main.entry}'`);
  }
  const events = main.events ?? [];
  if (!stringArray(events)) errors.push("main.events 须为字符串数组");
  else {
    for (const ev of events) {
      if (!(VALID_EVENTS as readonly string[]).includes(ev)) {
        errors.push(`main.events 未知事件 '${ev}'，合法：${VALID_EVENTS.join(", ")}`);
      }
    }
  }
  if (main.background !== undefined && main.background !== null && typeof main.background !== "boolean") {
    errors.push("main.background 须为布尔");
  } else if (main.background === true && !hasPermission(permissions, "background")) {
    errors.push("main.background 为 true 时 permissions 必须包含 background");
  }
}

/** A view's `desktop` entry is a GPUI Shell ESM: shipped in the package, never compiled by the engine. */
function checkDesktop(
  errors: string[],
  view: Obj,
  desktop: unknown,
  files: Files,
  permissions: unknown,
) {
  const where = `view '${String(view.id)}'`;
  if (!isObject(desktop)) return void errors.push(`${where} desktop 须为对象`);
  unknownKeys(errors, `${where} desktop`, desktop, ["entry", "networkHosts", "width", "height"]);
  if (!isSafeRelativePath(desktop.entry)) {
    errors.push(`${where} desktop entry 路径 '${String(desktop.entry)}' 非法`);
  } else {
    if (!hasExtension(desktop.entry, [".js", ".mjs"])) {
      errors.push(`${where} desktop entry '${desktop.entry}' 扩展名须为 .js 或 .mjs`);
    }
    if (!inPackage(files, desktop.entry)) errors.push(`包内缺少文件 '${desktop.entry}'`);
  }
  if (!hasPermission(permissions, "desktop")) {
    errors.push(`${where} 声明 desktop 时 permissions 必须包含 desktop`);
  }
  for (const key of ["width", "height"] as const) {
    const value = desktop[key];
    if (value === undefined || value === null) continue;
    if (view.location !== "window") {
      errors.push(`${where} 的 ${key} 仅允许用于 location 为 window 的 view`);
    } else if (
      typeof value !== "number" ||
      !Number.isFinite(value) ||
      value < WINDOW_SIZE_MIN ||
      value > WINDOW_SIZE_MAX
    ) {
      errors.push(`${where} ${key} 须为 [${WINDOW_SIZE_MIN}, ${WINDOW_SIZE_MAX}] 内的有限数`);
    }
  }
  const hosts = desktop.networkHosts ?? [];
  if (!Array.isArray(hosts)) return void errors.push(`${where} networkHosts 须为数组`);
  if (hosts.length > NETWORK_HOSTS_MAX) errors.push(`${where} networkHosts 至多 ${NETWORK_HOSTS_MAX} 项`);
  const seen = new Set<string>();
  for (const host of hosts) {
    if (!isValidNetworkHost(host)) errors.push(`${where} networkHosts '${String(host)}' 非法`);
    else if (seen.has(host)) errors.push(`${where} networkHosts '${host}' 重复`);
    else seen.add(host);
  }
}

function checkContributes(
  errors: string[],
  manifest: Obj,
  files: Files,
  permissions: unknown,
) {
  const contributes = manifest.contributes;
  if (contributes === undefined || contributes === null) return;
  if (!isObject(contributes)) return void errors.push("contributes 须为对象");
  unknownKeys(errors, "contributes", contributes, ["views", "commands", "sniffRules"]);
  const hasMain = manifest.main !== undefined && manifest.main !== null;

  const views = contributes.views ?? [];
  if (!Array.isArray(views)) errors.push("contributes.views 须为数组");
  else {
    if (views.length > VIEWS_MAX) errors.push(`contributes.views 至多 ${VIEWS_MAX} 项`);
    const seen = new Set<string>();
    let settingsViews = 0;
    for (const view of views) {
      if (!isObject(view)) {
        errors.push("view 须为对象");
        continue;
      }
      unknownKeys(errors, "view", view, ["id", "title", "location", "desktop", "portable"]);
      if (!isValidContributionId(view.id)) {
        errors.push(`view id '${String(view.id)}' 非法：须匹配 ^[a-z][a-z0-9-]{0,31}$`);
      } else if (seen.has(view.id)) {
        errors.push(`view id '${view.id}' 重复`);
      } else seen.add(view.id);
      if (!validTitle(view.title)) {
        errors.push(`view '${String(view.id)}' title 须为 1–${TITLE_MAX_CHARS} 个字符`);
      }
      if (!(VIEW_LOCATIONS as readonly unknown[]).includes(view.location)) {
        errors.push(`view '${String(view.id)}' location 非法，合法：${VIEW_LOCATIONS.join(", ")}`);
      }
      if (view.location === "settings" && ++settingsViews > 1) {
        errors.push("location 为 settings 的 view 至多 1 个");
      }
      const hasDesktop = view.desktop !== undefined && view.desktop !== null;
      if (view.portable !== undefined && typeof view.portable !== "boolean") {
        errors.push(`view '${String(view.id)}' portable 须为布尔值`);
      }
      if (!hasDesktop) {
        if (view.portable === true) errors.push(`view '${String(view.id)}' 的 portable 仅允许与 desktop 同时声明`);
      } else checkDesktop(errors, view, view.desktop, files, permissions);
      const portable = !hasDesktop || view.portable === true;
      if (portable && !hasMain) errors.push(`view '${String(view.id)}' 有可移植实现，必须声明 main`);
    }
  }

  const commands = contributes.commands ?? [];
  if (!Array.isArray(commands)) errors.push("contributes.commands 须为数组");
  else {
    if (commands.length > COMMANDS_MAX) errors.push(`contributes.commands 至多 ${COMMANDS_MAX} 项`);
    const seen = new Set<string>();
    for (const command of commands) {
      if (!isObject(command)) {
        errors.push("command 须为对象");
        continue;
      }
      unknownKeys(errors, "command", command, ["id", "title", "palette", "taskMenu"]);
      if (!isValidContributionId(command.id)) {
        errors.push(`command id '${String(command.id)}' 非法：须匹配 ^[a-z][a-z0-9-]{0,31}$`);
      } else if (seen.has(command.id)) {
        errors.push(`command id '${command.id}' 重复`);
      } else seen.add(command.id);
      if (!validTitle(command.title)) {
        errors.push(`command '${String(command.id)}' title 须为 1–${TITLE_MAX_CHARS} 个字符`);
      }
      checkOptionalBool(errors, `command '${String(command.id)}' palette`, command.palette);
      checkOptionalBool(errors, `command '${String(command.id)}' taskMenu`, command.taskMenu);
    }
    if (commands.length > 0 && !hasMain) errors.push("声明 commands 时必须声明 main");
  }

  const rules = contributes.sniffRules;
  if (rules !== undefined && rules !== null) {
    if (!isSafeRelativePath(rules)) {
      errors.push(`sniffRules 路径 '${String(rules)}' 非法`);
    } else {
      if (!hasExtension(rules, [".json"])) errors.push(`sniffRules '${rules}' 扩展名须为 .json`);
      if (!hasPermission(permissions, "browserRules")) {
        errors.push("声明 sniffRules 时 permissions 必须包含 browserRules");
      }
      if (!inPackage(files, rules)) errors.push(`包内缺少文件 '${rules}'`);
    }
  }
}

function checkSetting(errors: string[], field: unknown, seen: Set<string>) {
  if (!isObject(field)) return void errors.push("settings 条目须为对象");
  const key = field.key;
  const where = `setting '${String(key)}'`;
  unknownKeys(errors, where, field, [
    "key", "title", "description", "type", "widget", "options", "default", "required",
    "min", "max", "pattern", "helperScript", "helperLabel",
  ]);
  if (!nonEmptyString(key)) errors.push("setting key 不可为空");
  else if (seen.has(key)) errors.push(`setting key '${key}' 重复`);
  else seen.add(key);
  if (typeof field.title !== "string") errors.push(`${where}: title 必填`);
  checkOptionalString(errors, `${where}: description`, field.description);
  checkOptionalBool(errors, `${where}: required`, field.required);
  const type = field.type;
  if (type !== "string" && type !== "number" && type !== "boolean") {
    return void errors.push(`${where}: type 非法`);
  }
  const widgets = ["text", "password", "textarea", "select", "toggle", "number", "folder"];
  const widget = field.widget ?? { string: "text", number: "number", boolean: "toggle" }[type];
  const allowedType: Record<string, string> = {
    text: "string", password: "string", textarea: "string", folder: "string", select: "string",
    toggle: "boolean", number: "number",
  };
  if (typeof widget !== "string" || !widgets.includes(widget)) {
    errors.push(`${where}: widget 非法`);
  } else if (allowedType[widget] !== type) {
    errors.push(`${where}: widget '${widget}' 不允许 type '${type}'`);
  }
  const optionsRaw = field.options ?? [];
  const optionsOk =
    Array.isArray(optionsRaw) &&
    optionsRaw.every((o) => isObject(o) && typeof o.value === "string" && typeof o.label === "string");
  if (!optionsOk) errors.push(`${where}: options 须为 {value,label} 字符串数组`);
  const options = optionsOk ? (optionsRaw as { value: string }[]) : [];
  const fallback = field.default;
  if (fallback !== undefined && fallback !== null && typeof fallback !== "string") {
    errors.push(`${where}: default 须为字符串`);
  }
  if (widget === "select") {
    if (optionsOk && options.length === 0) {
      errors.push(`${where}: select 的 options 不可为空`);
    } else if (typeof fallback === "string" && !options.some((o) => o.value === fallback)) {
      errors.push(`${where}: default '${fallback}' 不在 options 中`);
    }
  }
  for (const bound of ["min", "max"] as const) {
    const value = field[bound];
    if (value === undefined || value === null) continue;
    if (type !== "number") errors.push(`${where}: min/max 仅 number 有效`);
    else if (typeof value !== "number" || !Number.isFinite(value)) errors.push(`${where}: ${bound} 非有限数`);
  }
  if (typeof field.min === "number" && typeof field.max === "number" && field.min > field.max) {
    errors.push(`${where}: min(${field.min}) > max(${field.max})`);
  }
  if (field.pattern !== undefined && field.pattern !== null) {
    if (typeof field.pattern !== "string") errors.push(`${where}: pattern 须为字符串`);
    else if (type !== "string") errors.push(`${where}: pattern 仅 string 有效`);
  }
  if (field.helperScript !== undefined && field.helperScript !== null) {
    if (typeof field.helperScript !== "string") {
      errors.push(`${where}: helperScript 须为字符串`);
    } else {
      if (type !== "string") errors.push(`${where}: helperScript 仅 string 有效`);
      if (field.helperScript === "" || utf8Length(field.helperScript) > 4 * 1024) {
        errors.push(`${where}: helperScript 须非空且 ≤4KB`);
      }
    }
  }
  if (field.helperLabel !== undefined && field.helperLabel !== null) {
    if (
      typeof field.helperLabel !== "string" ||
      charLength(field.helperLabel) > 60 ||
      field.helperScript === undefined ||
      field.helperScript === null
    ) {
      errors.push(`${where}: helperLabel 须 ≤60 字符且须与 helperScript 同时出现`);
    }
  }
  if (type === "number" && typeof fallback === "string") {
    const value = Number(fallback);
    if (fallback.trim() === "" || !Number.isFinite(value)) {
      errors.push(`${where}: number default '${fallback}' 非法`);
    } else {
      if (typeof field.min === "number" && value < field.min) errors.push(`${where}: default ${value} < min ${field.min}`);
      if (typeof field.max === "number" && value > field.max) errors.push(`${where}: default ${value} > max ${field.max}`);
    }
  }
  if (type === "boolean" && typeof fallback === "string" && fallback !== "true" && fallback !== "false") {
    errors.push(`${where}: boolean default 必须为 'true'/'false'`);
  }
}

function utf8Length(s: string): number {
  let n = 0;
  for (const ch of s) {
    const cp = ch.codePointAt(0) as number;
    n += cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
  }
  return n;
}

/**
 * Validates a manifest against the engine rules. `files` is the set of package-relative
 * file paths (forward slashes). When omitted (`undefined`/`null`), file-existence checks are
 * skipped; `strict` mode requires it and reports a single error otherwise. Returns every error found.
 */
export function validateManifest(
  manifest: unknown,
  files?: Set<string> | null,
  options: ValidateOptions = {},
): ValidationResult {
  const errors: string[] = [];
  const strict = options.strict === true;
  const fileSet: Files = files ?? null;
  if (strict && fileSet === null) {
    return { ok: false, errors: ["strict 模式需要提供包内文件集合 files（用于校验入口文件是否存在）"] };
  }
  if (!isObject(manifest)) return { ok: false, errors: ["manifest.json 须为 JSON 对象"] };
  unknownKeys(errors, "manifest", manifest, [
    "identity", "name", "version", "description", "homepage", "icon", "minAppVersion",
    "resolvers", "subscriptions", "hooks", "interceptors", "auth", "main", "contributes", "settings", "permissions",
  ]);

  if (!isValidIdentity(manifest.identity)) {
    errors.push(`identity '${String(manifest.identity)}' 非法：须匹配 ^[a-z0-9_-]+@[a-z0-9_-]+$`);
  }
  if (!nonEmptyString(manifest.name)) errors.push("name 不可为空");
  const versionOk = (v: unknown) =>
    typeof v === "string" && (strict ? STRICT_SEMVER.test(v) : parseSemver(v) !== null);
  if (!versionOk(manifest.version)) {
    errors.push(
      strict
        ? `version '${String(manifest.version)}' 须为三段 MAJOR.MINOR.PATCH（不收预发布/v 前缀）`
        : `version '${String(manifest.version)}' 非法：须为 MAJOR.MINOR.PATCH`,
    );
  }
  for (const key of ["description", "homepage", "icon", "minAppVersion"]) {
    checkOptionalString(errors, key, manifest[key]);
  }
  if (typeof manifest.minAppVersion === "string" && manifest.minAppVersion !== "" && !versionOk(manifest.minAppVersion)) {
    errors.push(`minAppVersion '${manifest.minAppVersion}' 非法`);
  }
  if (typeof manifest.icon === "string" && manifest.icon !== "") {
    checkEntry(errors, "icon", manifest.icon, fileSet, strict);
  }

  const resolvers = manifest.resolvers ?? [];
  if (!Array.isArray(resolvers)) errors.push("resolvers 须为数组");
  else {
    if (resolvers.length > 1) errors.push("v1 每插件至多一个 resolver");
    for (const resolver of resolvers) {
      if (!isObject(resolver)) {
        errors.push("resolver 须为对象");
        continue;
      }
      unknownKeys(errors, "resolver", resolver, ["match", "entry", "timeoutMs", "multi"]);
      checkMatch(errors, "resolver match", resolver.match);
      checkEntry(errors, "resolver entry", resolver.entry, fileSet, strict);
      checkTimeout(errors, "resolver", resolver.timeoutMs);
      checkOptionalBool(errors, "resolver multi", resolver.multi);
    }
  }

  const subscriptions = manifest.subscriptions ?? [];
  if (!Array.isArray(subscriptions)) errors.push("subscriptions 须为数组");
  else {
    if (subscriptions.length > 1) errors.push("v1 每插件至多一个 subscription provider");
    for (const subscription of subscriptions) {
      if (!isObject(subscription)) {
        errors.push("subscription 须为对象");
        continue;
      }
      unknownKeys(errors, "subscription", subscription, ["providerId", "entry", "timeoutMs"]);
      const id = subscription.providerId;
      if (typeof id !== "string" || !/^[a-z0-9_-]{1,64}$/.test(id)) {
        errors.push(`subscription providerId '${String(id)}' 非法`);
      } else if (id === RSS_PROVIDER_ID) {
        errors.push(`subscription providerId '${id}' 为内置 provider 保留字`);
      }
      checkEntry(errors, "subscription entry", subscription.entry, fileSet, strict);
      checkTimeout(errors, "subscription", subscription.timeoutMs);
    }
  }

  const permissions = manifest.permissions ?? [];
  if (!stringArray(permissions)) errors.push("permissions 须为字符串数组");
  else {
    for (const permission of permissions) {
      if (!(VALID_PERMISSIONS as readonly string[]).includes(permission)) {
        errors.push(`未知权限 '${permission}'，合法：${VALID_PERMISSIONS.join(", ")}`);
      }
    }
  }

  if (manifest.hooks !== undefined && manifest.hooks !== null) {
    const hooks = manifest.hooks;
    if (!isObject(hooks)) errors.push("hooks 须为对象");
    else {
      unknownKeys(errors, "hooks", hooks, ["entry", "events", "match"]);
      checkEntry(errors, "hooks entry", hooks.entry, fileSet, strict);
      if (!stringArray(hooks.events) || hooks.events.length === 0) {
        errors.push("hooks.events 不可为空");
      } else {
        for (const event of hooks.events) {
          if (!(VALID_EVENTS as readonly string[]).includes(event)) {
            errors.push(`未知事件 '${event}'，合法：${VALID_EVENTS.join(", ")}`);
          }
        }
      }
      if (hooks.match !== undefined && hooks.match !== null) checkMatch(errors, "hooks match", hooks.match);
    }
  }

  if (manifest.auth !== undefined && manifest.auth !== null) {
    const auth = manifest.auth;
    if (!isObject(auth)) errors.push("auth 须为对象");
    else {
      unknownKeys(errors, "auth", auth, ["entry", "timeoutMs"]);
      checkEntry(errors, "auth entry", auth.entry, fileSet, strict);
      if (!hasPermission(permissions, "auth")) errors.push("声明 auth.entry 时 permissions 必须包含 auth");
      checkTimeout(errors, "auth", auth.timeoutMs);
    }
  }

  if (manifest.interceptors !== undefined && manifest.interceptors !== null) {
    const ic = manifest.interceptors;
    if (!isObject(ic)) errors.push("interceptors 须为对象");
    else {
      unknownKeys(errors, "interceptors", ic, ["entry", "stages", "match", "timeoutMs", "onFailure"]);
      checkEntry(errors, "interceptors entry", ic.entry, fileSet, strict);
      if (!stringArray(ic.stages) || ic.stages.length === 0) {
        errors.push("interceptors.stages 不可为空");
      } else {
        const seenStages = new Set<string>();
        for (const stage of ic.stages) {
          if (!(VALID_INTERCEPT_STAGES as readonly string[]).includes(stage)) {
            errors.push(`未知拦截阶段 '${stage}'，合法：${VALID_INTERCEPT_STAGES.join(", ")}`);
          } else if (seenStages.has(stage)) {
            errors.push(`interceptors.stages 重复：'${stage}'`);
          }
          seenStages.add(stage);
        }
      }
      checkMatch(errors, "interceptors match", ic.match);
      checkTimeout(errors, "interceptors", ic.timeoutMs);
      if (ic.onFailure !== undefined && ic.onFailure !== null && ic.onFailure !== "continue" && ic.onFailure !== "fail") {
        errors.push("interceptors.onFailure 须为 continue 或 fail");
      }
      if (!hasPermission(permissions, "intercept")) {
        errors.push("声明 interceptors 时 permissions 必须包含 intercept");
      }
    }
  }

  checkMain(errors, manifest, fileSet, permissions);
  checkContributes(errors, manifest, fileSet, permissions);

  const settings = manifest.settings ?? [];
  if (!Array.isArray(settings)) errors.push("settings 须为数组");
  else {
    const seen = new Set<string>();
    for (const field of settings) checkSetting(errors, field, seen);
  }
  return { ok: errors.length === 0, errors };
}

/** Risk level derived from permissions: `desktop` → full; any other elevated capability → elevated. */
export function riskOf(manifest: { permissions?: readonly string[] | null }): PluginRisk {
  const permissions = manifest.permissions ?? [];
  if (permissions.includes("desktop")) return "full";
  const elevated = ["ffmpeg", "ytdlp", "auth", "tasks", "browserRules", "background", "intercept"];
  return permissions.some((p) => elevated.includes(p)) ? "elevated" : "standard";
}

/** Sorted, de-duplicated union of every view's `desktop.networkHosts`. */
export function networkHostsOf(manifest: {
  contributes?: { views?: readonly { desktop?: { networkHosts?: readonly string[] | null } | null }[] | null } | null;
}): string[] {
  const hosts = new Set<string>();
  for (const view of manifest.contributes?.views ?? []) {
    for (const host of view.desktop?.networkHosts ?? []) hosts.add(host);
  }
  // Rust sorts `String`s bytewise; hostnames are ASCII, so code-unit order is identical.
  return [...hosts].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
}

/** Files that must exist at install: `main.entry`, each `desktop.entry`, `sniffRules`. */
export function packageFilesOf(manifest: Obj): string[] {
  const files: string[] = [];
  const main = manifest.main;
  if (isObject(main) && typeof main.entry === "string") files.push(main.entry);
  const contributes = manifest.contributes;
  if (isObject(contributes)) {
    if (Array.isArray(contributes.views)) {
      for (const view of contributes.views) {
        if (isObject(view) && isObject(view.desktop) && typeof view.desktop.entry === "string") {
          files.push(view.desktop.entry);
        }
      }
    }
    if (typeof contributes.sniffRules === "string") files.push(contributes.sniffRules);
  }
  return files;
}

/** Validates a sniff rule pack file: ≤256 KiB, valid JSON, right `format`, `rules` array ≤500. */
export function validateSniffPack(content: string | Uint8Array): ValidationResult {
  const errors: string[] = [];
  const size = typeof content === "string" ? utf8Length(content) : content.byteLength;
  if (size > SNIFF_PACK_MAX_BYTES) {
    errors.push(`sniffRules 文件超过 ${SNIFF_PACK_MAX_BYTES} 字节`);
    return { ok: false, errors };
  }
  const text = typeof content === "string" ? content : new TextDecoder().decode(content);
  let pack: unknown;
  try {
    pack = JSON.parse(text);
  } catch (err) {
    errors.push(`sniffRules JSON 解析失败: ${(err as Error).message}`);
    return { ok: false, errors };
  }
  if (!isObject(pack) || pack.format !== SNIFF_PACK_FORMAT) {
    errors.push(`sniffRules format 须为 ${SNIFF_PACK_FORMAT}`);
  } else if (!Array.isArray(pack.rules)) {
    errors.push("sniffRules rules 须为数组");
  } else if (pack.rules.length > SNIFF_PACK_MAX_RULES) {
    errors.push(`sniffRules 至多 ${SNIFF_PACK_MAX_RULES} 条规则`);
  }
  return { ok: errors.length === 0, errors };
}
