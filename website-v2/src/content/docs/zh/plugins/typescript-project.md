---
title: TypeScript 项目
description: 用 fluxdown-plugin-sdk 和 TypeScript 做一个完整插件——resolver、hook、设置、JSX 视图和命令——再校验并打成 .fxplug。
section: plugins
order: 3
sourceHash: "3ca2f3ce7b33"
---

这篇教程带你用 [`fluxdown-plugin-sdk`](/docs/zh/plugins/sdk/)，从一个空文件夹走到打好包的 `.fxplug`。你将做出 **Mirror Demo**，一个做五件事的小插件：

| 功能 | 所在位置 |
|---|---|
| 一个 **resolver**，把 `https://mirror-demo.invalid/files/<name>` 改写到可配置的镜像 | `src/resolve.ts` |
| 一个 **hook**，统计下载完成的任务数 | `src/hooks.ts` |
| 一个带镜像地址的**设置**表单 | `manifest.json` → `settings` |
| 活动栏上的一个**视图**，以及一个添加示例下载的**命令** | `src/main.tsx` |
| 视图创建下载所需的 `tasks` **权限** | `manifest.json` → `permissions` |

你需要 Node.js 20 或更高版本，以及 FluxDown（桌面应用，或你自己控制的服务器的 Web 端）。本教程不要求先读别的文档，不过 [写第一个插件](/docs/zh/plugins/your-first-plugin/) 对 resolver 的概念讲得更细，[概览](/docs/zh/plugins/overview/)则解释了执行模型。

## 1. 创建项目

```bash
npx fluxdown-plugin-sdk create mirror-demo --identity me@mirror-demo --name "Mirror Demo"
cd mirror-demo
npm install
```

- `--identity` 是插件的永久 ID，格式 `作者@名称`，只能用小写字母、数字、`_` 和 `-`。省略时取 `me@` 加文件夹名；`--name` 默认取文件夹名。
- 目标文件夹非空时，`create` 会拒绝写入。
- 生成的 `package.json` 依赖 `fluxdown-plugin-sdk` 和 `typescript`，`npm install` 会把它们装好。

## 2. 生成文件导览

```
mirror-demo/
├── manifest.json                  插件是什么、贡献了什么
├── fluxdown-plugin.config.json    哪些源文件打包成哪个脚本
├── package.json                   脚本：build、validate、pack、dev、typecheck
├── tsconfig.json                  严格模式、ES2022、使用 SDK 的 JSX、不含 DOM
├── .gitignore                     node_modules/、dist/、*.fxplug
└── src/
    ├── env.d.ts                   让 TypeScript 认识全局的 `flux`
    └── main.tsx                   一个计数器视图和一个「Say hello」命令
```

- **`manifest.json`** 是唯一的事实来源。它声明了一个常驻的 `main` 脚本（`main.js`）、一个活动栏视图 `dashboard` 和一个命令面板命令 `hello`。每个字段见 [Manifest 参考](/docs/zh/plugins/manifest/)。
- **`fluxdown-plugin.config.json`** 把 *`dist/` 里的输出脚本* 映射到 *源文件*：`"main.js": "src/main.tsx"`。manifest 里点名的每个脚本，要么在这里列出，要么是项目根目录下的一个现成文件。
- **`src/main.tsx`** 运行在常驻的 `main` 上下文里。`defineMain((flux) => …)` 包住注册代码：`flux.ui.view("dashboard", …)` 注册 manifest 中声明了 `id` 的视图，`flux.commands.register("hello", …)` 注册命令。JSX 标签（`<stack>`、`<button>` 等）就是视图节点，规则见 [SDK 参考](/docs/zh/plugins/sdk/#jsx)。
- **`dist/`** 现在还不存在。它是构建产物——FluxDown 真正安装的就是这个文件夹。

## 3. 先构建一次，装进 FluxDown

```bash
npm run typecheck     # tsc --noEmit：只报类型错误，不写文件
npm run build         # 把 src/ 打包进 dist/，复制 manifest.json，并校验
npm run validate      # 输出：valid (risk: standard)
```

`dist/` 里现在有 `main.js` 和 `manifest.json`。把它作为**开发模式插件**安装：

1. 打开**设置 → 扩展 → 插件**。
2. 在**从目录安装（开发模式）**中选择 **`dist/`** 文件夹（不是项目根目录），点**安装**。在 Web 端，填的是 `dist/` 在**运行 FluxDown 的那台机器**上的绝对路径；服务器或 Docker 容器的话，就是容器内的路径（例如通过挂载进去）。

开发模式插件记录的是文件夹路径而不是拷贝文件，行上有「开发模式」徽标和**重新加载**按钮（提示文字：「重新加载（重读 manifest 与脚本）」）。这个插件没有权限，所以立即启用。你会在活动栏看到带点击计数器的 **Mirror Demo**，命令面板里有 **Say hello**。

开发循环有两条规则，当 `dist/` 就是已安装的文件夹时尤其重要：

- **`npm run build`、`npm run pack` 和 `npm run dev` 在重建前都会删除 `dist/`。** 这一瞬间已安装的文件夹是空的；等构建跑完即可。如果重建之后插件行显示**加载失败**，请在构建结束后点一次**重新加载**。
- **改了 manifest 或 `main` 要点重新加载。** 开发模式插件的 resolver、hooks、auth 和订阅脚本每次调用都会重新读取，对它们来说重建一下就够了。

从现在起，在第二个终端里让监听一直运行：

```bash
npm run dev
```

它每次保存都会重建 `dist/`，并打印 `rebuilt, valid`（或问题清单）。它只在启动时读一次 `fluxdown-plugin.config.json`，所以**改了配置文件要重启它**。

## 4. 添加 resolver

resolver 在任务开始前改写它的 URL。我们的 resolver 把 `https://mirror-demo.invalid/files/<name>` 改写成 `<镜像>/<name>`。

**声明。** 在 `manifest.json` 里加上 `resolvers`，以及镜像地址的 `settings` 条目：

```json
{
  "resolvers": [
    {
      "match": { "urls": ["*://mirror-demo.invalid/files/*"] },
      "entry": "resolve.js"
    }
  ],
  "settings": [
    {
      "key": "mirror",
      "title": "Mirror base URL",
      "description": "Links under mirror-demo.invalid/files/ are downloaded from here instead.",
      "type": "string",
      "default": "https://proof.ovh.net/files",
      "pattern": "^https://"
    }
  ]
}
```

（把这两个键加在已有的键旁边；完整的 manifest 见第 6 步。）`match.urls` 只有 `*` 一种通配符，对整个 URL 匹配；`pattern` 是一个 JavaScript 正则表达式，设置表单用它校验输入。

**打包。** 在 `fluxdown-plugin.config.json` 里加上入口：

```json
{
  "entries": {
    "main.js": "src/main.tsx",
    "resolve.js": "src/resolve.ts"
  },
  "desktop": {},
  "assets": []
}
```

**编写。** 新建 `src/resolve.ts`：

```ts
import { defineResolver } from "fluxdown-plugin-sdk/runtime";

const MARKER = "/files/";

defineResolver((ctx) => {
  const start = ctx.url.toLowerCase().indexOf(MARKER);
  if (start < 0) return null; // 不是我们的：按原始 URL 下载

  // /files/ 之后的全部内容，去掉查询串和片段。
  const name = ctx.url.slice(start + MARKER.length).split(/[?#]/)[0];
  if (!name) return null;

  const mirror = String(flux.settings.mirror).replace(/\/+$/, "");
  const url = `${mirror}/${name}`;
  flux.logger.info("rewriting", ctx.url, "->", url);

  return { url, fileName: name };
});
```

值得注意的地方：

- `defineResolver` 把你的函数安装成引擎调用的全局 `resolve`。函数可以是 `async` 的，这里不需要 `await`。
- `ctx` 的类型是 `ResolveContext`，返回值类型是 `ResolveResult`。返回 `null` 表示「不是我的」；抛异常会让任务明确失败（fail-closed）。
- `flux.settings.mirror` 的类型是 `string | number | boolean | null`，所以用 `String(...)` 包了一层。这个设置有 `default`，所以总是有值。
- 插件的 JavaScript 环境是纯粹的 QuickJS——**没有 `URL` 类**，也没有 `setTimeout`，所以代码里用字符串切片。`lib` 是不含 DOM 的 `ES2022`，TypeScript 会替你拦下 `new URL(...)`。
- 所有逻辑都在函数**里面**执行：FluxDown 在安装时编译脚本，那时 `flux` 并不存在，顶层的 `flux.*` 调用会让安装失败。

**试一试。** 重启 `npm run dev`（配置改过了），等到出现 `rebuilt, valid`，再在插件行上点**重新加载**。然后新建一个 `https://mirror-demo.invalid/files/1Mb.dat` 的下载。这个主机并不存在，也不需要存在：resolver 在建立任何连接之前就把链接改写了。任务会从镜像下载 `1Mb.dat`（默认镜像是 FluxDown 自带演示插件用的公共测试文件主机；想换别的，到插件设置（齿轮按钮）里改**镜像地址**，即 **Mirror base URL**）。日志里会出现：

```
… [plugin:me@mirror-demo] rewriting https://mirror-demo.invalid/files/1Mb.dat -> https://proof.ovh.net/files/1Mb.dat
```

如果没有反应，见[调试与排错](/docs/zh/plugins/debugging/)——最先要怀疑的是 URL 匹配规则，以及忘记点**重新加载**。

## 5. 添加 hook

hook 会在任务状态变化时收到通知。我们的 hook 统计已完成的下载，并记住最近一个文件，用的是 `flux.storage`——因为每次 hook 调用都在全新的 JavaScript 上下文里运行，它是调用之间唯一能保留的状态。

在 `manifest.json` 里**声明**：

```json
{
  "hooks": {
    "entry": "hooks.js",
    "events": ["onDone"]
  }
}
```

`events` 列出插件想接收 `onStart`、`onError`、`onDone`、`onMetaProbed`、`onCancel` 中的哪几个。没有 `hooks.match` 时，hook 对所有任务生效。

**打包。** 在 `fluxdown-plugin.config.json` 的 `entries` 里加上 `"hooks.js": "src/hooks.ts"`，然后重启 `npm run dev`。

**编写。** 新建 `src/shared.ts`——一个 hook 和视图共用的普通模块，这正是打包工具的用武之地：

```ts
/** hook（写入方）与视图（读取方）共用的 storage 键。 */
export const DONE_COUNT_KEY = "doneCount";
export const LAST_FILE_KEY = "lastFile";
```

以及 `src/hooks.ts`：

```ts
import { defineHooks } from "fluxdown-plugin-sdk/runtime";
import { DONE_COUNT_KEY, LAST_FILE_KEY } from "./shared";

defineHooks({
  async onDone(ev) {
    const previous = Number((await flux.storage.get(DONE_COUNT_KEY)) ?? "0");
    await flux.storage.set(DONE_COUNT_KEY, String(previous + 1));
    await flux.storage.set(LAST_FILE_KEY, ev.filePath);
    flux.logger.info("finished", ev.taskId, ev.filePath);
  },
});
```

`ev` 的类型来自事件本身：`onDone` 带有 `taskId`、`url`、`filePath`、`audioPath` 和 `muxed`。storage 的值是字符串（未设置时为 `null`）。hook 抛异常或超时只会往日志里写一条警告，永远不会破坏下载。重试、ffmpeg 和其他事件见[事件钩子指南](/docs/zh/plugins/hooks-guide/)。

**试一试。** **重新加载**之后，让任意一个下载完成。日志里会出现 `[plugin:me@mirror-demo] finished <任务 id> <文件路径>`。

## 6. 添加视图、命令和权限

视图与命令都放在常驻的 `main` 脚本里。我们的视图展示设置和统计数据，并带一个通过 `flux.tasks` 创建下载的按钮。这个接口受权限保护，所以我们要声明权限，并让 FluxDown 去征求用户同意。

最终的 `src/shared.ts`——加上示例 URL（resolver 会把它改写掉）：

```ts
/** hook（写入方）与视图（读取方）共用的 storage 键。 */
export const DONE_COUNT_KEY = "doneCount";
export const LAST_FILE_KEY = "lastFile";

/** 一个会被 resolver 改写的链接；这个主机名不需要真实存在。 */
export const SAMPLE_URL = "https://mirror-demo.invalid/files/1Mb.dat";
```

替换 `src/main.tsx`：

```tsx
import { defineMain } from "fluxdown-plugin-sdk/runtime";
import type { MainFlux, ViewEvent } from "fluxdown-plugin-sdk";
import { DONE_COUNT_KEY, LAST_FILE_KEY, SAMPLE_URL } from "./shared";

interface State {
  doneCount: number;
  lastFile: string;
  note: string;
}

async function loadStats(): Promise<Pick<State, "doneCount" | "lastFile">> {
  return {
    doneCount: Number((await flux.storage.get(DONE_COUNT_KEY)) ?? "0"),
    lastFile: (await flux.storage.get(LAST_FILE_KEY)) ?? "",
  };
}

async function downloadSample(flux: MainFlux): Promise<void> {
  if (!flux.tasks) throw new Error('declare the "tasks" permission in manifest.json');
  await flux.tasks.create({ request: { url: SAMPLE_URL } });
}

defineMain((flux) => {
  flux.ui.view<State>("dashboard", {
    init: async () => ({ ...(await loadStats()), note: "" }),

    render: (state) => (
      <stack gap="md" padding="md">
        <heading text="Mirror Demo" />
        <callout
          tone="info"
          text="Links under mirror-demo.invalid/files/ are downloaded from the mirror set in this plugin's settings."
        />
        <kv
          items={[
            { key: "Mirror", value: String(flux.settings.mirror) },
            { key: "Completed downloads", value: String(state.doneCount) },
            { key: "Last file", value: state.lastFile || "-" },
          ]}
        />
        <stack key="actions" direction="row" gap="sm">
          <button key="sample" text="Download sample file" variant="primary" />
          <button key="refresh" text="Refresh" />
        </stack>
        {state.note ? <text text={state.note} tone="muted" /> : null}
      </stack>
    ),

    async event(state, ev: ViewEvent) {
      if (ev.type !== "press") return;
      switch (ev.node) {
        case "root/actions/sample":
          try {
            await downloadSample(flux);
            state.note = "Sample download added.";
          } catch (error) {
            state.note = `Could not add the download: ${String(error)}`;
          }
          break;
        case "root/actions/refresh":
          Object.assign(state, await loadStats(), { note: "" });
          break;
      }
    },
  });

  flux.commands.register("download-sample", async () => {
    await downloadSample(flux);
    flux.ui.toast("success", "Sample download added");
  });
});
```

它是怎么工作的：

- **`init`** 在视图打开时构造初始状态；**`render(state)`** 把状态变成节点树，必须没有副作用；**`event(state, ev)`** 原地修改 `state`，之后 FluxDown 会重新渲染。
- **节点 id。** 根节点是 `root`；子节点的 id 是它的 `key`，没有 `key` 时用它的位置序号。`<stack key="actions">` 这一行里的按钮 key 分别是 `sample` 和 `refresh`，所以 id 为 `root/actions/sample` 和 `root/actions/refresh`——`ev.node` 带的就是它。同级节点的 key 必须唯一。事件、受控输入和定时器见[可移植界面指南](/docs/zh/plugins/ui-guide/)。
- **JSX 的子节点只能是节点。** 没有 `<button>文字</button>` 这种写法：文字放进 `text` prop，独立文字需要一个 `<text>` 节点。`null` 和 `false` 子节点会被丢弃，所以 `{cond ? <text …/> : null}` 可以用。
- **`defineMain` 递给你的是 `MainFlux`**：全局 `flux` 的类型由所有入口脚本共用，所以 `ui`、`commands`、`rpc`、`events`、`timers` 只在 `defineMain` 传入的 `flux` 上有类型。`flux.tasks` 在类型里是可选的，因为只有 manifest 声明了 `tasks` 权限它才存在。这个判断把「忘了声明」变成一条能读懂的信息，而不是 `TypeError`。
- **`flux.commands.register`** 把处理函数绑定到 manifest 里 `id` 相同的命令；`flux.ui.toast` 弹出通知。
- `main` 里的 `flux.settings` 反映已保存的设置，`flux.storage` 与 hook 写入的是同一个存储。

接着更新 manifest。这是此时**完整**的 `manifest.json`：

```json
{
  "identity": "me@mirror-demo",
  "name": "Mirror Demo",
  "version": "0.1.0",
  "description": "Rewrites mirror-demo.invalid links to a configurable mirror and shows download stats.",
  "resolvers": [
    {
      "match": { "urls": ["*://mirror-demo.invalid/files/*"] },
      "entry": "resolve.js"
    }
  ],
  "hooks": {
    "entry": "hooks.js",
    "events": ["onDone"]
  },
  "main": { "entry": "main.js" },
  "contributes": {
    "views": [
      { "id": "dashboard", "title": "Mirror Demo", "location": "activity" }
    ],
    "commands": [
      { "id": "download-sample", "title": "Mirror Demo: download sample file", "palette": true }
    ]
  },
  "settings": [
    {
      "key": "mirror",
      "title": "Mirror base URL",
      "description": "Links under mirror-demo.invalid/files/ are downloaded from here instead.",
      "type": "string",
      "default": "https://proof.ovh.net/files",
      "pattern": "^https://"
    }
  ],
  "permissions": ["tasks"]
}
```

与第 3 步相比的变化：命令 `hello` 变成了 `download-sample`，并新增了 `permissions`。视图和命令的标题必须是 1–40 个字符；id 必须匹配 `^[a-z][a-z0-9-]{0,31}$`。

**批准权限。** 点**重新加载**。manifest 现在请求了 `tasks`，所以插件行会变成**需要授权**，启用开关被锁住，应用会弹出一个列出所请求能力的授权对话框（可以用**授权**按钮重新打开）。对话框里有风险徽标：`tasks` 属于「较高权限」（只有 `desktop` 才是「完全信任」）。点**授权**，插件随即启用。以后的 manifest 如果请求了*额外*的权限或网络主机，会再走一遍这个步骤。每项权限允许什么、哪些会被审计，见[权限](/docs/zh/plugins/permissions/)。

**试一试。**

1. 在活动栏打开 **Mirror Demo**，能看到统计数据和镜像地址。
2. 点**Download sample file**。会出现一个 `https://mirror-demo.invalid/files/1Mb.dat` 的任务，被你的 resolver 改写后开始下载。
3. 下载完成后点 **Refresh**：*Completed downloads* 增加了，*Last file* 显示文件路径——这是你的 hook 写进去的。
4. 打开命令面板，运行 **Mirror Demo: download sample file**。

## 7. 类型检查、校验与打包

```bash
npm run typecheck
npm run validate      # valid (risk: elevated)
npm run pack          # packed …/me@mirror-demo-0.1.0.fxplug
```

- `typecheck` 以严格设置运行 `tsc --noEmit`；打包前先改掉它报的问题。
- `validate` 以严格模式应用与应用本身、插件索引相同的规则。结果行里包含根据权限推导出的**风险等级**。错误信息与引擎一样是中文，常见的几条在[调试与排错](/docs/zh/plugins/debugging/#安装与校验报错)里有说明。
- `pack` 会构建、校验，并把 `dist/` 压缩为项目根目录下的 `<identity>-<version>.fxplug`，manifest 位于压缩包根目录。压缩结果是确定性的，同样的源码重复打包得到同样的字节。

想像用户一样测试安装包：先**卸载开发模式的那份**（同一个 identity 的开发模式登记优先于已安装的副本），再用**安装 .fxplug / .zip 插件**选择这个文件。由于是全新安装，授权对话框会再次出现。

## 8. 接下来

- **发布：** 每次发版都在 `manifest.json` 里提升 `version`，然后按[打包与市场](/docs/zh/plugins/packaging/)把 `.fxplug` 发布到插件索引。
- **更多能力：** [事件钩子指南](/docs/zh/plugins/hooks-guide/)（重试、ffmpeg、yt-dlp）、[账号登录与认证指南](/docs/zh/plugins/auth-guide/)（登录流程）、[订阅源指南](/docs/zh/plugins/subscription-guide/)、[可移植界面指南](/docs/zh/plugins/ui-guide/)（输入控件、表格、定时器、设置与任务详情页签）、[桌面原生界面指南](/docs/zh/plugins/desktop-ui-guide/)。
- **出了问题：** [调试与排错](/docs/zh/plugins/debugging/)。
- **参考：** [SDK 与 CLI](/docs/zh/plugins/sdk/)、[Manifest](/docs/zh/plugins/manifest/)、[API](/docs/zh/plugins/api-reference/)、[视图](/docs/zh/plugins/views/)。
- **完整示例**在仓库里：[`feature-demo`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk/examples/feature-demo)（覆盖全部能力，附验收清单）和 [`youtube-down`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk/examples/youtube-down)（在插件内自研站点解析、打包第三方脚本、`memoryLimitMb`、多文件清单、DOM 嗅探规则）。
