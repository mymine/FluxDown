---
description: 根 Cargo.toml 的 [patch.crates-io]（rquickjs / gpui-fast / gpui-kit / librqbit-dualstack-sockets）与 release profile 的编译期陷阱
condition: '[\s\S]'
scope: "tool:edit, tool:write"
globs:
  - '**/Cargo.toml'
  - '**/Cargo.lock'
  - '**/third_party/**'
interruptMode: never
---

你正在改 Cargo 清单 / 锁文件 / 第三方补丁。以下补丁与配置牵一发动全身，升级或删改前逐条核对：

- **rquickjs**：根 `[patch.crates-io]` 把 crates.io `rquickjs` 换成 gpui-kit 同 commit 的兼容外壳（指向 Longbridge `quickjs-jit` 分叉，gpui-shell 的 LLRT 标准库要求同一 VM 类型），**engine 插件运行时随之使用同一分叉**；升级 gpui-kit 时外壳与 `gpui-shell` 必须同 rev，并重跑 `cargo nextest run -p fluxdown_engine --features plugins,components plugin`。engine 侧仍禁止叠加 `rust-alloc`/`allocator`（会让 `set_memory_limit` 静默失效）；必带 `parallel`（`AsyncRuntime`/`AsyncContext` 的 Send/Sync 依赖它）。gpui-shell 的 LLRT crypto 对 `rsa` 预发布版敏感，`Cargo.lock` 中 `rsa` / `pkcs1` 须与 gpui-kit 锁文件同版本（`cargo update -p rsa --precise …`）。
- **GPUI 核心 = gpui-fast**（`[patch.crates-io]`）：gpui-kit 组件（`gpui-base` / `gpui-component` / `gpui-component-macros` / `gpui-kit-assets`）与 `gpui-shell` 统一 patch 到 gpui-kit main 的同一 commit；升级时 `gpui` / `gpui_platform` 照抄该 commit 的 `=` 版本，并确认 gpui-fast `compat/` 已跟到同一 `gpui-pre` 版本。渲染语义见 `.omp/knowledge/clients.md`「GPUI 核心 = gpui-fast」。
- **iOS 依赖 `third_party/librqbit-dualstack-sockets`**：上游 0.7.0 在 iOS 上不编译（Apple 平台只放行 macOS 的按索引绑定）。删除补丁前先确认上游已修，并重跑 `mobile/FluxDown/scripts/build-core.sh`。
- **`profile.release` 不设 `panic="abort"`**：`download_manager` 靠 `catch_unwind` 恢复 task panic。
- 新增 crate / path 依赖 / `[patch]` 路径后跑 `python3 .github/scripts/check_ci_paths.py`。
