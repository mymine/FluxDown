---
description: 拦截项目禁止命令与 Rust 忽错门禁绕过
scope: tool:bash
condition:
  - 'cargo\s+test\s+--workspace'
  - 'git\s+push\s+\S*\s*v\d'
  - 'git\s+push\s+.*--tags'
  - 'git\s+(?:-C\s+\S+\s+)?(?:commit|merge|push|rebase|cherry-pick|am|revert)\b[^\n;&|]*\s--no-verify\b'
  - '(?:-A\s*|--allow(?:=|\s+))(?:clippy::)?(?:todo|unimplemented|let_underscore_must_use|let_underscore_future|unused_result_ok|unused_must_use|unsafe_op_in_unsafe_fn|undocumented_unsafe_blocks)\b'
  - '--cap-lints(?:=|\s+)(?:allow|warn)\b'
repeatMode: after-gap
repeatGap: 2
---

该命令属于 FluxDown 项目禁令：

- `cargo test --workspace`：禁止全量跑。用 `cargo nextest run -p <crate> <filter>` 或 `cargo test -p <crate> -- <filter>` 精准跑。静态门禁不同：使用 `cargo clippy --workspace --all-targets -- -D warnings`，含测试目标。
- 推送 `v*` tag 会触发 GitHub Actions 全平台发布流水线：只在用户本会话要求发版时执行；按 `release` skill 第 4 节先让 tag 提交上的全量 CI 与同名演练都 success，再以字面 tag 名推送（`git push origin vX.Y.Z`），此时直接推、不再向用户确认。
- `--no-verify` 绕过 `.githooks` 本地门禁：修复 hook 报告的问题（如 `cargo fmt` 后重新 add）再提交。
- 用 `-A` / `--allow` 关闭 todo、unimplemented、忽错或 unsafe 边界 lint（unsafe_op_in_unsafe_fn / undocumented_unsafe_blocks），或用 `--cap-lints allow/warn` 降低门禁：修复实现及安全论证，不用命令行消音。

omp 会话另有 `.omp/hooks/pre/guard.ts` 在执行前硬拦截上述命令（tag 推送：CI 与演练全绿自动放行，否则交互会话弹确认），本规则负责提示原因。

请换用合规命令后继续。
