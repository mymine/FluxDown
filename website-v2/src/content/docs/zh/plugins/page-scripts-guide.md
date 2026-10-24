---
title: 页面脚本
description: 通过 FluxDown 浏览器扩展在网页中运行插件脚本——声明、使用 fluxdown 桥、搭配 main 与 isolated 两个 world，以及各项限制。
section: plugins
order: 4.8
sourceHash: "7d69e9d99fdf"
---

**页面脚本**是插件自带的一个 JavaScript 文件，由 FluxDown 浏览器扩展在你列出的网站里运行——例如给视频站加一个「下载」按钮，或把页面上找到的链接交给插件的 `main` 脚本处理。扩展从 FluxDown 桌面端获取代码，并用浏览器自带的 `userScripts` API 注册。

manifest 字段见 [Manifest 参考](/docs/zh/plugins/manifest/#pagescripts)；所需权限是 `pageScripts`（**完全**风险，见[权限](/docs/zh/plugins/permissions/#浏览器中的页面脚本)）。

## 声明页面脚本

```json
{
  "identity": "acme@page-demo",
  "name": "页面演示",
  "version": "1.0.0",
  "main": { "entry": "main.js" },
  "permissions": ["pageScripts"],
  "contributes": {
    "pageScripts": [{
      "id": "hello",
      "title": "你好按钮",
      "matches": ["https://example.com/*"],
      "js": ["page/hello.js"]
    }]
  }
}
```

`main.js` 用 `page: true` 向页面暴露一个处理函数：

```js
flux.rpc.handle("lookup", async (args, ctx) => {
  // ctx = { caller: "page", pageUrl }——两者都来自网页，属于不可信输入。
  const url = typeof args?.url === "string" ? args.url : "";
  if (!/^https:\/\//.test(url)) return { ok: false, error: "https only" };
  return { ok: true, title: "found", from: ctx.pageUrl };
}, { page: true });
```

`page/hello.js` 在 `https://example.com/*` 上运行并调用它：

```js
(async () => {
  const reply = await fluxdown.invoke("lookup", { url: location.href });
  if (reply.ok) {
    await fluxdown.download({ url: location.href });
  }
})();
```

处理函数在这次调用里用 `flux.tasks.create` 新建、或用 `flux.tasks.resume` 继续的任务，与浏览器直接捕获的下载走同一套桌面提示：恰好开始一个任务、且 FluxDown 主窗口不在前台时，按「通知 → 下载窗口」的设置弹出进度窗口和完成窗口；一次开始多个任务不弹。要让提示生效，请在处理函数返回前 `await` 这些调用——处理函数返回后才完成的调用不会计入。

完整可运行的版本见[功能演示插件](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk/examples/feature-demo)（`page/hello.js`）。

## 选择 world

| `world` | 运行位置 | `fluxdown` 桥 |
|---|---|---|
| `isolated`（默认） | 扩展的用户脚本 world：独立的 JavaScript 全局对象，共享页面 DOM | **有** |
| `main` | 页面自己的 JavaScript world：能看到页面的变量和函数 | **没有** |

`main` 脚本故意不提供桥：该 world 中的一切都能被页面自己观察和伪造，在那里放桥会被网站滥用。`main` 只用于读取或调用页面的 JavaScript。

### main 与 isolated 脚本搭配

如果既需要页面的 JavaScript 状态（比如播放器对象），又需要桥，就各声明一条脚本，每个 world 一条，通过 DOM 事件通信：

```json
"pageScripts": [
  { "id": "probe", "title": "探针", "matches": ["https://example.com/*"], "js": ["page/probe-main.js"], "world": "main" },
  { "id": "bridge", "title": "桥接", "matches": ["https://example.com/*"], "js": ["page/probe-bridge.js"] }
]
```

```js
// page/probe-main.js —— world: main
document.dispatchEvent(new CustomEvent("acme-demo:state", { detail: JSON.stringify({ id: window.player?.id ?? null }) }));
```

```js
// page/probe-bridge.js —— world: isolated
document.addEventListener("acme-demo:state", async (event) => {
  let state;
  try { state = JSON.parse(String(event.detail)); } catch { return; }
  if (typeof state?.id !== "string") return;   // 页面可以发送任何内容：务必校验
  await fluxdown.invoke("lookup", { url: `https://example.com/v/${encodeURIComponent(state.id)}` });
});
```

isolated 一侧必须把每个事件都当作**不可信输入**：页面（或页面上的其他脚本）可以派发同名事件，里面放任何内容。

## `fluxdown` 桥

只在 `isolated` 脚本中可用。每次调用都返回 Promise，失败时 reject 一个带可读消息的 `Error`。

| API | 作用 |
|---|---|
| `fluxdown.info` | 当前脚本的 `{ identity, scriptId, version }`。 |
| `fluxdown.download({ url, filename?, referrer?, headers? })` | 把下载交给 FluxDown。`url` 必须是 `http`/`https`。`headers` 只能包含 `referer`、`origin`、`user-agent`、`accept`、`accept-language`、`x-requested-with`。`referrer` 缺省为当前页面。返回 `{ ok, message? }`。 |
| `fluxdown.addMedia([{ url, title?, mime? }])` | 把资源加入本标签页的嗅探媒体列表（与嗅探结果是同一个列表，并标注来源插件）。仅限 `http`/`https`，单次至多 100 条。返回被接受的条数。 |
| `fluxdown.invoke(name, args?)` | 调用你的 `main` 用 `flux.rpc.handle(name, fn, { page: true })` 注册的处理函数。 |
| `fluxdown.storage.get(key)` / `set(key, value)` / `remove(key)` | 扩展中按插件隔离的持久存储。值必须可 JSON 序列化。 |

TypeScript 声明在 `fluxdown-plugin-sdk/types/page`（`/// <reference types="fluxdown-plugin-sdk/types/page" />`）。

## 限制

| 限制 | 值 |
|---|---|
| 每个插件的页面脚本数 | 16 |
| 每条脚本的 `matches` / `excludeMatches` | 各 32 条 |
| 每条脚本的 `js` 文件数 | 8 |
| 单个 `js` 文件 | 256 KiB |
| 单条脚本（文件拼接后） | 384 KiB |
| 单条桥消息（JSON） | 256 KiB |
| `fluxdown.addMedia` 每次 | 100 条 |
| 每个插件的 `fluxdown.storage` | 1 MiB |

桥调用由扩展校验，并按插件限流。脚本所在 world 的内容安全策略禁止把字符串当代码执行：`isolated` 脚本里 `eval` 和 `new Function` 不可用，所以请直接交付普通代码，不要在运行时拼装。

## 用户会看到什么，更新何时生效

1. 用户在 FluxDown 里批准 `pageScripts` 权限（对话框会列出每条脚本的标题与站点）。
2. 在浏览器扩展的选项页，用户为你的插件开启页面脚本——扩展会按插件询问一次并说明风险。浏览器本身还需要打开用户脚本开关（见[插件页面脚本](/docs/zh/browser-extension/usage/#插件页面脚本)）。
3. 此后扩展会自动与 FluxDown 保持一致：插件新版本、新增脚本、修改 `matches` 或更新代码都会自动生效，**不再**重复询问。停用或卸载插件会移除这些脚本。

扩展始终从桌面端获取代码，自己不保存源码副本。桌面端不可达时，已经注册的脚本继续运行。

## 安全注意事项

- 页面脚本能做该站点上页面用户能做的一切。匹配范围取**最小**需要的站点，不涉及的区域用 `excludeMatches` 排除。
- 来自页面的一切都不可信：DOM 内容、事件、`ctx.pageUrl`、`args`。除了在脚本里校验，`main` 的处理函数里也要校验。
- 只有当「匹配页面上的任何脚本都可以安全调用」该处理函数时，才用 `page: true` 暴露它。能调用到基于 `flux.tasks` 的处理函数的页面脚本，可以替用户创建下载。
- 不要把密钥放进页面脚本或 `fluxdown.storage`；最坏情况下页面环境可以读到它们。
- 审查插件时，把它当作要在这些站点的浏览器控制台里执行的代码，因为它就是这样的东西。

## 你会遇到的错误

| 消息 | 原因 |
|---|---|
| `声明 pageScripts 时 permissions 必须包含 pageScripts` | manifest 声明了页面脚本，却没有 `pageScripts` 权限。 |
| `包内缺少文件 '<路径>'` | `js` 中的某个文件不在包内。 |
| `rpc not exposed to page: <name>` | 处理函数注册时没有带 `{ page: true }`。 |
| `rpc not registered: <name>` | `main` 里没有这个名称的处理函数（或插件已停用）。 |

更多校验消息见[调试与排错](/docs/zh/plugins/debugging/)。
