---
description: 分类保存目录路由、新建下载分类选择器与一键分类目录名推导须四端同语义
condition: '[\s\S]'
scope: "tool:edit, tool:write"
globs:
  - '**/native/engine/src/category_route.rs'
  - '**/native/engine/src/fixtures/category_route_cases.json'
  - '**/native/agent/src/category_dir.rs'
  - '**/crates/downloads/src/model/save_dir_category.rs'
  - '**/crates/downloads/src/pages/new_download/category.rs'
  - '**/crates/downloads/src/submission.rs'
  - '**/web/src/lib/{categoryRoute,category-dir}.ts'
  - '**/web/src/pages/downloads/dialogs/{CategoryPicker,NewDownloadDialog}.tsx'
  - '**/native/mobile/src/session.rs'
  - '**/mobile/Android/core/src/main/java/com/fluxdown/core/protocol/Categories.kt'
  - '**/mobile/Android/app/src/main/java/com/fluxdown/app/feature/newtask/NewDownloadState.kt'
  - '**/mobile/FluxDown/FluxKit/Sources/FluxDomain/Model/SaveDirCategory.swift'
interruptMode: never
---

你正在改动分类目录路由（引擎 / agent / GPUI / Web / Android / iOS）的一侧。下列条目里，标题是事实源一侧、「→」后是必须同回合同步的镜像端：改任一侧都要把另一侧过一遍；有共享夹具的先改夹具用例，再改两侧实现，两侧测试都要跑（`/verify` 第 3 步）。只改了一侧就交付 = 不完整。

## `native/engine/src/category_route.rs::CategoryRoute::save_dir_for`（分类目录路由，外部捕获与各端新建下载共用：建任务按初始名、开始写盘前按单 GET 首响应名、完成前按最终文件名定落点；路由零网络请求；BT / eD2K / 组成员不路由，清单裂变以基准目录为根）

→ agent `native/agent/src/category_dir.rs::route_save_dir`（确认表单预填）逐条对齐，Rust 两端共用 `native/engine/src/fixtures/category_route_cases.json`（改规则先改用例）；原生 Android `Categories.kt::CategoryRules.saveDirFor` 镜像同一匹配语义；规则 wire = `fluxdown_protocol::CategoryRouteDto`（`from_categories` / `from_preference`）↔ `web/src/lib/rpc/protocol/agent.ts`；「是否跟随分类」= `CategoryRouteDto::{follows, base_dir}`（agent 捕获、GPUI `submission.rs::route_for_batch`、移动端 `native/mobile/src/session.rs` 共用）↔ Web `web/src/lib/categoryRoute.ts`（`routeFromCategories` / `routeFollows` / `routeBaseDir`，`categoryRoute.test.ts` 对照同组语义）。**新建下载表单分类选择器**（保存目录旁「自动 / 不分类 / 指定分类」：自动 = 按重命名 › 首条识别名 › 首条 URL 预测目录并在目录框未被手改时跟随、提交照常路由；不分类 / 指定分类 = 目录框写默认目录 / 分类目录且提交**不路由**；选项 = 可见、非 all、目录非空的分类）四端同语义：预测 GPUI `crates/downloads/src/model/save_dir_category.rs::predict_save_dir` 与 Web `categoryRoute.ts::routeSaveDir` 复用同一夹具，iOS 经 UniFFI `HostSession::category_save_dir`（调 agent `route_save_dir`），Android 用 `CategoryRules.saveDirFor`；状态机 GPUI `pages/new_download/category.rs`、Web `dialogs/CategoryPicker.tsx` + `NewDownloadDialog.tsx`、Android `NewDownloadState.kt`（`SaveDirMode`）、iOS `FluxDomain/Model/SaveDirCategory.swift`

## 「一键分类目录」的目录名推导

→ 当前客户端须与 `web/src/lib/category-dir.ts` 的 `sanitizeCategoryDirName` / `categoryDirUnder` 保持相同分隔符归一与清洗规则；内置分类显示名共用 `assets/i18n` 的 `categoryVideo/...` 键，避免同机桌面与 Web 各建一套目录
