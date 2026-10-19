/**
 * URL glob 与 host 匹配。
 *
 * urlGlobMatch 逐行移植自 native/engine/src/plugin/manifest.rs 的
 * url_glob_match / glob_segments，两端语义必须保持一致。
 */
import { normalizeDomain } from "../utils/domain-exclusion";

/** 仅 ASCII 大写转小写（对应 Rust to_ascii_lowercase）；无大写字母时原样返回，不分配。 */
export function asciiLower(s: string): string {
  return /[A-Z]/.test(s) ? s.replace(/[A-Z]+/g, (m) => m.toLowerCase()) : s;
}

/** 编译后的 glob：入参必须是已经 {@link asciiLower} 的 URL。 */
export type GlobMatcher = (lowerUrl: string) => boolean;

/**
 * 编译 glob。`*` 是唯一通配符，按 `*` 分段顺序子串匹配（首段前缀锚定、尾段后缀锚定）；
 * pattern 在编译时小写化。纯 `*foo*`、`foo*`、`*foo` 走 includes / startsWith / endsWith。
 */
export function compileGlob(pattern: string): GlobMatcher {
  const p = asciiLower(pattern);
  // 无通配符 = 精确匹配。
  if (!p.includes("*")) return (t) => t === p;
  const parts = p.split("*");
  const n = parts.length;
  if (n === 2) {
    const [head, tail] = parts;
    if (head === "" && tail === "") return () => true;
    if (head === "") return (t) => t.endsWith(tail);
    if (tail === "") return (t) => t.startsWith(head);
    return (t) => t.length >= head.length + tail.length && t.startsWith(head) && t.endsWith(tail);
  }
  if (n === 3 && parts[0] === "" && parts[2] === "" && parts[1] !== "") {
    const mid = parts[1];
    return (t) => t.includes(mid);
  }
  return (t) => {
    let pos = 0;
    for (let i = 0; i < n; i++) {
      const part = parts[i];
      if (part === "") continue;
      if (i === 0) {
        // 首段前缀锚定。
        if (!t.startsWith(part)) return false;
        pos = part.length;
      } else if (i === n - 1) {
        // 尾段后缀锚定（在剩余串上）。
        if (t.length - pos < part.length || !t.endsWith(part)) return false;
      } else {
        // 中段：从 pos 起找首个出现。
        const idx = t.indexOf(part, pos);
        if (idx < 0) return false;
        pos = idx + part.length;
      }
    }
    return true;
  };
}

/** 一次性匹配（pattern 与文本都先小写化）；热路径请用 {@link compileGlob}。 */
export function urlGlobMatch(pattern: string, url: string): boolean {
  return compileGlob(pattern)(asciiLower(url));
}

/** 已解析的 host 模式。 */
export interface HostPattern {
  domain: string;
  /** `.` + domain，子域匹配用（编译期预算，避免逐次拼接） */
  suffix: string;
  /** true = 只匹配本域（"=example.com"） */
  exact: boolean;
}

/** 解析 host 模式；无效或使用 "*." 时返回 null。 */
export function parseHostPattern(pattern: string): HostPattern | null {
  const raw = pattern.trim();
  const exact = raw.startsWith("=");
  const body = exact ? raw.slice(1) : raw;
  if (body.trim().startsWith("*.")) return null;
  const domain = normalizeDomain(body);
  return domain ? { domain, suffix: `.${domain}`, exact } : null;
}

/** host（须已小写）是否命中已解析的模式（DNS 标签边界）。 */
export function hostMatchesLower(host: string, p: HostPattern): boolean {
  return p.exact ? host === p.domain : host === p.domain || host.endsWith(p.suffix);
}

/** host 是否命中已解析的模式（DNS 标签边界）；host 大小写不限。 */
export function hostMatchesPattern(host: string, p: HostPattern): boolean {
  return hostMatchesLower(host.toLowerCase(), p);
}

/** 便捷版：解析并匹配。 */
export function hostMatches(host: string, pattern: string): boolean {
  const p = parseHostPattern(pattern);
  return p !== null && hostMatchesPattern(host, p);
}
