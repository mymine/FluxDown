/**
 * 扩展内统一的错误 / 警告出口：替代 `console.error` / `console.warn`。
 *
 * - 记入扩展错误日志（`error-log-store.ts`），侧边栏「诊断」页查看；
 * - 控制台只以 `console.info` 打印，浏览器扩展管理页的「错误」列表不收录；
 * - `installErrorCapture` 接管本上下文由扩展代码抛出的未捕获异常 / 未处理 rejection（只认扩展自己的
 *   脚本：内容脚本里页面自己的错误不碰）。
 *
 * 每个入口（background、popup、options、sidepanel、内容脚本）启动时调用一次 `installErrorCapture`；
 * 之前产生的条目先缓存在内存里，安装后补发。记录失败只打 `console.debug`，绝不递归上报。
 */
import { browser } from "wxt/browser";
import { formatErrorArgs, type ErrorLevel, type ErrorReport, type ErrorSource } from "./error-log-store";

export const ERROR_LOG_RECORD = "error-log-record";
export const ERROR_LOG_CLEAR = "error-log-clear";

/** 扩展页面清空错误日志（写入只在 background）。 */
export async function clearErrorLog(): Promise<void> {
  const reply: unknown = await browser.runtime.sendMessage({ type: ERROR_LOG_CLEAR });
  if ((reply as { ok?: unknown } | undefined)?.ok !== true) throw new Error("error log clear failed");
}

type Recorder = (report: ErrorReport) => Promise<void>;

const MAX_PENDING = 50;
/** 同一上下文里相同文本的重复上报间隔下限（存储侧也会合并计数，这里只削减消息量）。 */
const REPEAT_SUPPRESS_MS = 1000;

let recorder: Recorder | null = null;
const pending: ErrorReport[] = [];
const lastSent = new Map<string, number>();

function record(level: ErrorLevel, args: readonly unknown[]): void {
  try {
    console.info(`[FluxDown ${level}]`, ...args);
    const report: ErrorReport = { level, message: formatErrorArgs(args) };
    const key = `${level}\u0000${report.message}`;
    const now = Date.now();
    const previous = lastSent.get(key);
    if (previous !== undefined && now - previous < REPEAT_SUPPRESS_MS) return;
    lastSent.set(key, now);
    if (lastSent.size > 200) lastSent.clear();
    if (!recorder) {
      if (pending.length < MAX_PENDING) pending.push(report);
      return;
    }
    deliver(recorder, report);
  } catch (e) {
    console.debug("[FluxDown] error log record failed:", e);
  }
}

function deliver(target: Recorder, report: ErrorReport): void {
  target(report).catch((e: unknown) => console.debug("[FluxDown] error log write failed:", e));
}

/** 记一条错误（参数同 `console.error`）。 */
export function logError(...args: unknown[]): void {
  record("error", args);
}

/** 记一条警告（参数同 `console.warn`）。 */
export function logWarn(...args: unknown[]): void {
  record("warn", args);
}

/** 扩展自己的脚本地址前缀（chrome-extension:// 或 moz-extension://）。 */
function extensionOrigin(): string {
  try {
    return browser.runtime.getURL("/");
  } catch {
    return "";
  }
}

function fromExtension(origin: string, ...locations: (string | undefined)[]): boolean {
  return origin !== "" && locations.some((l) => typeof l === "string" && l.includes(origin));
}

/**
 * 安装本上下文的错误出口。`background` 直接写 store（传 `write`）；其余上下文经 runtime 消息交给 background。
 * 同时接管扩展脚本的未捕获异常与未处理 rejection（`preventDefault` 阻止浏览器另行报告）。
 */
export function installErrorCapture(source: ErrorSource, write?: Recorder): void {
  recorder =
    write ??
    (async (report) => {
      const reply: unknown = await browser.runtime.sendMessage({ type: ERROR_LOG_RECORD, report });
      if ((reply as { ok?: unknown } | undefined)?.ok !== true) throw new Error(`error log rejected (${source})`);
    });
  for (const report of pending.splice(0)) deliver(recorder, report);

  // 同一 JS world 里可能有多份本模块（顶层帧的 sensor 与面板是两个包）：全局监听只装一份，否则每个
  // 异常记两次。扩展更新后旧实例的上下文失效（runtime.id 变 undefined），由新实例重新接管。
  const shared = globalThis as { __fluxdownErrorCaptureAlive?: () => boolean };
  if (shared.__fluxdownErrorCaptureAlive?.() === true) return;
  shared.__fluxdownErrorCaptureAlive = () => {
    try {
      return browser.runtime?.id !== undefined;
    } catch {
      return false;
    }
  };
  const origin = extensionOrigin();
  globalThis.addEventListener?.("error", (event: ErrorEvent) => {
    if (!fromExtension(origin, event.filename, (event.error as Error | undefined)?.stack)) return;
    event.preventDefault();
    record("error", ["Uncaught", event.error ?? event.message]);
  });
  globalThis.addEventListener?.("unhandledrejection", (event: PromiseRejectionEvent) => {
    const reason: unknown = event.reason;
    if (!fromExtension(origin, reason instanceof Error ? reason.stack : undefined)) return;
    event.preventDefault();
    record("error", ["Unhandled rejection", reason]);
  });
}
