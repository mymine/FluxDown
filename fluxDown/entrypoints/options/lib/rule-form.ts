/**
 * 本地规则的「表单模式」与 JSON 之间的转换、id 生成与校验。纯函数，无 DOM / browser 依赖。
 * 表单只覆盖 response（识别 / 忽略）与 normalize（文件名 / 去参数）两类规则；
 * 其余 stage 或带高级字段的规则只能在 JSON 模式编辑（ruleToForm 返回 null）。
 */
import {
  RULE_ENGINE_VERSION,
  type MatchSpec,
  type ResourceType,
  type RulePack,
  type SniffRuleV1,
} from "@/rules/schema";
import { PackRejectedError, validatePack } from "@/rules/validate";
import { normalizeDomain } from "@/utils/domain-exclusion";
import type { MessageKey } from "@/utils/i18n";
import type { RuleIssue } from "./rule-issue";

export type FormStage = "response" | "normalize";
export type FormAction = "resource" | "ignore" | "filename" | "strip-params";

export interface RuleForm {
  title: string;
  stage: FormStage;
  host: string[];
  pageHost: string[];
  url: string[];
  ext: string[];
  mime: string[];
  requestTypes: string[];
  minSizeMB: string;
  maxSizeMB: string;
  action: FormAction;
  resourceType: ResourceType;
  params: string[];
  /** 空 = 不设置（引擎按 0 处理）。 */
  priority: string;
  /** 命中这些条件时规则不生效（每个条目独立，任一命中即排除）。 */
  excludeHost: string[];
  excludeUrl: string[];
}

export const RESOURCE_TYPE_OPTIONS: ResourceType[] = [
  "video",
  "audio",
  "document",
  "archive",
  "image",
  "executable",
  "torrent",
  "stream",
  "subtitle",
  "magnet",
  "other",
];

/** webRequest ResourceType 中对嗅探有意义的取值（MAIN world 观察到的请求记作 xmlhttprequest）。 */
export const REQUEST_TYPE_OPTIONS = [
  "media",
  "xmlhttprequest",
  "main_frame",
  "sub_frame",
  "image",
  "script",
  "object",
  "other",
] as const;

const FORM_MATCH_KEYS: Record<string, true> = {
  host: true,
  pageHost: true,
  url: true,
  ext: true,
  mime: true,
  resourceType: true,
  minSize: true,
  maxSize: true,
};

const BYTES_PER_MB = 1024 * 1024;

export function emptyForm(): RuleForm {
  return {
    title: "",
    stage: "response",
    host: [],
    pageHost: [],
    url: [],
    ext: [],
    mime: [],
    requestTypes: [],
    minSizeMB: "",
    maxSizeMB: "",
    action: "resource",
    resourceType: "video",
    params: [],
    priority: "",
    excludeHost: [],
    excludeUrl: [],
  };
}

/** host / pageHost 条目：保留 `=` 精确匹配前缀，其余按域名归一化；无法解析返回空串（TagInput 会丢弃）。 */
export function normalizeHostPattern(raw: string): string {
  const exact = raw.trim().startsWith("=");
  const domain = normalizeDomain(exact ? raw.trim().slice(1) : raw);
  if (!domain) return "";
  return exact ? `=${domain}` : domain;
}

/** 扩展名：小写、去前导点。 */
export function normalizeRuleExt(raw: string): string {
  return raw.trim().toLowerCase().replace(/^\.+/, "");
}

export function formatMB(bytes: number | undefined): string {
  if (bytes === undefined) return "";
  return String(Number((bytes / BYTES_PER_MB).toFixed(3)));
}

/** exclude 条目只含单个 host 或 url 列表时可用表单表示；否则返回 null。 */
function parseExclude(exclude: MatchSpec[] | undefined): { host: string[]; url: string[] } | null {
  const out = { host: [] as string[], url: [] as string[] };
  for (const entry of exclude ?? []) {
    const keys = Object.keys(entry);
    if (keys.length !== 1) return null;
    if (keys[0] === "host" && entry.host) out.host.push(...entry.host);
    else if (keys[0] === "url" && entry.url) out.url.push(...entry.url);
    else return null;
  }
  return out;
}

/** 规则能否用表单表示；不能则返回 null（含 exclude / pathRegex / confidence 等高级字段）。 */
export function ruleToForm(rule: SniffRuleV1): RuleForm | null {
  if (rule.stage !== "response" && rule.stage !== "normalize") return null;
  if (rule.requires !== undefined || rule.classifyOnly !== undefined) return null;
  const excluded = parseExclude(rule.exclude);
  if (excluded === null) return null;
  if (!Object.keys(rule.match).every((k) => FORM_MATCH_KEYS[k])) return null;

  const form = emptyForm();
  form.title = rule.title ?? "";
  form.priority = rule.priority === undefined ? "" : String(rule.priority);
  form.excludeHost = excluded.host;
  form.excludeUrl = excluded.url;
  form.stage = rule.stage;
  const m = rule.match;
  form.host = m.host ?? [];
  form.pageHost = m.pageHost ?? [];
  form.url = m.url ?? [];
  form.ext = m.ext ?? [];
  form.mime = m.mime ?? [];
  form.requestTypes = m.resourceType ?? [];
  form.minSizeMB = formatMB(m.minSize);
  form.maxSizeMB = formatMB(m.maxSize);

  const action = rule.action;
  if (rule.stage === "response") {
    if (action.kind === "resource" && action.confidence === undefined) {
      form.action = "resource";
      form.resourceType = action.type;
      return form;
    }
    if (action.kind === "ignore") {
      form.action = "ignore";
      return form;
    }
    return null;
  }
  if (action.kind === "filename") {
    form.action = "filename";
    form.params = action.extFromQuery;
    return form;
  }
  if (action.kind === "strip-params") {
    form.action = "strip-params";
    form.params = action.params;
    return form;
  }
  return null;
}

export interface RuleBase {
  id: string;
  version: number;
  enabled?: boolean;
}

export type FormResult = { ok: true; rule: SniffRuleV1 } | { ok: false; errorKey: MessageKey };

/** 空串 → undefined；非法（NaN / 负数）→ null。 */
function parseMBToBytes(text: string): number | undefined | null {
  const trimmed = text.trim();
  if (!trimmed) return undefined;
  const value = Number(trimmed);
  if (!Number.isFinite(value) || value < 0) return null;
  return Math.round(value * BYTES_PER_MB);
}

export function formToRule(form: RuleForm, base: RuleBase): FormResult {
  const priorityText = form.priority.trim();
  const priority = priorityText === "" ? undefined : Number(priorityText);
  if (priority !== undefined && !Number.isInteger(priority)) return { ok: false, errorKey: "rules.form.errPriority" };
  const minSize = parseMBToBytes(form.minSizeMB);
  const maxSize = parseMBToBytes(form.maxSizeMB);
  if (minSize === null || maxSize === null) return { ok: false, errorKey: "rules.form.errSize" };
  if (minSize !== undefined && maxSize !== undefined && minSize > maxSize) {
    return { ok: false, errorKey: "rules.form.errSize" };
  }

  const match: MatchSpec = {};
  if (form.host.length) match.host = form.host;
  if (form.pageHost.length) match.pageHost = form.pageHost;
  if (form.url.length) match.url = form.url;
  if (form.ext.length) match.ext = form.ext;
  if (form.mime.length) match.mime = form.mime;
  if (form.requestTypes.length) match.resourceType = form.requestTypes;
  if (minSize !== undefined) match.minSize = minSize;
  if (maxSize !== undefined) match.maxSize = maxSize;
  if (Object.keys(match).length === 0) return { ok: false, errorKey: "rules.form.errNoMatch" };

  let action: SniffRuleV1["action"];
  if (form.stage === "response") {
    action =
      form.action === "ignore" ? { kind: "ignore" } : { kind: "resource", type: form.resourceType };
  } else {
    if (form.params.length === 0) return { ok: false, errorKey: "rules.form.errParams" };
    action =
      form.action === "strip-params"
        ? { kind: "strip-params", params: form.params }
        : { kind: "filename", extFromQuery: form.params };
  }

  const rule: SniffRuleV1 = {
    id: base.id,
    version: base.version,
    tier: "local",
    stage: form.stage,
    match,
    action,
  };
  const title = form.title.trim();
  if (title) rule.title = title;
  if (base.enabled === false) rule.enabled = false;
  if (priority !== undefined) rule.priority = priority;
  const exclude: MatchSpec[] = [];
  if (form.excludeHost.length) exclude.push({ host: form.excludeHost });
  if (form.excludeUrl.length) exclude.push({ url: form.excludeUrl });
  if (exclude.length) rule.exclude = exclude;
  return { ok: true, rule };
}

/** `local.<slug>-<时间戳 base36>`；slug 仅 [a-z0-9-]，为空（如纯中文标题）则用 "rule"。 */
export function generateRuleId(title: string, now: number): string {
  const slug =
    title
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40)
      .replace(/-+$/g, "") || "rule";
  return `local.${slug}-${now.toString(36)}`;
}

const LOCAL_ID_RE = /^local\.[a-z0-9][a-z0-9._-]{1,62}$/;

export type CheckResult = { ok: true; rule: SniffRuleV1 } | { ok: false; errors: RuleIssue[] };

/** 把单条规则包进 local 包，用 validatePack 校验；逐条错误以字符串返回。 */
export function checkLocalRule(rule: SniffRuleV1, now: number): CheckResult {
  const pack: RulePack = {
    format: "fluxdown-sniff-pack",
    packId: "local",
    sequence: 0,
    engine: RULE_ENGINE_VERSION,
    issuedAt: new Date(now).toISOString(),
    rules: [rule],
  };
  try {
    const { pack: checked, rejected } = validatePack(pack, "local");
    if (checked.rules.length === 1) return { ok: true, rule: checked.rules[0] };
    return {
      ok: false,
      errors: rejected.length
        ? rejected.map((r) => ({ code: r.code, param: r.param, detail: r.reason }))
        : [{ detail: "invalid rule" }],
    };
  } catch (e) {
    if (e instanceof PackRejectedError) return { ok: false, errors: [{ detail: e.message }] };
    throw e;
  }
}

export interface FinalizeOptions {
  /** 正在编辑的已存规则（保持 id、version+1）；新建为 null。 */
  editing: SniffRuleV1 | null;
  takenIds: ReadonlySet<string>;
  now: number;
}

/**
 * 把草稿（JSON 解析结果或表单产物）定型为可保存的本地规则：tier 固定 local；
 * 编辑沿用原 id 且 version+1；新建 version=1，id 取草稿里合法且未占用的 local id，否则自动生成。
 */
export function finalizeLocalRule(draft: unknown, opts: FinalizeOptions): CheckResult {
  if (typeof draft !== "object" || draft === null || Array.isArray(draft)) {
    return { ok: false, errors: [{ code: "not_object", detail: "rule must be a JSON object" }] };
  }
  const raw = draft as Record<string, unknown>;
  let id: string;
  if (opts.editing) {
    id = opts.editing.id;
  } else if (typeof raw.id === "string" && LOCAL_ID_RE.test(raw.id) && !opts.takenIds.has(raw.id)) {
    id = raw.id;
  } else {
    const title = typeof raw.title === "string" ? raw.title : "";
    id = generateRuleId(title, opts.now);
    let bump = opts.now;
    while (opts.takenIds.has(id)) id = generateRuleId(title, ++bump);
  }
  const rule = {
    ...raw,
    id,
    version: opts.editing ? opts.editing.version + 1 : 1,
    tier: "local",
  } as unknown as SniffRuleV1;
  return checkLocalRule(rule, opts.now);
}

/** 新建规则时 JSON 模式的初始骨架（body / dom / page / request 只能在 JSON 模式编写）。 */
export function ruleTemplate(stage: SniffRuleV1["stage"], id: string): SniffRuleV1 {
  const base = { id, version: 1, tier: "local" as const };
  if (stage === "body") {
    return {
      ...base,
      stage,
      match: { url: ["*/api/example*"] },
      action: {
        kind: "extract",
        each: "data.items",
        fields: { url: "url", name: "name" },
        emit: { mode: "resource", url: "${url}", filename: "${name}" },
      },
    };
  }
  if (stage === "dom") {
    return {
      ...base,
      stage,
      match: { pageHost: ["example.com"] },
      action: {
        kind: "dom",
        selector: "a.download",
        attr: "href",
        emit: { mode: "resource", url: "${value}" },
      },
    };
  }
  if (stage === "page") {
    return {
      ...base,
      stage,
      match: { pageHost: ["example.com"] },
      action: { kind: "title", selector: ".video-title" },
    };
  }
  if (stage === "request") {
    return {
      ...base,
      stage,
      match: { host: ["cdn.example.com"] },
      action: { kind: "headers", set: { referer: "https://www.example.com/" } },
    };
  }
  return {
    ...base,
    stage: "response",
    match: { host: ["example.com"], ext: ["mp4"] },
    action: { kind: "resource", type: "video" },
  };
}
