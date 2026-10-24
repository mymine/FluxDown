<!--
  代码 PR 模板 / Code PR template。纯文档 PR 请改用 ?template=docs.md。
  不适用的小节整节删除，不要留空标题。Remove sections that do not apply.
-->

Closes #

## 说明 / Description

<!-- 解决什么问题、改动范围。一个 PR 只做一件事，不夹带无关重构或格式化。 -->
<!-- The problem and the scope. One PR does one thing; no unrelated refactors or formatting. -->

## 契约变化 / Contract changes

<!--
  逐项列出新增 / 修改 / 删除的跨端契约，每项一行：签名 + 一句用途。
  覆盖：protocol DTO / RPC 方法 / 事件 / ErrorReason、HTTP 路由（native/api）、设置键与设备偏好键、
  DB 列、UniFFI DTO（native/mobile）、插件 manifest / 宿主 JS API、扩展 ↔ NMH 消息、CLI 子命令与参数。
  List every cross-client contract item added / changed / removed, with signature and purpose.
-->

- `daemon.task.example`（`native/protocol/src/method.rs`）：`{ id: TaskId } -> ExampleDto` —— 用途

## 破坏性变化 / Breaking changes

<!-- 修改已有契约的项另列于此并给出 diff；协议严格枚举新增需升协议版本（`.omp/rules/mirror-wire-protocol.md`）。 -->

```diff
- old
+ new
```

## 镜像契约 / Mirror contracts

<!-- 改动命中 AGENTS.md §5 索引的任一 `.omp/rules/mirror-*.md` 时，列出同步改了哪一端、两侧测试 / 共享夹具是否都跑过。 -->

## 验证 / Verification

<!--
  实际跑过的命令与结果；本机跑不了的平台写明「未跑：原因」。UI 改动附截图或说明未做 GUI 验证。
  Commands actually run and their results; platforms not exercised and why.
-->

## Checklist

- [ ] 已读 [CONTRIBUTING](../CONTRIBUTING.md) 与 `AGENTS.md`；一个 PR 只做一件事
      Read CONTRIBUTING and AGENTS.md; this PR does one thing
- [ ] `cargo fmt --check`、改动 crate 的 `cargo clippy -p <crate> --all-targets -- -D warnings` 与 `cargo nextest run -p <crate>` 通过
      fmt, clippy and tests of every touched crate pass
- [ ] `python3 scripts/lint-changed.py` 改动行 0 条阻断（Rust 改动）
      No blocking findings on changed lines (Rust changes)
- [ ] 已逐行审阅并确认 AI 生成的代码（如有）正确
      Reviewed and confirmed any AI-generated code
- [ ] 新增 UI 文案只补 `assets/i18n/{en,zh}.json` 基线对，无硬编码
      New UI strings are in the en + zh baseline, none hard-coded
- [ ] 未新增依赖（或已在说明中给出理由并获维护者同意）
      No new dependency (or justified and approved)
- [ ] 改变文档化行为的同时更新了对应文档（官网 / `.omp/knowledge`）
      Documentation updated where documented behavior changed
