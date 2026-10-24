---
description: 非测试 Rust 代码禁止 unwrap/expect（clippy deny 级）
astCondition:
  - $E.unwrap()
  - $E.expect($$$)
globs: "{native,crates}/**/*.rs"
repeatMode: after-gap
repeatGap: 3
---

FluxDown 的 clippy 配置将 `unwrap_used` / `expect_used` 设为 deny 级，非测试代码中出现会直接编译失败。

- 用 `?` 传播错误，错误类型走 `thiserror` 派生（复用 `DownloadError` 等已有类型）。
- `.expect(...)` 同样禁止。
- 例外：`#[cfg(test)]` 模块、`tests/` 目录、doctest 中允许。根 `clippy.toml` 的 `allow-unwrap-in-tests` / `allow-expect-in-tests` 与此一致；`cargo clippy --workspace --all-targets -- -D warnings` 仍检查测试的其他 lint。若当前编辑目标确在测试代码内，可继续原实现。
