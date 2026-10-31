/**
 * 导航请求事务（method + body）记录的纯逻辑：按 URL 缓存、跨重定向传递、发送时查找。
 * webRequest 监听接线见 `request-cache.ts`。
 */

export type CapturedBody =
  | { kind: "formData"; fields: Record<string, string[]> }
  | { kind: "raw"; bytesB64: string; contentType?: string };

export interface RequestRecord {
  url: string;
  method: string;
  body?: CapturedBody;
  ts: number;
}

export type RequestRecordCache = Map<string, RequestRecord>;

/**
 * 记录一次非 GET（或带体）的导航请求。307/308 跳转后的下一跳浏览器可能不再给出
 * requestBody，此时沿用 [`carryAcrossRedirect`] 带过来的同方法请求体。
 */
export function recordCapturedRequest(
  cache: RequestRecordCache,
  url: string,
  method: string,
  body: CapturedBody | undefined,
  now: number,
): void {
  const previous = cache.get(url);
  const keptBody = body ?? (previous?.method === method ? previous.body : undefined);
  cache.set(url, { url, method, body: keptBody, ts: now });
}

/**
 * 重定向时传递请求事务：只有 307 / 308 保持原方法与请求体；301 / 302 / 303 后
 * 浏览器改发 GET（Fetch 规范），目标 URL 不能继承跳转前的 POST——否则引擎会对
 * 只收 GET 的直链（如 file.xiecdn.com）发 POST 而被断开连接，且 POST 任务不可恢复。
 */
export function carryAcrossRedirect(
  cache: RequestRecordCache,
  fromUrl: string,
  toUrl: string,
  statusCode: number,
  now: number,
): void {
  if (statusCode !== 307 && statusCode !== 308) return;
  const record = cache.get(fromUrl);
  if (!record) return;
  cache.set(toUrl, { ...record, url: toUrl, ts: now });
}

/** 取出下载 URL 自身的 (method, body)；GET 不回传（默认值，节省 NM 协议字节）。 */
export function lookupRequestRecord(
  cache: RequestRecordCache,
  url: string,
): { method?: string; body?: CapturedBody } {
  const record = cache.get(url);
  if (!record) return {};
  const method = record.method !== "GET" ? record.method : undefined;
  return { method, body: record.body };
}
