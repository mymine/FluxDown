/**
 * 扩展错误日志：所有上下文（background / 扩展页面 / 内容脚本）的错误与警告集中记到
 * `storage.local`，侧边栏「诊断」页展示，连接诊断导出时附带（导出前脱敏）。
 *
 * 错误不再走 `console.error` / 未捕获异常，因此不会出现在浏览器扩展管理页的「错误」列表里。
 * 只有 background 持有 store 实例并写入；其余上下文经 runtime 消息上报，避免并发读改写丢条目。
 */
export const ERROR_LOG_KEY = "extensionErrorLog";
export const MAX_ERROR_LOG_ENTRIES = 100;
export const ERROR_LOG_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_MESSAGE_CHARS = 2000;

export type ErrorLevel = "error" | "warn";
export type ErrorSource = "background" | "popup" | "options" | "sidepanel" | "content";
const LEVELS: readonly ErrorLevel[] = ["error", "warn"];
const SOURCES: readonly ErrorSource[] = ["background", "popup", "options", "sidepanel", "content"];

export interface ErrorReport {
  level: ErrorLevel;
  message: string;
}

export interface ErrorLogEntry extends ErrorReport {
  source: ErrorSource;
  firstAt: number;
  lastAt: number;
  count: number;
}

interface Storage {
  get(key: string): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
}

/** 把 console 风格的参数拼成一行文本：Error 取 name / message（含 cause），对象转 JSON。 */
export function formatErrorArgs(args: readonly unknown[]): string {
  const text = args.map(formatValue).join(" ");
  return sanitize(text);
}

function formatValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error) {
    const own = `${value.name}: ${value.message}`;
    return value.cause === undefined ? own : `${own} (cause: ${formatValue(value.cause)})`;
  }
  if (value === undefined) return "undefined";
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function sanitize(text: string): string {
  return text.replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, " ").slice(0, MAX_MESSAGE_CHARS);
}

function normalize(input: unknown): ErrorReport | null {
  if (!input || typeof input !== "object") return null;
  const value = input as Record<string, unknown>;
  if (!LEVELS.includes(value.level as ErrorLevel) || typeof value.message !== "string" || !value.message) return null;
  return { level: value.level as ErrorLevel, message: sanitize(value.message) };
}

export function isErrorSource(value: unknown): value is ErrorSource {
  return SOURCES.includes(value as ErrorSource);
}

export class ErrorLogStore {
  private tail: Promise<unknown> = Promise.resolve();

  constructor(
    private storage: Storage,
    private now = Date.now,
  ) {}

  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.tail.then(operation);
    // 一次存储失败不堵死队列；调用方仍拿到原始 rejection。
    this.tail = result.catch(() => undefined);
    return result;
  }

  private async read(): Promise<ErrorLogEntry[]> {
    const stored = (await this.storage.get(ERROR_LOG_KEY))[ERROR_LOG_KEY];
    if (!Array.isArray(stored)) return [];
    const cutoff = this.now() - ERROR_LOG_MAX_AGE_MS;
    return stored.slice(-MAX_ERROR_LOG_ENTRIES).flatMap((item) => {
      const report = normalize(item);
      if (
        !report ||
        !isErrorSource(item.source) ||
        !Number.isFinite(item.firstAt) ||
        !Number.isFinite(item.lastAt) ||
        item.firstAt > item.lastAt ||
        item.lastAt < cutoff ||
        !Number.isSafeInteger(item.count) ||
        item.count < 1
      ) {
        return [];
      }
      return [{ ...report, source: item.source, firstAt: item.firstAt, lastAt: item.lastAt, count: item.count }];
    });
  }

  /** 追加一条；与已有条目（同来源 / 级别 / 文本）重复时只累加计数并移到最新。 */
  append(input: unknown, source: ErrorSource): Promise<void> {
    const report = normalize(input);
    if (!report) return Promise.reject(new Error("Invalid error report"));
    return this.enqueue(async () => {
      const entries = await this.read();
      const now = this.now();
      const index = entries.findIndex(
        (e) => e.source === source && e.level === report.level && e.message === report.message,
      );
      if (index >= 0) {
        const [existing] = entries.splice(index, 1);
        existing.lastAt = now;
        existing.count = Math.min(Number.MAX_SAFE_INTEGER, existing.count + 1);
        entries.push(existing);
      } else {
        entries.push({ ...report, source, firstAt: now, lastAt: now, count: 1 });
      }
      await this.storage.set({ [ERROR_LOG_KEY]: entries.slice(-MAX_ERROR_LOG_ENTRIES) });
    });
  }

  snapshot(): Promise<ErrorLogEntry[]> {
    return this.enqueue(() => this.read());
  }

  clear(): Promise<void> {
    return this.enqueue(() => this.storage.set({ [ERROR_LOG_KEY]: [] }));
  }
}
