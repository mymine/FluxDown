/**
 * GPUI 主题文件（`fluxdown.gpui-theme` v2）的 TS 镜像类型。
 *
 * 与 Rust `crates/theme/src/document.rs` 的 `ThemeDocument` 一一对应：所有字段可选，
 * token 层保持原始 JSON（非法值只在 resolve 时回退并记诊断），未知键进 `extra` 原样保留。
 */

export type Json = null | boolean | number | string | Json[] | JsonObject;
export interface JsonObject {
  [key: string]: Json;
}

export type ThemeMode = "dark" | "light";
/** token 层：`tokens`（两模式共享）/ `dark` / `light`。 */
export type TokenLayer = "tokens" | "dark" | "light";
export type ExportMode = "diff" | "full";

export const THEME_FORMAT = "fluxdown.gpui-theme";
export const THEME_SCHEMA_VERSION = 2;
export const THEME_SCHEMA_URL = "https://fluxdown.com/schemas/gpui-theme.v2.json";

export type DiagnosticKind =
  | "invalidValue"
  | "unknownKey"
  | "outOfRange"
  | "newerVersion"
  | "unknownExtends"
  | "refCycle"
  | "migrated";

/** 非致命问题；`path` 为文件内路径（如 `dark.colors.primary`、`meta.name`）。 */
export interface Diagnostic {
  path: string;
  kind: DiagnosticKind;
  message: string;
}

export interface ThemeMeta {
  id?: string;
  name?: string;
  author?: string;
  version?: string;
  description?: string;
  /** 未知 meta 键（原样保留）。 */
  extra: JsonObject;
}

export interface ThemeDocument {
  /** `$schema`。 */
  schema?: string;
  format?: string;
  /** 文件声明的版本（迁移后为当前版本；更高版本原样保留）。 */
  schemaVersion?: number;
  meta?: ThemeMeta;
  /** `builtin:*`；缺省为 `builtin:default`。 */
  extends?: string;
  tokens: JsonObject;
  dark: JsonObject;
  light: JsonObject;
  /** 顶层未知键（原样保留）。 */
  extra: JsonObject;
}

export type ThemeParseErrorKind = "invalidJson" | "notAnObject" | "unsupportedFormat";

export interface ThemeParseError {
  kind: ThemeParseErrorKind;
  message: string;
}

export type ParseResult =
  | { ok: true; document: ThemeDocument; diagnostics: Diagnostic[] }
  | { ok: false; error: ThemeParseError };

/**
 * 颜色，与 gpui `Hsla` 同表示（f32，h/s/l/a 均 0~1）。Rust 端颜色在内存中保持 Hsla，
 * 混色与输出都经 Hsla ↔ Rgba 的 f32 往返，TS 端逐步复刻以保证 `#rrggbbaa` 逐位一致。
 */
export interface Hsla {
  h: number;
  s: number;
  l: number;
  a: number;
}

export interface ShadowValue {
  x: number;
  y: number;
  blur: number;
  spread: number;
  color: Hsla;
  inset: boolean;
}

/** 已解析的单个 token 值。 */
export type TokenValue =
  | { type: "color"; color: Hsla }
  | { type: "number"; value: number }
  | { type: "font"; value: string }
  | { type: "shadow"; layers: ShadowValue[] };

/** 单个模式按注册表路径扁平化的 wire 值（与 Rust `to_flat_json` 同形）。 */
export type FlatTokens = Record<string, Json>;

export function isJsonObject(value: Json | undefined): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function emptyDocument(): ThemeDocument {
  return { tokens: {}, dark: {}, light: {}, extra: {} };
}

export function cloneDocument(document: ThemeDocument): ThemeDocument {
  return structuredClone(document);
}
