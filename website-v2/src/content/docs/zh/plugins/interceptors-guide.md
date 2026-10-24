---
title: 拦截器指南
description: 读取并修改运行中的下载——改写请求、设置连接数、选择流媒体画质、过滤分片、在完成前处理文件并决定是否重试。
section: plugins
order: 4.5
sourceHash: "4bcd9e13155e"
---

拦截器让插件能够“深入”引擎的任务流水线。[事件钩子](/docs/zh/plugins/hooks-guide/)只能在事后做出反应，而拦截器可以看到引擎持有的任务信息，并在引擎使用之前修改它：请求（URL、method、body、请求头、Cookie、Referer、User-Agent、按站点请求头）、连接数、流媒体画质、单个分片、已下载完成的文件，以及失败任务是否重试。

manifest 字段见 [Manifest 参考](/docs/zh/plugins/manifest/#interceptors)；所需权限是 `intercept`（较高风险，见[权限](/docs/zh/plugins/permissions/)）。

## 声明并编写插件

```json
{
  "identity": "acme@demo-intercept",
  "name": "Demo Interceptor",
  "version": "1.0.0",
  "permissions": ["intercept"],
  "interceptors": {
    "entry": "intercept.js",
    "stages": ["prepare", "segments"],
    "match": { "urls": ["https://example.com/*"] }
  }
}
```

入口脚本定义**一个**全局函数 `intercept(ctx)`。插件订阅的每个阶段，FluxDown 都会调用它一次；按 `ctx.stage` 分支处理即可。返回 `null` / `undefined` 表示不做改动，否则返回该阶段的结果对象。所有字段名均为 camelCase，每次调用都在全新的脚本上下文中运行。

```js
globalThis.intercept = function (ctx) {
  if (ctx.stage === "prepare") {
    return { userAgent: "Demo/1", setHeaders: { "X-Demo": "on" }, segments: 4 };
  }
  if (ctx.stage === "segments") {
    return { drop: ctx.segments.filter((s) => s.tags.some((t) => t.startsWith("#EXT-X-CUE-OUT"))).map((s) => s.id) };
  }
  return null;
};
```

使用 TypeScript SDK 时用 `defineInterceptors`，它会替你安装入口，并为每个阶段的上下文与结果提供类型：

```ts
import { defineInterceptors } from "fluxdown-plugin-sdk/runtime";

defineInterceptors({
  prepare: (ctx) => ({ setHeaders: { "X-Demo": "on" } }),
  segments: (ctx) => ({ drop: ctx.segments.filter((s) => s.duration === 0).map((s) => s.id) }),
});
```

## 拦截哪些任务、什么顺序

已启用、持有 `intercept` 权限，且 `match.urls` 命中任务原始 URL 或 resolver 改写后 URL 的插件会拦截该任务。多个插件同时命中时按 identity 升序执行，后者看到前者改写后的结果。`ctx.task` 始终包含 `id`、`url`（本次运行的 URL）、`sourceUrl`（持久化的 URL）、`fileName`、`saveDir`、`protocol`（`http`、`ftp`、`hls`、`dash`）、`trackPair` 与 `isResume`。BitTorrent 与 eD2K 任务不会被拦截。

调用抛错、超时或返回非法值时由 `onFailure` 决定：`continue` 在任务活动日志（类型**插件**）里记一条说明并跳过该插件；`fail` 则让任务失败。

## 阶段

| 阶段 | 何时 | 返回 |
|---|---|---|
| `prepare` | 构造请求之前——每次运行都会执行：首次启动、重试、继续、重启续传；元数据探测之前也会执行。 | `RequestPatch` |
| `probed` | HTTP / FTP 探测之后、规划分段之前。上下文额外含 `probe`（`totalBytes`、`supportsRange`、`fileName`、`contentType`、`etag`、`lastModified`）。 | `{ segments?, maxConnections?, fail? }` |
| `refresh` | 运行中请求被 401 / 403 / 410 拒绝。上下文：`request`（被拒请求的身份，`url` 为实际地址）、`status`、`refreshCount`。处于下载热路径：`timeoutMs` 上限 5 秒。 | `RefreshPatch`：`url`、`cookies`、`referrer`、`userAgent`、`setHeaders`、`removeHeaders`、`originHeaders` |
| `selectVariant` | HLS 主清单或 DASH 的 representation（`variants[]`，含带宽、尺寸、编码）。 | `{ select?: 序号 }` |
| `selectPeriod` | 含多个 Period 的 DASH MPD（`periods[]`、`manifestText`）。只下载被选中的 Period。 | `{ select?: 序号 }` |
| `segments` | 点播清单 / MPD，每条轨道一次（`track` 为 `video` 或 `audio`）。 | `{ drop?: id[], rewrite?: {id, uri}[] }` |
| `liveSegments` | 直播清单的每次刷新（`timeoutMs` 上限 2 秒）。 | `{ drop?: id[], rewrite?: {id, uri}[] }` |
| `finalize` | 下载完成之后、任务标记为完成之前（除 BT / eD2K 外的所有协议）。任务此时保持“下载中 100%”。上下文含 `filePath`、`fileName`、`saveDir`、`totalBytes`、`audioPath`。 | `{ fail?, replaceWith? }` |
| `failed` | 任务失败之后（上下文含 `errorMessage`、`httpStatus`、`autoRetryCount`）。 | `{ retry?: { delayMs? } }` |

说明：

- `segments` / `liveSegments` 收到 `format`、`track`、`playlistUrl`、`manifestText`（清单或 MPD 原文，上限 4 MiB；超出时为 `""` 且 `manifestTextOmitted` 为 `true`）以及 `segments[]`。每项含 `id`（HLS 为绝对 media sequence；DASH 为媒体分片中从 0 起的序号，不含 init）、`uri`、`duration`、`byteRange`、`discontinuity`、`title`、`programDateTime`、`dateRange`、`tags`（引擎未识别的标签原文）、`key`、`map`，DASH 模板分片还有 `number` / `time`。
- 多个插件丢弃的 id 取并集；后续插件只看到尚未被丢弃的分片。未知 id，或会清空全部分片的丢弃，对该插件而言是错误。丢弃分片会使已有的续传失效：下载会干净地重新开始。直播中被丢弃的分片不会在下一轮重新规划。
- `rewrite` 替换分片地址（例如逐片签名的 URL）。相对地址按 `playlistUrl` 解析，最终必须是 http(s)。后续插件看到的是改写后的地址。同一插件的 `drop` 与 `rewrite` 要么一起采纳，要么都不采纳（未知 id 或非法地址视为错误）；被丢弃分片的改写会被舍弃。
- `selectVariant` / `selectPeriod`：第一个返回合法序号的插件胜出；越界序号视为错误。
- `probed.fail` 与 `finalize.fail` 无论 `onFailure` 如何设置，都会让任务失败。
- `finalize.replaceWith` 是保存目录内一个已存在的文件名；旧文件会被删除，任务改指向新文件。若任务在 `finalize` 期间被暂停或取消，则按原文件完成。
- `failed.retry.delayMs` 被钳制到 `0..=300000`。重试走正常的继续路径，因此 `prepare` 会再次运行。

## `RequestPatch`

由 `prepare` 返回，也可作为 [resolver](/docs/zh/plugins/api-reference/) 返回值的 `request` 字段（结构相同）。

| 字段 | 规则 |
|---|---|
| `url` | 不超过 8 KiB，合法 URL，**主机必须与原来相同**，且**协议类别不得改变**（HTTP 仍是 HTTP，HLS 仍是 HLS……）。跨站改写请用 resolver。 |
| `method` | 仅 HTTP 任务；大小写不敏感；拒绝 `OPTIONS`。 |
| `body` / `clearBody` | 仅 HTTP 任务。`body` 为 `{kind:"formData",fields}`、`{kind:"urlencoded",raw}` 或 `{kind:"raw",bytesB64,contentType?}`；`clearBody: true` 清除请求体。 |
| `cookies` | 替换 Cookie 字符串，并移除已有的 `Cookie` 请求头。 |
| `referrer` | 空串或 http(s) 链接。会锁定 Referer，引擎自带的 Referer 推断不再改动它。 |
| `userAgent` | 非空且不含控制字符。会锁定 User-Agent：引擎的反爬 UA 切换、探测被拒的自适应和机器人挑战逃逸都不再改动它。 |
| `setHeaders` / `removeHeaders` | 名称大小写不敏感。引擎管理的请求头（如 `Range`、`Host`、`Content-Length`）不可设置。设置或删除 `User-Agent` / `Referer` 会将其锁定。 |
| `originHeaders` | `[{origin, headers}]`——只对站点（`scheme://host[:port]`）等于 `origin` 的请求附加的额外请求头，例如分片所在主机。至多 32 条，按 origin 合并。 |
| `segments` | `0..=512`；**覆盖本次运行的用户线程数**（0 = 自动）。 |
| `maxConnections` | `1..=512`；始终生效的硬上限。 |
| `maxRangeBytes` | 仅 HTTP 任务；整数，单位字节，`>= 65536`（64 KiB）。限制该任务**每一个**数据请求的 `Range` 跨度，适用于拒绝大区间的直链（例如网盘 CDN 单次请求超过约 5 MiB 就返回 403——请设为不超过该上限的值，如 `4 * 1024 * 1024`）。分段 worker 在各自区间内按不超过该值的窗口逐个请求并复用连接；单流与回退单流只发不超过该值的有界 `Range` 请求（绝不发无 Range 的 GET 或开放式 `bytes=N-`）；与 `ephemeral` 同用时，首个连接也带 `Range`。需要已知大小（`totalBytes` 或探测结果）且服务器支持 `Range`，否则无法生效。元数据探测（`HEAD`、`Range: bytes=0-0`）不受影响。FTP / HLS / DASH 任务会拒绝该字段。 |
| `ephemeral` | 仅 HTTP 任务。与 resolver 返回值同义：本次运行跳过元数据探测，避免一次性凭据被探测消耗。 |
| `totalBytes` / `rangeSupported` | 仅 HTTP，且必须与 `ephemeral: true` 同用。`totalBytes`（> 0）是代替探测使用的大小；`rangeSupported` 表示服务器已知支持 Range（首次运行默认 `false`）。 |
| `fileName` | 不含路径分隔符、`..` 与控制字符。**仅在首次运行（下载开始前）生效**；之后的运行会忽略并记一条说明。 |

所有字段通用的规则：

- **FTP** 任务只接受 `url`、`segments`、`maxConnections` 与 `fileName`；请求头、Cookie、method、body 类字段会被拒绝。
- 补丁是**原子**的：任一字段非法则整份补丁被拒绝，任务保持不变。`onFailure: "continue"` 时该插件被跳过并记一条说明。
- 代理地址与“忽略 TLS 错误”可在 `ctx.request`（`proxyUrl`、`ignoreTlsErrors`）里看到，但只读。
- `ctx.request.userAgent` 是有效 UA（有请求头 UA 则取它，否则取解析后的任务 / 队列 / 全局 UA）。
- 活动日志只记录*改了哪些东西*（字段名、文件名），绝不记录请求头的值、Cookie 或请求体。

## 一次性鉴权

按凭据类型选择做法：

| 凭据类型 | 做法 |
|---|---|
| 时间窗口内可用 | 在 `prepare` 设置请求头 / Cookie / URL 参数即可，无需其他处理。 |
| 首次请求即作废 | `prepare` 返回凭据并带 `ephemeral: true`（已知时附带 `totalBytes` / `rangeSupported`），避免探测把它用掉。 |
| 下载中途过期 | 订阅 `refresh`：请求被 401 / 403 / 410 拒绝时插件返回新凭据，引擎对该请求重发一次。 |
| 每个请求一个令牌 | `prepare` 用 `ephemeral: true`，并在每次被拒时由 `refresh` 换新；流媒体用 `rewrite` 为每个分片地址签名。 |

限制：

- **连续 5 次**换发后都没换来任何成功响应（凭据始终无效）即停止刷新。被拒的凭据此前成功用过至少一次属于正常消耗，会让计数归零——因此「每请求一个令牌」的方案可以按需反复刷新。
- 并发被拒会**单飞合并**：一次插件调用服务所有请求；用旧身份发出的请求直接用当前身份重发。插件未做任何改动时，该身份代际被标记为已拒绝，不再重复询问。
- 刷新得到的凭据**不落库**；下次运行从 `prepare` 重新开始。
- 非 HTTP 任务不能刷新 `url`（其他凭据字段可以）。`refresh` 不能改写 `method`、`body`、`segments`、`maxConnections`、`maxRangeBytes`、`fileName`、`ephemeral`。
- `failed` 重试有独立的 5 次额度，**不受**自动重试开关影响。
- 锁定 User-Agent / Referer 之后，探测的自适应不再重发相同的请求。

## 改写在重试中持续生效，且不落库

补丁在每次运行都会重新应用，因此探测重试、分段重试、反爬 UA 切换、自动重试、手动继续、重启续传以及 HLS 回退 HTTP 时，改写都保持有效。任务记录里只保存基础上下文（引擎自己未被锁定的 User-Agent 与 Referer 自适应结果）；插件添加的请求头、插件指定的 method 与 body、按站点请求头**绝不写入数据库**，下次运行时由插件重新提供。

## 示例

403 失败后 5 秒重试（stages 为 `["failed"]`）：

```js
globalThis.intercept = (ctx) =>
  ctx.stage === "failed" && ctx.httpStatus === 403 && ctx.autoRetryCount < 3
    ? { retry: { delayMs: 5000 } }
    : null;
```

选择不超过 720p 的最高带宽画质（stages 为 `["selectVariant"]`）：

```js
globalThis.intercept = (ctx) => {
  const ok = ctx.variants.filter((v) => (v.height ?? 0) <= 720);
  if (!ok.length) return null;
  return { select: ok.reduce((a, b) => (b.bandwidth > a.bandwidth ? b : a)).index };
};
```
