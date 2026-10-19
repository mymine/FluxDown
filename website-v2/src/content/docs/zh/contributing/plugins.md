---
title: 发布插件
description: 开发插件，从你自己的 GitHub 仓库发布到 FluxDown 插件市场，并持续更新。
section: contributing
order: 1
sourceHash: "2929a0395619"
---

插件放在**你自己的公开 GitHub 仓库**里。插件市场就是开放索引 [zerx-lab/fluxdown-plugin-index](https://github.com/zerx-lab/fluxdown-plugin-index)：它记录哪个仓库发布哪个插件 identity，并自动收录每一个新的 GitHub Release。只需登记一次，之后发布更新就是发布一个 Release。已发布的插件都能在[插件市场](/zh/plugins/)里看到。

## 1. 开发插件

- 第一次写插件？先看[插件概览](/docs/zh/plugins/overview/)和[写第一个插件](/docs/zh/plugins/your-first-plugin/)。
- 需要 TypeScript、多个源文件或用 JSX 写视图时，使用官方 SDK [`fluxdown-plugin-sdk`](https://www.npmjs.com/package/fluxdown-plugin-sdk)：

  ```bash
  npx fluxdown-plugin-sdk create my-plugin --identity me@my-plugin --name "My Plugin"
  ```

  它用与引擎、索引同一套的 manifest 校验器完成打包、校验与 `.fxplug` 封装，详见[插件 SDK 与 CLI](/docs/zh/plugins/sdk/)。
- 只声明你真正需要的权限（见[权限](/docs/zh/plugins/permissions/)）。用户安装前会看到这些权限，索引机器人也会在你的登记 PR 中列出它们。

## 2. 发布 Release

先用 `fluxdown-plugin validate` 自检插件包，再发布 GitHub Release（非草稿、非预发布），附件中**恰好一个** `.fxplug`，且必须是仓库的 *Latest* Release。`manifest.json` 的 `version` 须为纯 `MAJOR.MINOR.PATCH`，且高于索引中已有的所有版本。

## 3. 登记一次

向索引仓库提一个只修改 `registry.json` 的 Pull Request，追加你的条目：

```json
{ "id": "author@name", "repo": "owner/repo", "tags": ["resolver", "video"] }
```

- `id` 必须等于 manifest 的 `identity`。
- 一个仓库只发布一个插件。
- `tags` 可选（至多 10 个小写标签），会在市场中展示。

机器人会下载你的最新 Release，跑完整准入检查，并在 PR 里评论结果、你声明的权限以及插件拦截的 URL 规则。修好 Release 后向 PR 推送任意提交即可重跑。维护者审核合并后，插件几分钟内就会出现在市场中。

## 4. 发布更新

发布一个版本号更高的新 Release 即可，无需再提 PR。索引每小时检查一次新 Release。无法收录时（identity 不符、版本号没升、manifest 格式错误等），索引仓库会开一个 `stage-fail` 标签的 issue，修好的 Release 收录成功后自动关闭。

完整的准入规则（大小限制、zip 结构、manifest 校验）见[打包与插件市场](/docs/zh/plugins/packaging/#发布到插件索引)以及索引仓库的 [README](https://github.com/zerx-lab/fluxdown-plugin-index#简体中文)。

## 改进 SDK 与插件文档

SDK 源码位于主仓库的 [`packages/plugin-sdk`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk)，与本文档同仓。请向 `main` 分支提交 Pull Request，见[参与文档贡献](/docs/zh/contributing/docs/)。

## 报告已发布插件的问题

插件本身的 bug 请提交到该插件自己的仓库（市场条目中有链接）。如果某个已发布版本有缺陷或有害，请在[索引仓库](https://github.com/zerx-lab/fluxdown-plugin-index/issues)开 issue：维护者可以撤回该版本，标记为 `vulnerable` 或 `malicious` 的版本在已安装的设备上也会被停用。
