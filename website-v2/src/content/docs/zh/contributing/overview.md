---
title: 贡献方式总览
description: 插件、主题、嗅探规则、翻译、文档与代码分别贡献到哪里、用哪个公开仓库。
section: contributing
order: 0
sourceHash: "88aa49fb927e"
---

FluxDown 在几个公开 GitHub 仓库中公开开发。每类贡献都有各自的仓库和流程——找到与你想分享的内容对应的那一行即可。

| 你想分享 | 仓库 | 如何收录 | 指南 |
| --- | --- | --- | --- |
| 插件（解析器、钩子、视图） | [zerx-lab/fluxdown-plugin-index](https://github.com/zerx-lab/fluxdown-plugin-index) | 登记一次你自己的插件仓库，之后每次 GitHub Release 自动收录 | [发布插件](/docs/zh/contributing/plugins/) |
| 主题 | [zerx-lab/fluxdown-themes](https://github.com/zerx-lab/fluxdown-themes) | 提交包含主题文件与截图的 Pull Request，CI 校验、维护者审核 | [提交主题](/docs/zh/contributing/themes/) |
| 浏览器扩展的嗅探规则 | [zerx-lab/fluxdown-sniff-rules](https://github.com/zerx-lab/fluxdown-sniff-rules) | 维护自己的规则仓库（登记一次），或通过 Issue 表单一次性提交 | [贡献嗅探规则](/docs/zh/contributing/sniff-rules/) |
| 翻译 | [zerx-lab/FluxDown](https://github.com/zerx-lab/FluxDown) | 向 `main` 分支提交 Pull Request | [参与 FluxDown 翻译](/docs/zh/contributing/translations/) |
| 文档修订或代码改动 | [zerx-lab/FluxDown](https://github.com/zerx-lab/FluxDown) | 向 `main` 分支提交 Pull Request | [参与文档贡献](/docs/zh/contributing/docs/) |

## 贡献者工具

- **插件 SDK** —— npm 上的 [`fluxdown-plugin-sdk`](https://www.npmjs.com/package/fluxdown-plugin-sdk)：带类型的 `flux.*` API、用于视图的 JSX 运行时、与引擎和插件索引同一套的 manifest 校验器，以及打包 `.fxplug` 的 CLI。源码位于主仓库的 [`packages/plugin-sdk`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk)，用法见[插件 SDK 与 CLI](/docs/zh/plugins/sdk/)。
- **主题编辑器** —— [主题编辑器](/zh/theme-builder/)对照桌面客户端的实时预览编辑每一个主题 token，并导出主题 JSON。
- **规则仓库模板** —— [zerx-lab/fluxdown-sniff-rules-template](https://github.com/zerx-lab/fluxdown-sniff-rules-template) 用来创建你自己的嗅探规则仓库，自检工作流已经配好。

## 其他参与方式

- 在[反馈页](/zh/feedback/)或 [GitHub Issues](https://github.com/zerx-lab/FluxDown/issues) 报告问题、提出需求。
- 在[功能投票](/zh/feature-vote/)页决定接下来做什么。
- 每页文档底部都有**编辑此页**入口和反馈表单。

## 须知

- 在主仓库中，Pull Request 请提交到 **`main`** 分支——它是开发分支，`stable` 只承载稳定版本。
- 贡献内容按所贡献仓库的许可证接收；主仓库为 AGPL-3.0。
- 插件与嗅探规则的作者在自己的仓库里维护作品，索引仓库只负责登记，发布节奏由你自己掌握。
