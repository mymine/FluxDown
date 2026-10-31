/**
 * 响应头嗅探：
 * 1. 主框架导航响应：下载类响应缓存到 responseDownloadCache（供拦截层使用），并加入嗅探列表；
 * 2. media / xhr / object / other / sub_frame 响应：交给规则引擎 `classifyResponse` 判定。
 */
import { browser } from "wxt/browser";
import type { Browser } from "wxt/browser";
import { parseContentDispositionFilename, readDispositionHeader } from "@/utils/filename";
import { hostOf } from "@/rules/url";
import { isDownloadContentType, responseDownloadCache, urlHost } from "./intercept";
import type { ResponseDownloadInfo } from "./intercept";
import { extractAuthFromCache } from "./request-cache";
import { recordRuleHit } from "./rule-hits";
import { getEngine } from "./rules-store";
import { peekCachedProfiles, peekCachedSettings } from "./settings-cache";
import { getTabPageUrl, ingestResources } from "./tab-state";
import { profileFor } from "@/utils/site-profiles";
import { logWarn } from "@/utils/error-log";

interface ParsedHeaders {
  contentType: string;
  contentLength: number;
  contentDisposition: string;
  dispositionByteCarrier: boolean;
  /** 小写名 → 值（供规则的 header 匹配）。 */
  all: Record<string, string>;
  /** 已从 Content-Range 取得总长（或确认未知），Content-Length 不再覆盖。 */
  rangeTotalKnown: boolean;
}

function parseHeaders(headers: Browser.webRequest.HttpHeader[]): ParsedHeaders {
  const parsed: ParsedHeaders = {
    contentType: "",
    contentLength: -1,
    contentDisposition: "",
    dispositionByteCarrier: false,
    all: {},
    rangeTotalKnown: false,
  };
  for (const h of headers) {
    const name = h.name.toLowerCase();
    if (h.value) parsed.all[name] = h.value;
    if (name === "content-type" && h.value) {
      parsed.contentType = h.value.split(";")[0].trim().toLowerCase();
    } else if (name === "content-length" && h.value) {
      const n = parseInt(h.value, 10);
      if (!isNaN(n) && !parsed.rangeTotalKnown) parsed.contentLength = n;
    } else if (name === "content-range" && h.value) {
      // 206 分段响应的 Content-Length 只是本段长度（播放器 Range 读取常只有几 KB）；
      // 文件总长在 `bytes a-b/total` 里，未知（`*`）时视为大小未知。
      const total = /\/\s*(\d+)\s*$/.exec(h.value);
      parsed.contentLength = total ? parseInt(total[1], 10) : -1;
      parsed.rangeTotalKnown = true;
    } else if (name === "content-disposition") {
      const d = readDispositionHeader(
        { value: h.value, binaryValue: h.binaryValue ? new Uint8Array(h.binaryValue) : undefined },
        import.meta.env.FIREFOX,
      );
      if (d) {
        parsed.contentDisposition = d.text;
        parsed.dispositionByteCarrier = d.byteCarrier;
      }
    }
  }
  return parsed;
}

/** 全局开关与站点配置 sniff 都允许；缓存冷启动为 null 时放行，避免同步路径丢事件。 */
function sniffAllowed(tabId: number, pageUrl: string): boolean {
  const settings = peekCachedSettings();
  if (settings && !settings.resourceSniffing) return false;
  const profiles = peekCachedProfiles();
  if (profiles) {
    const host = hostOf(getTabPageUrl(tabId) || pageUrl);
    if (host && !profileFor(profiles, host).profile.sniff) return false;
  }
  return true;
}

function onMainFrameResponse(details: Browser.webRequest.OnHeadersReceivedDetails): void {
  // 只关注主框架导航（sub_frame、xhr 等交给正常 download 流程处理）
  if (details.type !== "main_frame" || !details.responseHeaders) return;
  const h = parseHeaders(details.responseHeaders);
  const isAttachment = h.contentDisposition.toLowerCase().startsWith("attachment");
  if (!isAttachment && !isDownloadContentType(h.contentType)) return;

  const dispositionFilename = parseContentDispositionFilename(
    h.contentDisposition,
    h.dispositionByteCarrier,
    { host: urlHost(details.url) },
  );
  const info: ResponseDownloadInfo = {
    url: details.url,
    contentType: h.contentType,
    contentLength: h.contentLength,
    dispositionFilename,
    disposition: h.contentDisposition
      ? { text: h.contentDisposition, byteCarrier: h.dispositionByteCarrier }
      : undefined,
    tabId: details.tabId,
    ts: Date.now(),
  };
  responseDownloadCache.set(details.url, info);
  // 60 秒后自动清理
  setTimeout(() => responseDownloadCache.delete(details.url), 60_000);

  // 同时把 main_frame 下载资源加入嗅探面板（responseDownloadCache 服务于下载拦截，
  // 不受嗅探开关影响，仅面板入口受控）。
  if (details.tabId < 0 || !sniffAllowed(details.tabId, details.url)) return;
  const auth = extractAuthFromCache(details.url);
  ingestResources(
    details.tabId,
    "",
    [
      {
        url: details.url,
        type: getEngine().classify(details.url, h.contentType),
        filename: dispositionFilename,
        size: h.contentLength > 0 ? h.contentLength : undefined,
        mimeType: h.contentType,
        detectedBy: "webRequest",
        isAttachment,
      },
    ],
    { frameId: details.frameId, cookies: auth.cookies, headers: auth.headers },
  );
}

function onSniffResponse(details: Browser.webRequest.OnHeadersReceivedDetails): void {
  if (details.tabId < 0 || !details.responseHeaders) return;
  // 跳过非成功响应（重定向、客户端/服务器错误）
  if (details.statusCode < 200 || details.statusCode >= 400) return;
  const { documentUrl, originUrl } = details as Browser.webRequest.OnHeadersReceivedDetails & {
    documentUrl?: string;
    originUrl?: string;
  };
  const pageUrl = documentUrl || originUrl || "";
  if (!sniffAllowed(details.tabId, pageUrl)) return;

  const h = parseHeaders(details.responseHeaders);
  const isAttachment = h.contentDisposition.toLowerCase().startsWith("attachment");
  const engine = getEngine();
  const effectivePageUrl = getTabPageUrl(details.tabId) || pageUrl;
  const decision = engine.classifyResponse({
    url: details.url,
    pageUrl: effectivePageUrl,
    resourceType: details.type,
    mime: h.contentType,
    size: h.contentLength,
    status: details.statusCode,
    headers: h.all,
    attachment: isAttachment,
  });
  // 命中统计含 ignore 决定：回答「为什么没识别到 X」，也用于清理从不命中的死规则。
  if (decision.decision !== "none") recordRuleHit(decision.rule.id);
  if (decision.decision === "ignore") return;
  // 既非规则命中、又非附件 → 丢弃
  if (decision.decision === "none" && !isAttachment) return;

  const type = decision.decision === "resource" ? decision.type : engine.classify(details.url, h.contentType);
  const filename = h.contentDisposition
    ? parseContentDispositionFilename(h.contentDisposition, h.dispositionByteCarrier, {
        host: urlHost(details.url),
      })
    : "";
  // 随资源一起持久存储认证信息：用户稍后点下载时 requestHeaderCache 可能已过期。
  const auth = extractAuthFromCache(details.url);
  ingestResources(
    details.tabId,
    "",
    [
      {
        url: details.url,
        type,
        filename,
        size: h.contentLength > 0 ? h.contentLength : undefined,
        mimeType: h.contentType,
        detectedBy: "webRequest",
        isAttachment,
      },
    ],
    {
      frameId: details.frameId,
      ruleIds: decision.decision === "resource" ? [decision.rule.id] : undefined,
      cookies: auth.cookies,
      headers: auth.headers,
    },
  );
}

/** 同步注册响应头监听；由 startBackground 调用一次。 */
export function installSniff(): void {
  try {
    browser.webRequest.onHeadersReceived.addListener(
      (details) => {
        onMainFrameResponse(details);
        return undefined;
      },
      // 让浏览器侧过滤：不带 types 时每个图片 / 脚本响应的头都会送进扩展再被丢弃
      { urls: ["<all_urls>"], types: ["main_frame"] },
      ["responseHeaders"],
    );
    browser.webRequest.onHeadersReceived.addListener(
      (details) => {
        onSniffResponse(details);
        return undefined;
      },
      {
        urls: ["<all_urls>"],
        types: ["media", "xmlhttprequest", "object", "other", "sub_frame"],
      },
      ["responseHeaders"],
    );
  } catch (e) {
    logWarn("[FluxDown] Failed to register resource sniffer:", e);
  }
}
