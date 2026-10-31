---
title: 提交主题
description: 设计主题，连同截图打包，通过 Pull Request 提交到 FluxDown 主题市场。
section: contributing
order: 2
sourceHash: "ca68c14cf4a6"
---

主题市场就是公开仓库 [zerx-lab/fluxdown-themes](https://github.com/zerx-lab/fluxdown-themes)。每个主题占一个目录；Pull Request 合并后，CI 会重新生成仓库的 `index.json`，主题随即出现在[主题市场](/zh/themes/)和客户端里。仓库的 [CONTRIBUTING.md](https://github.com/zerx-lab/fluxdown-themes/blob/main/CONTRIBUTING.md) 是权威规范，本页带你走一遍流程。

## 1. 设计主题

打开[主题编辑器](/zh/theme-builder/)，选一个暗色或亮色基础预设，对照实时预览编辑 token。选中 **Flutter** 客户端时，**导出**得到的就是市场变体文件使用的主题格式（`schemaVersion: 2`）。也可以在客户端 **设置 → 外观** 中调好主题后导出 JSON。

可以只做一个变体（暗色或亮色），也可以两个都做。主题还可以附带 GPUI 客户端版本——市场会用 **GPUI** 徽标标出，编辑器的 **GPUI** 模式导出的就是这种格式（`fluxdown.gpui-theme` v2）。如何放入仓库请以仓库的 CONTRIBUTING.md 为准。

## 2. 组织目录

每个主题放在 `themes/<theme-id>/`。**一个 Pull Request 只允许改动一个主题目录**（CI 强制）。

| 文件 | 必需 | 说明 |
| --- | --- | --- |
| `manifest.json` | 是 | 市场元数据，见下文 |
| `theme.dark.json` / `theme.light.json` | 至少一个 | 导出的主题文件 |
| `screenshot-dark.png` / `screenshot-light.png` | 与变体一一对应 | 见[截图规范](#3-准备截图) |
| `README.md` | 否 | 主题介绍 |

`theme-id` 规则：

- kebab-case：小写字母、数字、连字符（`^[a-z0-9]+(-[a-z0-9]+)*$`），3–40 个字符，全仓库唯一。
- 以下为内置主题保留名，不可使用：`default`、`default-dark`、`default-light`、`midnight-blue`、`nord`、`warm-light`、`graphite`、`graphite-light`。

`manifest.json`：

```json
{
  "manifestVersion": 1,
  "id": "ocean-breeze",
  "name": "Ocean Breeze",
  "author": "your-github-username",
  "version": "1.0.0",
  "description": "A calm deep-sea palette.",
  "tags": ["dark", "blue", "minimal"],
  "variants": {
    "dark": "theme.dark.json",
    "light": "theme.light.json"
  }
}
```

- `id` 必须与目录名一致。
- `version` 为 semver（`x.y.z`），每次更新主题时递增。
- `author` 建议填 GitHub 用户名，市场页会链接到你的主页。
- `tags` 可选，0–8 个，每个不超过 20 个字符。
- `variants` 至少一个键（`dark` / `light`），值为目录内的主题文件名。

每个主题文件里的 `appearance` 必须与变体键一致（`theme.dark.json` 中为 `"appearance": "dark"`）。颜色为 hex 字符串（`aarrggbb` 或 `rrggbb`，可带 `#`）。缺失的颜色会回退到对应明暗度的默认值，但建议填全 `colors` 下的所有分组。`metrics` 可选，`segmentPalette` 最多 32 色。最小示例见 [`themes/ocean-breeze/`](https://github.com/zerx-lab/fluxdown-themes/tree/main/themes/ocean-breeze)。

## 3. 准备截图

- **PNG** 格式，文件名固定为 `screenshot-dark.png` / `screenshot-light.png`。
- 横向窗口比例，宽高比 1.5–1.85（16:10 或 16:9，如 1280×800、1920×1080），宽度 1200–4000px。
- 单张不超过 **500 KB**，可以用 [Squoosh](https://squoosh.app) 压缩。
- 内容为 FluxDown 主界面，含若干下载任务。
- **不得出现个人信息**（真实文件名、URL、用户名等）——下载几个 Linux ISO 作为演示任务就很合适。

## 4. 提交 Pull Request

1. Fork 仓库，新建 `themes/<theme-id>/` 并放入文件。
2. 可选：本地运行 `node scripts/validate.mjs` 校验。
3. 提交 Pull Request，按模板填写。
4. CI 自动校验 manifest 格式、id 唯一性、主题文件格式、截图尺寸与大小，以及是否只改动了一个主题目录。
5. 维护者过目截图后合并，CI 自动更新 `index.json`，主题即上架。

## 更新主题

修改你自己的主题目录，并递增 `manifest.json` 中的 `version`。修改他人主题的 PR 需要原作者确认或由维护者裁定。

提交的主题按仓库的[许可证](https://github.com/zerx-lab/fluxdown-themes/blob/main/LICENSE)分发。
