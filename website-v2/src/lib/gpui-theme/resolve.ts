/**
 * 主题解析：`ThemeDocument` + 模式 → 每个注册 token 的最终值（与 Rust
 * `crates/theme/src/resolve.rs` 同算法，规则见 `registry.json` 的 `resolution`）。
 *
 * 每个 token 依次尝试：`<mode>` 层 → `tokens` 共享层 → `extends` 基底（Base token）或
 * 注册表默认表达式。某层取值非法（类型错、语法错、悬空引用）或引用成环时记诊断并回退到
 * 下一层；数值最后按注册表范围 clamp，行高另受「不小于同角色字号」约束。kitBound token
 * 忽略文件覆盖。求值按注册表顺序进行，结果记忆化。强调色 / 主色对比度 / ui_scale 仅运行时生效。
 */
import { REGISTRY, TOKENS, tokenSpec, type DefaultExpr, type TokenSpec } from "./registry";
import { getPath } from "./document";
import { parseTheme } from "./parse";
import type {
  Diagnostic,
  DiagnosticKind,
  FlatTokens,
  JsonObject,
  Hsla,
  ThemeDocument,
  ThemeMode,
  TokenValue,
} from "./types";
import {
  TRANSPARENT,
  f32Shortest,
  fitsKind,
  mixSrgb,
  parseHexColor,
  parseWire,
  tokenValueJson,
  withMinContrast,
  withOpacity,
  type Reference,
  type WireValue,
} from "./value";

/** 单个模式全部 token 的解析结果（未缩放），按注册表顺序。 */
export type ResolvedTokens = Map<string, TokenValue>;

export interface ResolveResult {
  tokens: ResolvedTokens;
  diagnostics: Diagnostic[];
}

/** 引用成环：正在求值的 token 被再次请求。 */
class Cycle {}

const baseCache = new Map<string, Map<string, TokenValue>>();

/** `extends` 基底在某模式下的预设值（Base token 及预设固定的非 Base token；来自 registry 的 builtins 快照）。 */
function baseValues(extendsValue: string, mode: ThemeMode): Map<string, TokenValue> {
  const cacheKey = `${extendsValue}:${mode}`;
  const cached = baseCache.get(cacheKey);
  if (cached) return cached;
  const values = new Map<string, TokenValue>();
  for (const [path, raw] of Object.entries(REGISTRY.builtins[extendsValue]?.[mode] ?? {})) {
    const spec = tokenSpec(path);
    if (!spec) continue;
    const parsed = parseWire(spec.kind, raw);
    if (parsed.ok && parsed.value.type === "literal") {
      values.set(path, parsed.value.value);
    } else if (parsed.ok && parsed.value.type === "shadow") {
      values.set(path, {
        type: "shadow",
        layers: parsed.value.layers.map((layer) => ({
          ...layer,
          color: layer.color.type === "literal" ? layer.color.color : TRANSPARENT,
        })),
      });
    }
  }
  baseCache.set(cacheKey, values);
  return values;
}

function literalValue(values: { dark: string | number; light: string | number }, mode: ThemeMode): TokenValue {
  const value = values[mode];
  if (typeof value === "number") return { type: "number", value: Math.fround(value) };
  return { type: "color", color: parseHexColor(value) ?? TRANSPARENT };
}

class Resolver {
  readonly memo = new Map<string, TokenValue>();
  readonly stack: string[] = [];
  readonly diagnostics: Diagnostic[] = [];

  constructor(
    readonly mode: ThemeMode,
    readonly layers: readonly [string, JsonObject][],
    readonly base: Map<string, TokenValue>,
  ) {}

  /** 抛出 `Cycle` 表示成环。 */
  eval(spec: TokenSpec): TokenValue {
    const memoized = this.memo.get(spec.path);
    if (memoized) return memoized;
    if (this.stack.includes(spec.path)) throw new Cycle();
    this.stack.push(spec.path);
    let result: [TokenValue, string | undefined];
    try {
      result = this.evalUncached(spec);
    } finally {
      this.stack.pop();
    }
    const value = this.constrain(spec, result[0], result[1]);
    this.memo.set(spec.path, value);
    return value;
  }

  evalUncached(spec: TokenSpec): [TokenValue, string | undefined] {
    for (const [layerKey, layer] of this.layers) {
      const raw = getPath(layer, spec.path);
      if (raw === undefined) continue;
      const layerPath = `${layerKey}.${spec.path}`;
      if (spec.kitBound) {
        this.diagnose(
          layerPath,
          "invalidValue",
          "该组件圆角由 gpui-component 跟随 radius.md / radius.lg，文件覆盖被忽略",
        );
        continue;
      }
      const wire = parseWire(spec.kind, raw);
      if (!wire.ok) {
        this.diagnose(layerPath, "invalidValue", wire.error);
        continue;
      }
      try {
        const value = this.evalWire(spec, wire.value, layerPath);
        if (value) return [value, layerPath];
      } catch (error) {
        if (!(error instanceof Cycle)) throw error;
        this.diagnose(layerPath, "refCycle", "引用成环，回退到下一层");
      }
    }
    return [this.evalDefault(spec, spec.default), undefined];
  }

  /** 文件值 → token 值；`undefined` 表示非法（已记诊断）。 */
  evalWire(spec: TokenSpec, wire: WireValue, layerPath: string): TokenValue | undefined {
    switch (wire.type) {
      case "literal":
        return wire.value;
      case "ref": {
        const value = this.evalReference(wire.ref, layerPath);
        if (!value) return undefined;
        if (fitsKind(value, spec.kind)) return value;
        this.diagnose(layerPath, "invalidValue", `引用 ${wire.ref.path} 的类型与 ${spec.kind} 不符`);
        return undefined;
      }
      case "shadow": {
        const layers = [];
        for (const layer of wire.layers) {
          let color: Hsla;
          if (layer.color.type === "literal") {
            color = layer.color.color;
          } else {
            const value = this.evalReference(layer.color.ref, layerPath);
            if (!value) return undefined;
            if (value.type !== "color") {
              this.diagnose(layerPath, "invalidValue", `阴影颜色引用 ${layer.color.ref.path} 不是颜色`);
              return undefined;
            }
            color = value.color;
          }
          layers.push({ x: layer.x, y: layer.y, blur: layer.blur, spread: layer.spread, color, inset: layer.inset });
        }
        return { type: "shadow", layers };
      }
    }
  }

  /** 解析引用（含透明度）；悬空引用记诊断返回 `undefined`。 */
  evalReference(reference: Reference, layerPath: string): TokenValue | undefined {
    const target = tokenSpec(reference.path);
    if (!target) {
      this.diagnose(layerPath, "invalidValue", `悬空引用 {${reference.path}}`);
      return undefined;
    }
    const value = this.eval(target);
    if (value.type === "color" && reference.opacity !== undefined) {
      return { type: "color", color: withOpacity(value.color, reference.opacity) };
    }
    return value;
  }

  evalDefault(spec: TokenSpec, expr: DefaultExpr): TokenValue {
    // 预设基底的值优先于默认表达式（Base token 与预设固定的非 Base token），同 Rust `eval_default`。
    const preset = this.base.get(spec.path);
    if (preset) return preset;
    switch (expr.kind) {
      case "base":
        return this.fallback(spec);
      case "ref":
        return this.evalPath(expr.path);
      case "refAlpha": {
        const value = this.evalPath(expr.path);
        return value.type === "color" ? { type: "color", color: withOpacity(value.color, expr.alpha) } : value;
      }
      case "refOffset": {
        const value = this.evalPath(expr.path);
        return value.type === "number"
          ? { type: "number", value: Math.fround(value.value + Math.fround(expr.offset)) }
          : value;
      }
      case "mix": {
        const from = this.evalPath(expr.from);
        const to = this.evalPath(expr.to);
        return {
          type: "color",
          color: mixSrgb(
            from.type === "color" ? from.color : TRANSPARENT,
            to.type === "color" ? to.color : TRANSPARENT,
            expr.amount[this.mode],
          ),
        };
      }
      case "contrast": {
        const color = this.evalPath(expr.color);
        const against = this.evalPath(expr.against);
        return {
          type: "color",
          color: withMinContrast(
            color.type === "color" ? color.color : TRANSPARENT,
            against.type === "color" ? against.color : TRANSPARENT,
            expr.min,
          ),
        };
      }
      case "literal":
        return literalValue(expr.value, this.mode);
      case "byMode":
        return this.evalDefault(spec, expr[this.mode]);
    }
  }

  evalPath(path: string): TokenValue {
    const target = tokenSpec(path);
    return target ? this.eval(target) : { type: "number", value: 0 };
  }

  /** 所有候选都因成环失败时的兜底：基底值或默认表达式中不含引用的部分。 */
  fallback(spec: TokenSpec): TokenValue {
    const base = this.base.get(spec.path);
    if (base) return base;
    if (spec.default.kind === "literal") return literalValue(spec.default.value, this.mode);
    switch (spec.kind) {
      case "color":
        return { type: "color", color: TRANSPARENT };
      case "fontFamily":
        return { type: "font", value: "" };
      case "shadow":
        return { type: "shadow", layers: [] };
      default:
        return { type: "number", value: 0 };
    }
  }

  /** 范围 clamp 与「行高不小于字号」。 */
  constrain(spec: TokenSpec, value: TokenValue, source: string | undefined): TokenValue {
    if (value.type !== "number") return value;
    const original = value.value;
    let number = original;
    if (spec.range) number = Math.min(Math.max(number, spec.range.min), spec.range.max);
    if (spec.path.endsWith(".lineHeight")) {
      const sizeSpec = tokenSpec(`${spec.path.slice(0, -".lineHeight".length)}.size`);
      if (sizeSpec) {
        try {
          const size = this.eval(sizeSpec);
          if (size.type === "number") number = Math.max(number, size.value);
        } catch (error) {
          if (!(error instanceof Cycle)) throw error;
        }
      }
    }
    if (number !== original) {
      this.diagnose(
        source ?? spec.path,
        "outOfRange",
        `${f32Shortest(original)} 越界，已 clamp 为 ${f32Shortest(number)}`,
      );
    }
    return { type: "number", value: number };
  }

  diagnose(path: string, kind: DiagnosticKind, message: string): void {
    this.diagnostics.push({ path, kind, message });
  }
}

/** 诊断全等（去重用）。 */
export const sameDiagnostic = (a: Diagnostic, b: Diagnostic) =>
  a.path === b.path && a.kind === b.kind && a.message === b.message;

/** 解析指定模式的全部 token。 */
export function resolveTheme(document: ThemeDocument, mode: ThemeMode): ResolveResult {
  const diagnostics: Diagnostic[] = [];
  let base = REGISTRY.defaultExtends;
  if (document.extends !== undefined) {
    if (REGISTRY.extends.includes(document.extends)) {
      base = document.extends;
    } else {
      diagnostics.push({
        path: "extends",
        kind: "unknownExtends",
        message: `未知基底 ${document.extends}，回退 builtin:default`,
      });
    }
  }
  const resolver = new Resolver(
    mode,
    [
      [mode, document[mode]],
      ["tokens", document.tokens],
    ],
    baseValues(base, mode),
  );
  resolver.diagnostics.push(...diagnostics);
  const tokens: ResolvedTokens = new Map();
  for (const spec of TOKENS) {
    let value: TokenValue;
    try {
      value = resolver.eval(spec);
    } catch (error) {
      if (!(error instanceof Cycle)) throw error;
      value = resolver.fallback(spec);
    }
    tokens.set(spec.path, value);
  }
  const unique: Diagnostic[] = [];
  for (const diagnostic of resolver.diagnostics) {
    if (!unique.some((seen) => sameDiagnostic(seen, diagnostic))) unique.push(diagnostic);
  }
  return { tokens, diagnostics: unique };
}

/** 按注册表路径扁平化：`{ "colors.primary": "#3b82f6ff", ... }`。 */
export function flatTokens(tokens: ResolvedTokens): FlatTokens {
  const flat: FlatTokens = {};
  for (const [path, value] of tokens) flat[path] = tokenValueJson(value);
  return flat;
}

export interface ResolvedSnapshot {
  diagnostics?: Diagnostic[];
  dark?: FlatTokens;
  light?: FlatTokens;
  error?: string;
}

/**
 * 与 Rust `resolved_snapshot` 同形：解析文件后两个模式的扁平结果与全部诊断
 * （解析诊断在前，其后按 dark → light 追加未出现过的解析期诊断）。
 */
export function resolvedSnapshot(text: string): ResolvedSnapshot {
  const parsed = parseTheme(text);
  if (!parsed.ok) return { error: parsed.error.message };
  const diagnostics = [...parsed.diagnostics];
  const dark = resolveTheme(parsed.document, "dark");
  const light = resolveTheme(parsed.document, "light");
  for (const diagnostic of [...dark.diagnostics, ...light.diagnostics]) {
    if (!diagnostics.some((seen) => sameDiagnostic(seen, diagnostic))) diagnostics.push(diagnostic);
  }
  return { diagnostics, dark: flatTokens(dark.tokens), light: flatTokens(light.tokens) };
}
