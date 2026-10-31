/**
 * background 侧的扩展错误日志：持有唯一的 `ErrorLogStore`，处理各上下文的上报与侧边栏的读取 / 清空。
 */
import { browser, type Browser } from "wxt/browser";
import { ERROR_LOG_CLEAR, ERROR_LOG_RECORD, installErrorCapture } from "./error-log";
import { ErrorLogStore, type ErrorLogEntry, type ErrorSource } from "./error-log-store";

export const ERROR_LOG_TYPES: readonly string[] = [ERROR_LOG_RECORD, ERROR_LOG_CLEAR];

const store = new ErrorLogStore(browser.storage.local);

/** startBackground 的第一步：之后 background 内的 logError / 未捕获异常都直接落库。 */
export function installBackgroundErrorLog(): void {
  installErrorCapture("background", (report) => store.append(report, "background"));
}

export function errorLogSnapshot(): Promise<ErrorLogEntry[]> {
  return store.snapshot();
}

type PageSource = Exclude<ErrorSource, "background" | "content">;

function senderSource(sender: Browser.runtime.MessageSender): ErrorSource | null {
  if (sender.id !== browser.runtime.id) return null;
  const page = sender.url?.split(/[?#]/, 1)[0];
  const pages: Record<string, PageSource> = {
    [browser.runtime.getURL("/popup.html")]: "popup",
    [browser.runtime.getURL("/options.html")]: "options",
    [browser.runtime.getURL("/sidepanel.html")]: "sidepanel",
  };
  // 设置页可能开在标签页里（sender.tab 也存在），先按页面地址认。
  const extensionPage = page ? pages[page] : undefined;
  if (extensionPage) return extensionPage;
  return sender.tab ? "content" : null;
}

/**
 * 处理 `ERROR_LOG_TYPES` 消息：内容脚本只能上报，清空只给扩展页面。
 * 读取不走消息：扩展页面直接只读 `storage.local`（`ErrorLogStore.snapshot`），写入只在这里。
 */
export async function handleErrorLogMessage(
  message: { type?: unknown; report?: unknown },
  sender: Browser.runtime.MessageSender,
): Promise<unknown> {
  const source = senderSource(sender);
  if (!source) throw new Error("Error log is only available to the extension");
  if (message.type === ERROR_LOG_RECORD) {
    await store.append(message.report, source);
    return { ok: true };
  }
  if (source === "content") throw new Error("Content scripts cannot clear the error log");
  await store.clear();
  return { ok: true };
}
