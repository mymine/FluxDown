/**
 * 正则安全子集（解析式 ReDoS 防线）。
 *
 * 规则里的 pathRegex / header / track.pattern 会在 service worker 的 webRequest 同步路径上执行，
 * 一条灾难性回溯的正则会冻结整个嗅探。这里把正则源码解析成简化语法树后拒绝：
 *
 * - 嵌套量词：被 `*` / `+` / `{n,}` 修饰的分组内再含重复（`(a+)+`、`((a+))+`、`(?:a*)*`）；
 * - 歧义选择支：被重复的分组含 `|`，且各支不是「互不为前缀的纯字面量」（`(a|aa)+`、`(\w|\d)+`）；
 * - 相邻重叠量词：中间仅隔可空项的两个无界重复，字符集相交（`a*a*a*`、`\w+\d+`、`.*.*`）；
 * - 无界重复总数超过 {@link MAX_UNBOUNDED_QUANTIFIERS}；
 * - 反向引用 / 环视、超长、语法错误。
 *
 * 另外，非官方层级的规则在执行时还受输入长度上限 {@link REGEX_INPUT_CAP} 约束（见 engine.ts）。
 */

export const MAX_REGEX_LEN = 256;
/** 非官方规则执行正则时的输入长度上限；更长的输入视为不匹配。 */
export const REGEX_INPUT_CAP = 2048;
const MAX_UNBOUNDED_QUANTIFIERS = 4;
/** `{n,m}` 的上界超过它按无界处理。 */
const BOUNDED_MAX = 100;

/** 字符集：0–127 为 ASCII 码位，128 代表「任一非 ASCII 字符」。 */
type CharSet = Uint8Array;
const SET_SIZE = 129;
const NON_ASCII = 128;

interface Item {
  node: Node;
  min: number;
  max: number;
}

type Node =
  | { k: "char"; set: CharSet; /** 精确字面量字符（小写）；类 / 点号为 null */ lit: string | null }
  | { k: "zero" }
  | { k: "group"; alts: Item[][] };

class Unsupported extends Error {}

function emptySet(): CharSet {
  return new Uint8Array(SET_SIZE);
}

function fullSet(): CharSet {
  return new Uint8Array(SET_SIZE).fill(1);
}

function addRange(set: CharSet, lo: number, hi: number): void {
  for (let c = lo; c <= hi; c++) set[c < NON_ASCII ? c : NON_ASCII] = 1;
}

function invert(set: CharSet): CharSet {
  const out = emptySet();
  for (let i = 0; i < SET_SIZE; i++) out[i] = set[i] ? 0 : 1;
  return out;
}

function overlaps(a: CharSet, b: CharSet): boolean {
  for (let i = 0; i < SET_SIZE; i++) if (a[i] && b[i]) return true;
  return false;
}

/** 大小写不敏感（`i` 标志）：补齐字母的另一种大小写。 */
function foldCase(set: CharSet): void {
  for (let c = 65; c <= 90; c++) {
    if (set[c] || set[c + 32]) {
      set[c] = 1;
      set[c + 32] = 1;
    }
  }
}

function classEscapeSet(letter: string): CharSet | null {
  const set = emptySet();
  switch (letter) {
    case "d":
    case "D":
      addRange(set, 48, 57);
      break;
    case "w":
    case "W":
      addRange(set, 48, 57);
      addRange(set, 65, 90);
      addRange(set, 97, 122);
      set[95] = 1;
      break;
    case "s":
    case "S":
      for (const c of [9, 10, 11, 12, 13, 32]) set[c] = 1;
      set[NON_ASCII] = 1; // \u00a0 \u2028 等
      break;
    default:
      return null;
  }
  return letter === "D" || letter === "W" || letter === "S" ? invert(set) : set;
}

const HEX2 = /^[0-9a-fA-F]{2}/;
const HEX4 = /^[0-9a-fA-F]{4}/;
const BRACE_QUANT = /\{(\d+)(?:(,)(\d*))?\}/y;
const CONTROL_ESCAPES: Record<string, number> = { n: 10, r: 13, t: 9, f: 12, v: 11, "0": 0 };

class Parser {
  private pos = 0;

  constructor(private readonly src: string) {}

  parse(): Item[][] {
    const alts = this.parseAlts();
    if (this.pos !== this.src.length) throw new Unsupported("trailing");
    return alts;
  }

  private parseAlts(): Item[][] {
    const alts: Item[][] = [[]];
    while (this.pos < this.src.length) {
      const c = this.src[this.pos];
      if (c === ")") break;
      if (c === "|") {
        alts.push([]);
        this.pos++;
        continue;
      }
      const node = this.parseAtom();
      const { min, max } = this.parseQuantifier();
      alts[alts.length - 1].push({ node, min, max });
    }
    return alts;
  }

  private parseQuantifier(): { min: number; max: number } {
    const c = this.src[this.pos];
    let min = 1;
    let max = 1;
    if (c === "*") {
      min = 0;
      max = Infinity;
      this.pos++;
    } else if (c === "+") {
      max = Infinity;
      this.pos++;
    } else if (c === "?") {
      min = 0;
      this.pos++;
    } else if (c === "{") {
      BRACE_QUANT.lastIndex = this.pos;
      const m = BRACE_QUANT.exec(this.src);
      if (m) {
        min = Number(m[1]);
        max = m[2] === undefined ? min : m[3] === "" ? Infinity : Number(m[3]);
        this.pos = BRACE_QUANT.lastIndex;
      }
    } else {
      return { min, max };
    }
    if (max > BOUNDED_MAX) max = Infinity;
    if (this.src[this.pos] === "?") this.pos++; // 惰性修饰
    return { min, max };
  }

  private parseAtom(): Node {
    const c = this.src[this.pos];
    switch (c) {
      case "(":
        return this.parseGroup();
      case "[":
        return this.parseClass();
      case "\\":
        return this.parseEscape();
      case ".": {
        this.pos++;
        const set = fullSet();
        set[10] = 0;
        set[13] = 0;
        return { k: "char", set, lit: null };
      }
      case "^":
      case "$":
        this.pos++;
        return { k: "zero" };
      default:
        this.pos++;
        return this.literal(c.charCodeAt(0));
    }
  }

  private literal(code: number): Node {
    const set = emptySet();
    set[code < NON_ASCII ? code : NON_ASCII] = 1;
    foldCase(set);
    return { k: "char", set, lit: String.fromCharCode(code).toLowerCase() };
  }

  private parseGroup(): Node {
    this.pos++; // (
    if (this.src[this.pos] === "?") {
      const next = this.src[this.pos + 1];
      if (next === ":") {
        this.pos += 2;
      } else if (next === "<" && this.src[this.pos + 2] !== "=" && this.src[this.pos + 2] !== "!") {
        const close = this.src.indexOf(">", this.pos);
        if (close < 0) throw new Unsupported("named group");
        this.pos = close + 1;
      } else {
        throw new Unsupported("lookaround");
      }
    }
    const alts = this.parseAlts();
    if (this.src[this.pos] !== ")") throw new Unsupported("unclosed group");
    this.pos++;
    return { k: "group", alts };
  }

  /** 解析转义的单个字符 / 类；`inClass` 时 `\b` 是退格。返回码位或字符集。 */
  private readEscape(inClass: boolean): { code: number } | { set: CharSet } | "zero" {
    this.pos++; // 反斜杠
    const e = this.src[this.pos];
    if (e === undefined) throw new Unsupported("dangling escape");
    this.pos++;
    const cls = classEscapeSet(e);
    if (cls) return { set: cls };
    if (e === "b") return inClass ? { code: 8 } : "zero";
    if (e === "B" && !inClass) return "zero";
    if (e === "k" || (e >= "1" && e <= "9")) throw new Unsupported("backreference");
    const ctrl = CONTROL_ESCAPES[e];
    if (ctrl !== undefined) return { code: ctrl };
    if (e === "x") {
      const m = HEX2.exec(this.src.slice(this.pos, this.pos + 2));
      if (m) {
        this.pos += 2;
        return { code: parseInt(m[0], 16) };
      }
    }
    if (e === "u") {
      const m = HEX4.exec(this.src.slice(this.pos, this.pos + 4));
      if (m) {
        this.pos += 4;
        return { code: parseInt(m[0], 16) };
      }
    }
    if (e === "c") {
      const letter = this.src[this.pos];
      if (letter !== undefined && /[a-zA-Z]/.test(letter)) {
        this.pos++;
        return { code: letter.charCodeAt(0) % 32 };
      }
    }
    return { code: e.charCodeAt(0) };
  }

  private parseEscape(): Node {
    const r = this.readEscape(false);
    if (r === "zero") return { k: "zero" };
    if ("set" in r) return { k: "char", set: r.set, lit: null };
    return this.literal(r.code);
  }

  private parseClass(): Node {
    this.pos++; // [
    let negate = false;
    if (this.src[this.pos] === "^") {
      negate = true;
      this.pos++;
    }
    const set = emptySet();
    while (this.pos < this.src.length && this.src[this.pos] !== "]") {
      const a = this.classAtom();
      // 范围 a-b（b 为 `]` 时 `-` 是字面量）。
      if (this.src[this.pos] === "-" && this.src[this.pos + 1] !== "]" && this.pos + 1 < this.src.length && "code" in a) {
        const save = this.pos;
        this.pos++;
        const b = this.classAtom();
        if ("code" in b) {
          addRange(set, Math.min(a.code, b.code), Math.max(a.code, b.code));
          continue;
        }
        // 端点是类转义：按字面量 a、`-`、类 b 处理。
        this.pos = save;
      }
      if ("code" in a) set[a.code < NON_ASCII ? a.code : NON_ASCII] = 1;
      else for (let i = 0; i < SET_SIZE; i++) if (a.set[i]) set[i] = 1;
    }
    if (this.src[this.pos] !== "]") throw new Unsupported("unclosed class");
    this.pos++;
    foldCase(set);
    return { k: "char", set: negate ? invert(set) : set, lit: null };
  }

  private classAtom(): { code: number } | { set: CharSet } {
    if (this.src[this.pos] === "\\") {
      const r = this.readEscape(true);
      if (r === "zero") throw new Unsupported("class escape");
      return r;
    }
    const code = this.src.charCodeAt(this.pos);
    this.pos++;
    return { code };
  }
}

/** 分组（含嵌套）里是否有任何重复（max > 1）。 */
function containsRepeat(alts: Item[][]): boolean {
  return alts.some((seq) =>
    seq.some((it) => it.max > 1 || (it.node.k === "group" && containsRepeat(it.node.alts))),
  );
}

/** 各支都是纯字面量序列且互不为前缀（⇒ 可唯一分解，不会歧义回溯）。 */
function isPrefixFreeLiterals(alts: Item[][]): boolean {
  const words: string[] = [];
  for (const seq of alts) {
    let w = "";
    for (const it of seq) {
      if (it.node.k !== "char" || it.node.lit === null || it.min !== 1 || it.max !== 1) return false;
      w += it.node.lit;
    }
    if (w === "") return false;
    words.push(w);
  }
  for (let i = 0; i < words.length; i++) {
    for (let j = 0; j < words.length; j++) {
      if (i !== j && words[j].startsWith(words[i])) return false;
    }
  }
  return true;
}

/** 返回问题代码；通过返回 null。`counter.n` 累计无界重复数。 */
function analyze(alts: Item[][], counter: { n: number }): string | null {
  for (const seq of alts) {
    let run: CharSet[] = [];
    for (const it of seq) {
      const { node, min, max } = it;
      if (node.k === "group") {
        if (max > 1) {
          if (max === Infinity && containsRepeat(node.alts)) return "nested_quantifier";
          if (node.alts.length > 1 && !isPrefixFreeLiterals(node.alts)) return "ambiguous_alternation";
        }
        const before = counter.n;
        const err = analyze(node.alts, counter);
        if (err) return err;
        if (max > 1 && max !== Infinity) counter.n += (counter.n - before) * (max - 1);
      }
      if (max === Infinity) {
        counter.n++;
        const set = node.k === "char" ? node.set : fullSet();
        if (run.some((r) => overlaps(r, set))) return "overlapping_quantifiers";
        run.push(set);
      } else if (node.k === "zero" || min === 0) {
        // 可空项：不隔断相邻性
      } else {
        run = [];
      }
      if (counter.n > MAX_UNBOUNDED_QUANTIFIERS) return "too_many_quantifiers";
    }
  }
  return null;
}

/** 正则不合格的原因代码；合格返回 null。 */
export function regexProblem(pattern: unknown): string | null {
  if (typeof pattern !== "string") return "not_string";
  if (pattern.length > MAX_REGEX_LEN) return "too_long";
  try {
    new RegExp(pattern, "i");
  } catch {
    return "syntax";
  }
  if (/\\[1-9]|\\k</.test(pattern)) return "backreference";
  if (/\(\?(?:=|!|<=|<!)/.test(pattern)) return "lookaround";
  try {
    const parser = new Parser(pattern);
    return analyze(parser.parse(), { n: 0 });
  } catch (e) {
    if (e instanceof Unsupported) return `unsupported_${e.message.replace(/\W+/g, "_")}`;
    throw e;
  }
}

/** 正则安全子集：长度、可编译、无反向引用 / 环视 / 灾难性回溯结构。 */
export function isSafeRegex(pattern: string): boolean {
  return regexProblem(pattern) === null;
}
