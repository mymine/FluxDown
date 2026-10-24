---
description: 主题 token 注册表、文件图标包与 chrome 布局模型改动须同步官网 / Web 镜像与共享夹具
condition: '[\s\S]'
scope: "tool:edit, tool:write"
globs:
  - '**/crates/theme/src/**'
  - '**/website-v2/src/lib/gpui-theme/**'
  - '**/crates/icon_pack/**'
  - '**/assets/icon-packs/**'
  - '**/web/src/lib/icon-pack/**'
  - '**/crates/shell/src/chrome_layout.rs'
  - '**/crates/app/src/chrome_layout.rs'
  - '**/crates/shell/tests/**'
  - '**/web/src/shell/{chromeLayout,chromeCatalog}.ts'
interruptMode: never
---

你正在改动主题 / 图标包 / chrome 布局的一侧。下列条目里，标题是事实源一侧、「→」后是必须同回合同步的镜像端：改任一侧都要把另一侧过一遍；有共享夹具的先改夹具用例，再改两侧实现，两侧测试都要跑（`/verify` 第 3 步）。只改了一侧就交付 = 不完整。

## `crates/theme` 的 token 注册表 / 解析 / 迁移 / 导出（`registry.rs`、`resolve.rs`、`migrate.rs`、`document.rs`、`flutter.rs`）

→ 重跑 `cargo run -p fluxdown_ui_theme --example gen_theme_registry` → `website-v2/src/lib/gpui-theme/registry.json` + `website-v2/public/schemas/gpui-theme.v2.json` + fixtures `*.resolved.json`；`website-v2/src/lib/gpui-theme/*.ts` 逐项对齐并过 `cd website-v2 && bun test tests`。token **只加不改**，改名只走声明式 rename 迁移，已发布 fixtures 永不删除

## `crates/icon_pack`（`pack.rs` 解析 / `svg_is_safe` / 匹配，`registry.rs` 回退链）、`assets/icon-packs/*`（内置包由 `bun scripts/gen_icon_packs.ts` 生成，勿手改；`kinds.json` 扩展名 → 大类）

→ `web/src/lib/icon-pack/{pack,registry}.ts` 逐条对齐，Rust `tests/fixtures.rs` 与 TS `pack.test.ts` 共用 `crates/icon_pack/tests/fixtures/cases.json`；格式只加不改（`schemaVersion` 递增、旧字段保留），偏好值规则 = `fluxdown_protocol::is_icon_pack_ref` ↔ TS `isPackRef`

## `crates/shell/src/chrome_layout.rs`（chrome 布局模型：六区域 / 承载条、归一化、拖放落点）+ `crates/app/src/chrome_layout.rs::catalog`（条目 id、默认区域、可进入的承载条）

→ `web/src/shell/chromeLayout.ts` 逐条对齐，Rust `crates/shell/tests/chrome_layout.rs` 与 TS `chromeLayout.test.ts` 共用 `crates/shell/tests/fixtures/chrome_layout_cases.json`（改语义先改用例）；目录 ↔ `web/src/shell/chromeCatalog.ts`（同 id 同默认位置，Web 无 `shutdown`）。布局两端各存一份、不同步：GPUI 设备本地偏好 `ui.chrome_layout`，Web localStorage `fluxdown.chromeLayout`
