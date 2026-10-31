---
title: 订阅源指南
description: 把任意网站或 API 变成订阅源：manifest 的 subscriptions[]、subscribe(ctx) 契约、全部校验规则，以及与 resolver 配合。
section: plugins
order: 6
sourceHash: "65b8c70b1a39"
---

FluxDown 的 RSS 功能会按计划检查一个*订阅源*，记住已经见过的条目，应用用户设置的过滤规则，并为新条目创建下载任务。默认情况下，订阅源是一个 RSS、Atom 或 JSON Feed 的 URL。**订阅源 provider** 是替换「抓取并解析」这一步的插件，于是订阅源可以是任何能用 `flux.fetch` 读到的东西：JSON API、HTML 列表页、发布页面。

其余一切——调度、去重、过滤、首轮规则、失败退避、创建任务——仍由 FluxDown 负责。你的插件只需要把*一个订阅源 URL* 变成*一组条目*。

本文说明用户如何把订阅源指向你的 provider、manifest 声明、带全部校验规则的完整 `subscribe(ctx)` 契约、两个完整示例（其中一个与 resolver 配合），以及测试方法。manifest 字段也列在 [Manifest 参考](/docs/zh/plugins/manifest/)里。

## 各部分如何配合

| 部分 | 谁负责 | 职责 |
|---|---|---|
| 订阅源（source） | 用户 | 一条记录：URL、`providerId`、可选的 `providerConfig`，加上常规的 RSS 选项（间隔、过滤、保存目录、队列、自动下载、Cookie、User-Agent）。 |
| Provider | 你的插件 | `subscribe(ctx)`：抓取订阅源，返回 `{ title, link, items }`。 |
| 引擎 | FluxDown | 按计划调用 provider，校验结果，存储新条目，评估过滤规则，创建任务。 |
| Resolver（可选） | 你的插件或别的插件 | 当条目带有 `resolverItem` 时，把条目的页面 `link` 变成真正的下载。 |

内置 provider 的 id 固定为 `rss`。`providerId` 是其他值的订阅源，会交给声明了该 id 的插件。

### 用户如何为你的 provider 创建订阅源

由插件支撑的订阅源和普通订阅没有区别：它们会出现在 RSS 页面里，有自己的条目、**刷新**、最近一次错误和各项选项，用户的所有过滤规则（包含/排除关键词、体积范围、智能剧集去重）、队列和保存目录设置都照常生效。

订阅源需要的是 `providerId` + `providerConfig` 这一对字段。用户可以在三处创建，最终写入的是同一条记录：

- **桌面端与 Web 界面。** 在 RSS 页面点击**新建订阅**，然后在**订阅提供方**里选择你的插件名称。选择器首项是内置的 **RSS / Atom / JSON Feed**，其后为每个当前可用的插件 provider 列出一项：插件必须已安装、已启用、已加载，且其权限与网络主机都已授权。没有任何可用的插件 provider 时，选择器会隐藏，表单与普通 RSS 表单完全一致。选择插件 provider 后，表单会要求填写**来源地址**（以 `ctx.url` 交给你）和可选的**提供方配置（JSON）**（以 `ctx.providerConfig` 交给你）；配置请留空或填写合法 JSON，其他内容在保存前就会被拒绝。与 RSS 一样，新建订阅须先通过**验证**：FluxDown 会调用一次你的 `subscribe`，并显示订阅标题和条目数，或你返回的错误文本。打开已有订阅时会显示它的 provider；如果对应插件缺失、已禁用或尚未授权，选择器会保留已存储的 `providerId` 并说明原因，而不是悄悄切回 RSS，该订阅也会一直报告抓取错误，直到插件重新可用。
- **命令行。** `fluxdown rss add <URL> --provider <providerId> --provider-config '<JSON>'`。`--provider-config` 可省略，必须是合法 JSON，且需要同时给出 `--provider`。不带 `--provider` 时使用内置 RSS provider。命令创建订阅源后会立即发起第一次抓取，可用 `fluxdown rss items <sourceId>` 查看条目，用 `fluxdown rss ls` 的 `STATE` 列查看状态。
- **管理 API。** `POST /api/v1/rss`（或 `daemon.rss.createSource` RPC）直接接受这两个字段；`POST /api/v1/rss/validate` 也接受 `providerId` 与 `providerConfig`，做与界面**验证**相同的试运行：

```bash
curl -X POST http://127.0.0.1:17800/api/v1/rss \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
        "providerId": "acme-releases",
        "providerConfig": "{\"assetPattern\":\"\\\\.zip$\"}",
        "url": "https://releases.example.com/api/releases?project=foo",
        "intervalMinutes": 60,
        "autoDownload": true
      }'
# -> {"sourceId":"…"}
```

`providerConfig` 是一个不透明的**字符串**，含义由你定义；界面和命令行只接受合法 JSON（或留空），所以请遵循 JSON 这一惯例，而 API 本身会原样保存任意字符串。`url` 原样传给你。需要启用管理 API 并设置 token，见 [API 概览](/docs/zh/api/overview/)。如果 provider 没有安装或没有启用，订阅源会一直显示错误，直到它可用。`GET /api/v1/plugins` 会列出各插件的 `subscriptionProviderIds`。

你的 provider 需要了解的调度行为：

- **首轮从不下载。** 第一次成功抓取只会把当前的条目记为「已见过」（标为已读）。只有后续抓取中新出现的条目才会变成下载。如果想让条目出现在列表里但不下载任何东西，创建订阅源时设置 `"autoDownload": false`（收集模式）。
- **间隔。** 默认 30 分钟，最小 1 分钟。失败后，每连续失败一次间隔就翻倍，最长 6 小时（但不会短于用户设定的间隔）；成功一次即恢复。
- **按 `guid` 去重**，范围是单个订阅源。每次抓取最多把 `maxPerFetch`（默认 20，范围 1–100）个新条目变成任务，按 `pubDate` 从旧到新。FluxDown 每个订阅源保留最新的 500 条。
- 订阅源自带代理时，`flux.fetch` 与 `flux.ytdlp` 走它；否则按订阅地址命中的站点规则与应用全局代理设置（含 Auto 直连失败时改走代理）。

## 声明 provider

```json
"subscriptions": [
  { "providerId": "acme-releases", "entry": "subscribe.js", "timeoutMs": 20000 }
]
```

| 字段 | 规则 |
|---|---|
| `providerId` | 必填。`^[a-z0-9_-]{1,64}$`。必须稳定：每个使用它的订阅源都会保存这个值。**`rss` 是保留字**，属于内置 provider，会被拒绝。如果两个已启用的插件声明了相同的 id，`identity` 按字母序更小的那个生效，所以请选一个不容易撞的 id。 |
| `entry` | 必填。脚本文件，安全的相对路径。必须定义全局函数 **`subscribe`**——函数名固定。安装时会做编译检查。 |
| `timeoutMs` | 可选。单次调用的墙钟上限。默认 **10 秒**；最高可设到 **30 秒** 的硬顶。不能为 `0`。 |

每个插件至多**一个** provider。一个插件可以同时包含 `subscriptions`、`resolvers`、`hooks`、`auth`、设置项等。`required` 设置项会被校验：缺少值时调用以 `缺少必填设置项: 插件 <identity> 需先配置「<title>」` 失败。

这类调用与 resolver 共用运行时：内存 64 MB；如果 3 秒内没有空闲的执行名额，调用以 `插件运行时过载（并发已满）` 失败。连续三次超时或内存超限会触发熔断，该插件被禁用，与 resolver 相同。

## `subscribe(ctx)` 契约

```js
async function subscribe(ctx) {
  // ... fetch and normalize ...
  return { title: "…", link: "…", items: [ /* … */ ] };
}
```

### `ctx`

| 字段 | 含义 |
|---|---|
| `providerId` | 你的 provider id（一个脚本服务多个 id 时有用）。 |
| `sourceId` | 订阅源记录的 id，适合作为 `flux.storage` 的键。 |
| `url` | 订阅源 URL，与保存的完全一致。 |
| `providerConfig` | 不透明的配置字符串。用户没填时是 `"{}"`，所以只要内容是合法 JSON，`JSON.parse(ctx.providerConfig)` 总是安全的。 |
| `cookies` | 订阅源的 Cookie 字符串（`Cookie` 请求头的值），或 `""`。 |
| `userAgent` | 该订阅源的 User-Agent（订阅源设置，否则取全局设置）。 |

可用的门面：`flux.fetch`、`flux.storage`、`flux.fs`、`flux.logger`、`flux.settings`、`flux.info`，以及 `flux.auth`（需要 `auth` 权限，凭据会自动附加到 `flux.fetch`，见[认证指南](/docs/zh/plugins/auth-guide/)）和 `flux.ytdlp`（需要 `ytdlp`）。没有 `flux.ffmpeg`、`flux.task`，也没有常驻脚本的门面。Cookie 和 User-Agent **不会**自动加到你的请求里；站点需要的话请自己传。

### 返回值

返回一个**对象**。返回 `null`/`undefined` 是错误（`订阅返回值非法: …`）。

| 字段 | 类型 | 含义 |
|---|---|---|
| `title` | string | 订阅源标题。如果用户没填订阅源名称，会在首次成功后用它填一次。 |
| `link` | string | 订阅源的主页，仅供参考。 |
| `items` | array | 最多 **1000** 条。缺省表示空订阅。 |

每个条目：

| 字段 | 类型 | 规则 |
|---|---|---|
| `guid` | string | **必填**，非空，最长 2048 **字节**。去重键：同一个 `guid` 绝不会生成第二个任务，所以它在多次抓取之间必须保持稳定，并且在一次返回结果内唯一。 |
| `title` | string | 最长 1024 字节。包含/排除过滤和智能剧集识别读的就是它；请给出有意义的非空文本。 |
| `link` | string | 页面或直链 URL。 |
| `enclosureUrl` | string | 直接下载地址。两者都有时优先于 `link`。 |
| `resolverItem` | string | 最长 2048 字节。给 resolver 的不透明标识；**必须同时有 `link`**。见[与 resolver 配合](#与-resolver-配合)。 |
| `enclosureLength` | integer | 字节数，未知为 `0`，不能为负。用于用户的体积过滤。 |
| `pubDate` | integer | 发布时间，**Unix 秒**，未知为 `0`。决定新条目的下载顺序（最旧的优先）。 |

表格之外的规则：

1. `link` 和 `enclosureUrl` 至少要有一个。
2. 每个 URL 必须以 `http:`、`https:`、`ftp:`、`magnet:` 或 `ed2k:` 开头，且不超过 8 KB。其他（`javascript:`、`file:`、相对 URL）会使该条目无效。
3. **类型是严格的。** `guid`、`title`、`link`、`enclosureUrl`、`resolverItem` 必须是字符串（不能是数字，也不能是 `null`）；`enclosureLength` 和 `pubDate` 必须是整数（请用 `Math.round`/`Math.floor`；`"2025-01-01"` 或 `12.5` 都是错误）。类型错误会让**整个**结果以 `订阅返回值非法: …` 失败。不想提供的字段请直接省略，而不是设成 `null`。
4. **坏条目被跳过，而不是整体失败。** 单个无效条目会被跳过并记录（`[plugin] subscription output: skipped N of M items, first reason: …`），其余照常使用。上游的一行脏数据不应该让订阅源进入退避。但如果列表非空而**全部**条目都无效，调用就失败（`订阅条目全部非法（N 条），首条原因: …`）。超过 1000 条也会失败（`订阅条目数量超过 1000`）。
5. 条目的下载地址：有 `resolverItem` 时是 `link`（经过 resolver）；否则是 `enclosureUrl`，没有时回退到 `link`。`.torrent` URL 和磁力链接与其他地方一样由 FluxDown 的 BitTorrent 引擎处理。

条目被跳过时日志里记录的原因有：

| 原因 | 含义 |
|---|---|
| `guid 必须非空且不超过 2048 字节` | guid 为空或超长 |
| `条目 <guid> 的 title 超过 1024 字节` | title 超长 |
| `条目 <guid> 的 resolverItem 超过 2048 字节` | resolverItem 超长 |
| `条目 <guid> 缺少 link / enclosureUrl` | link 和 enclosureUrl 都没有 |
| `条目 <guid> 含 resolverItem 时必须提供 link` | 有 resolverItem 却没有 link |
| `条目 <guid> 的 link 非法: 插件输出非法: url scheme 不允许: <scheme>` | link 的 scheme 不在允许范围（或 `url 超过 8KB`）；`enclosureUrl` 同理 |
| `条目 <guid> 的 enclosureLength 不可为负数` | enclosureLength 为负 |

## 示例 1：把 JSON API 变成订阅源

一个虚构的发布 API `https://releases.example.com/api/releases?project=foo` 返回：

```json
{
  "project": "foo",
  "homepage": "https://releases.example.com/foo",
  "releases": [
    {
      "id": 412,
      "name": "v2.4.0",
      "published": "2025-03-02T10:15:00Z",
      "page": "https://releases.example.com/foo/v2.4.0",
      "assets": [
        { "name": "foo-2.4.0-win.zip", "url": "https://dl.example.com/foo/2.4.0/foo-2.4.0-win.zip", "size": 48211967 },
        { "name": "foo-2.4.0-mac.dmg", "url": "https://dl.example.com/foo/2.4.0/foo-2.4.0-mac.dmg", "size": 51200333 }
      ]
    }
  ]
}
```

每个发布的每个附件都变成一个条目。`providerConfig` 带一个可选的文件名过滤：`{"assetPattern": "\\.zip$"}`。

`manifest.json`

```json
{
  "identity": "acme@release-feed",
  "name": "Release Feed",
  "version": "1.0.0",
  "description": "Subscribe to the assets of a project on releases.example.com.",
  "subscriptions": [
    { "providerId": "acme-releases", "entry": "subscribe.js", "timeoutMs": 20000 }
  ]
}
```

`subscribe.js`

```js
function parseConfig(raw) {
  try {
    return JSON.parse(raw || "{}");
  } catch (e) {
    throw new Error("providerConfig is not valid JSON: " + e.message);
  }
}

function normalize(data, config) {
  const pattern = config.assetPattern ? new RegExp(config.assetPattern, "i") : null;
  const items = [];
  for (const release of data.releases || []) {
    for (const asset of release.assets || []) {
      if (pattern && !pattern.test(asset.name)) continue;
      items.push({
        // Stable and unique per asset: never reuse a guid for different files.
        guid: release.id + "/" + asset.name,
        title: data.project + " " + release.name + " - " + asset.name,
        link: release.page,
        enclosureUrl: asset.url,
        enclosureLength: Math.max(0, Math.round(asset.size || 0)),
        // Unix seconds; 0 when the date cannot be parsed.
        pubDate: Math.floor(Date.parse(release.published) / 1000) || 0,
      });
    }
  }
  return { title: data.project + " releases", link: data.homepage || "", items };
}

async function subscribe(ctx) {
  const config = parseConfig(ctx.providerConfig);
  const headers = { Accept: "application/json", "User-Agent": ctx.userAgent };
  if (ctx.cookies) headers.Cookie = ctx.cookies;

  const res = await flux.fetch({ url: ctx.url, headers });
  if (res.status !== 200) throw new Error("releases API answered HTTP " + res.status);
  if (res.truncated) throw new Error("the response exceeded the 8 MB limit");

  return normalize(JSON.parse(res.body), config);
}
```

说明：

- 抓取失败时直接抛出异常就是正确做法：消息会成为订阅源的*最近一次错误*（前面加上 `插件运行时错误: `），并触发退避。
- `flux.fetch` 每个响应最多读 8 MB，每个请求 10 秒，只能访问公网的 `http`/`https` 主机；请检查 `res.truncated`。
- 接收纯数据的 `normalize` 函数很容易在 FluxDown 之外做单元测试（见[测试](#测试)）。
- 插件运行时里没有 DOM。HTML 页面请把解析控制在少量带锚点的 `RegExp` 内（下面的剧集示例就是这么做的），站点提供 JSON 接口时优先用 JSON。

## 与 resolver 配合

有时条目的最终链接只有在下载时才知道——比如一个放在订阅列表里会过期的签名 URL，或者一个提供多种画质的页面。这时把**页面**作为 `link` 返回，并设置 `resolverItem`。FluxDown 创建任务时会使用 `link`，找到 `resolvers[0].match.urls` 匹配它的 resolver 插件，并在调用时把 `ctx.resolverItem` 设为你的标识（二段解析）。由 resolver 把它变成真正的文件。

配合规则：

- `resolverItem` 对 FluxDown 是不透明的。它的含义（画质、id、`<id>@<variant>`……）由你定义，由你的 resolver 读取。
- 它要求有 `link`；带 `resolverItem` 却没有 `link` 的条目会被跳过。
- 如果没有任何已安装且已启用的插件的 resolver 匹配该 `link`，任务创建出来就是**失败**状态，错误为 `解析插件不可用`。它绝不会把页面当成文件下载下来。
- 二段解析是无人值守的：返回 `variants` 的 resolver 不会弹出选择框，而是使用 `defaultVariantIndex` 指向的那一项。二段解析**不能**返回 `manifest`（`二段解析（resolverItem 非空）不可返回 manifest：防递归裂变`）。
- resolver 可以是你自己的（同一个插件，如下例），也可以是别的插件的。

### 完整示例：剧集列表页

`tv.example.com/shows/42` 以 `/watch/7001` 这样的链接列出各集。用户在 `providerConfig` 里选择画质（`{"quality":"1080p"}`）。provider 为每一集生成一个条目；同一插件里的 resolver 在任务开始时挑出该集对应画质的流。

`manifest.json`

```json
{
  "identity": "acme@tv-episodes",
  "name": "TV Episodes",
  "version": "1.0.0",
  "description": "Follow a show on tv.example.com and download new episodes in your preferred quality.",
  "resolvers": [
    {
      "match": { "urls": ["*://tv.example.com/watch/*"] },
      "entry": "resolve.js"
    }
  ],
  "subscriptions": [
    { "providerId": "acme-tv", "entry": "subscribe.js" }
  ]
}
```

`subscribe.js`

```js
async function subscribe(ctx) {
  const config = JSON.parse(ctx.providerConfig);
  const quality = config.quality || "720p";

  const res = await flux.fetch({ url: ctx.url, headers: { "User-Agent": ctx.userAgent } });
  if (res.status !== 200) throw new Error("listing answered HTTP " + res.status);

  const origin = ctx.url.match(/^https?:\/\/[^/]+/)[0];
  const title = (res.body.match(/<h1[^>]*>([^<]+)<\/h1>/) || [])[1] || "TV show";

  const items = [];
  const re = /<a class="ep" href="(\/watch\/(\d+))"[^>]*>([^<]+)<\/a>/g;
  let m;
  while ((m = re.exec(res.body)) !== null) {
    items.push({
      guid: "episode-" + m[2],          // one task per episode, whatever the quality
      title: title.trim() + " - " + m[3].trim(),
      link: origin + m[1],              // the page, resolved at download time
      resolverItem: quality,            // handed to resolve() as ctx.resolverItem
    });
  }
  return { title: title.trim(), link: ctx.url, items };
}
```

`resolve.js`

```js
async function resolve(ctx) {
  const id = (ctx.url.match(/\/watch\/(\d+)/) || [])[1];
  if (!id) return null;

  // Subscription items carry the wanted quality; a link added by hand has none.
  const wanted = ctx.resolverItem || "720p";

  const res = await flux.fetch({
    url: "https://tv.example.com/api/episodes/" + id + "/sources",
    headers: { Referer: ctx.url },
  });
  if (res.status !== 200) throw new Error("sources API answered HTTP " + res.status);

  const sources = JSON.parse(res.body).sources || [];
  const pick = sources.find((s) => s.quality === wanted) || sources[0];
  if (!pick) throw new Error("no playable source for episode " + id);

  return { url: pick.url, fileName: "episode-" + id + "-" + pick.quality + ".mp4", ephemeral: true };
}
```

页面 URL 一直保存在订阅里，带签名的流地址则只在任务开始时（以及每次恢复时）才生成，所以条目列表里的链接永远不会过期。

## 测试

**在 FluxDown 之外对映射逻辑做单元测试。** 像示例 1 那样把抓取和规范化分开，然后在 Node 或 Bun 里用一个假的 `flux` 运行脚本：

```js
// test.mjs — node test.mjs
import fs from "node:fs";
import vm from "node:vm";

const sample = { project: "foo", releases: [{ id: 1, name: "v1", published: "2025-03-02T10:15:00Z",
  page: "https://releases.example.com/foo/v1",
  assets: [{ name: "foo.zip", url: "https://dl.example.com/foo.zip", size: 10 }] }] };

const flux = {
  fetch: async () => ({ status: 200, headers: {}, truncated: false, body: JSON.stringify(sample) }),
};
const sandbox = vm.createContext({ flux, JSON, Date, Math, RegExp, Error });
vm.runInContext(fs.readFileSync("subscribe.js", "utf8"), sandbox);

const out = await sandbox.subscribe({
  url: "https://releases.example.com/api/releases?project=foo",
  providerConfig: '{"assetPattern":"\\\\.zip$"}',
  cookies: "",
  userAgent: "test",
});
console.log(JSON.stringify(out, null, 2));
```

人工（或用代码）检查：每个 `guid` 都是唯一的字符串，每个 URL 都以允许的 scheme 开头，数字都是整数。

**在 FluxDown 里端到端测试：**

1. 用开发模式安装文件夹。`subscribe.js` 每次调用都会重新读取；修改 `manifest.json` 后，点插件行上的「重新加载」。
2. 用上面的 `curl` 命令创建订阅源，并设置 `"autoDownload": false`，这样迭代期间不会下载任何东西。
3. 随时用 `POST /api/v1/rss/{sourceId}/refresh` 触发一次抓取，再用 `GET /api/v1/rss/{sourceId}/items` 查看结果，用 `GET /api/v1/rss` 看订阅源的 `lastError`。插件日志里能看到你的 `flux.logger` 输出（见[调试与排错](/docs/zh/plugins/debugging/)）。
4. 第一次抓取只做初始化。想看到新条目出现：修改上游数据后删掉订阅源重新创建，或者改成 `autoDownload: true`，然后等下一次数据变化。
5. 配合 resolver：把某个条目的 `link` 放进普通下载，确认那里的 `ctx.resolverItem` 为空；再让订阅创建任务，确认它带上了值。

## 错误

订阅源最近一次错误里的文字就是引擎消息（中文），原样如下：

| 消息 | 原因 |
|---|---|
| `订阅 provider <id> 未安装或已禁用` | 没有已启用的插件声明这个 `providerId`——包括被熔断禁用之后。 |
| `订阅 provider <id> 的脚本读取失败` | `entry` 文件缺失或无法读取。 |
| `入口 subscribe 未定义` | 脚本里没有全局的 `subscribe`（用 `const` 声明的箭头函数不算——请用 `async function subscribe`）。 |
| `订阅返回值非法: …` | `null`/`undefined`、不是对象，或者某个字段类型错误。后面的内容是解析器的描述。 |
| `订阅条目数量超过 1000` | 条目超过 1000 条。 |
| `订阅条目全部非法（N 条），首条原因: …` | 每个条目都没通过校验；原因见上面的表。 |
| `缺少必填设置项: 插件 … 需先配置「…」` | 某个 `required` 设置项没有值。 |
| `脚本执行超时` | 超过了 `timeoutMs`（默认 10 秒，上限 30 秒）。 |
| `插件运行时错误: <你的消息>` | 你的脚本抛出的异常，或被拒绝的 `flux.*` 调用。 |

另见：需要登录的站点见[认证指南](/docs/zh/plugins/auth-guide/)；响应订阅创建的下载见[钩子指南](/docs/zh/plugins/hooks-guide/)；resolver 基础见[写第一个插件](/docs/zh/plugins/your-first-plugin/)。
