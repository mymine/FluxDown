---
description: 按改动范围跑 FluxDown 的门禁（fmt、改动行严格 lint、crate clippy、精准测试、镜像契约、CI 路径），报告实际跑了什么
argument-hint: "[crate / 子项目 / 路径，留空 = 自 origin/main 分叉以来的全部改动]"
---
验证当前 FluxDown 工作区的改动。范围：$ARGUMENTS（留空 = `git merge-base HEAD origin/main` 以来的已提交、暂存、未暂存与未跟踪改动）。

在主会话里**逐条串行**执行（不要派子代理并行跑 cargo / gradle / xcodebuild），遇到第一个失败先修再继续：

1. 定范围：`git status --short` 与 `git diff --stat $(git merge-base HEAD origin/main)`，列出改动落在哪些 crate（`native/*`、`crates/*`）与子项目（`web/`、`fluxDown/`、`packages/plugin-sdk/`、`website-v2/`、`mobile/Android`、`mobile/FluxDown`、`.omp/`、`.github/`）。
2. Rust 有改动：
   - `cargo fmt --check`（失败就 `cargo fmt` 后重跑）。
   - `python3 scripts/lint-changed.py`：改动行 0 条「阻断」；每条「提示」逐一读，能证明下标 / 偏移在范围内才保留，否则改成 `get` / `checked_*` / `split_at_checked`。
   - 每个改动 crate：`cargo clippy -p <crate> --all-targets -- -D warnings`（engine 改了插件相关代码再加 `--features plugins,components`）。
   - 每个改动 crate：`cargo nextest run -p <crate> <filter>`；engine 集成测试是单一二进制 `--test engine`，按模块过滤（如 `--test engine plugin_ffmpeg::`）。**禁止 `--workspace` 测试**。
   - 改了 `native/api`：重跑 `gen_openapi` 并确认 `website-v2/public/openapi.json` 无意外漂移。
   - 改了 engine 但未开 feature 的路径：`cargo check -p fluxdown_engine`（验证关插件时主链路零变化）。
3. 镜像契约：改动文件命中 AGENTS.md §5 索引的任一 `.omp/rules/mirror-*.md`（看规则 `globs`），就按该规则「→」后列出的每一端跑对应测试（共享夹具的 Rust 与 TS 两侧都要跑）。
4. 前端子项目有改动（在各自目录）：
   - `web/`：`bun run lint && bun run test && bun run build`
   - `fluxDown/`：`bun test && npm run typecheck && npm run build`
   - `packages/plugin-sdk/`：`bun run gen:check && bun run typecheck && bun run test`
   - `website-v2/`：`bun run test`
5. 原生移动端有改动：Android `./gradlew :core:testDebugUnitTest :bridge:testDebugUnitTest :app:assembleDebug`；iOS 改了 Rust 先 `scripts/build-core.sh`，再按 AGENTS.md §2 跑 FluxKit `xcodebuild test` 与 App `xcodebuild build` + `python3 scripts/check-i18n.py`。本机缺工具链就明确写「未跑：原因」。
6. 配置有改动：
   - `.github/workflows/ci.yml` 或新增跨目录引用 / crate / path 依赖：`python3 .github/scripts/check_ci_paths.py`。
7. 提交前的全量静态门禁只跑一次：`cargo clippy --workspace --all-targets -- -D warnings`（改动只在单 crate 且第 2 步已过时可跳过并说明）。

最后报告：每条命令 → 通过 / 失败（附错误摘录）/ 未跑（原因）；UI 改动注明「未做 GUI 验证」及建议的人工检查步骤。不要声称没跑过的检查通过。
