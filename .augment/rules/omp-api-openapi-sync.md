---
type: agent_requested
description: "改动 fluxdown_api 后需要重新生成 OpenAPI 规范"
---
<!-- 由 .omp/ 生成（python3 .augment/sync_from_omp.py），请改 .omp 源文件 -->

你正在修改 `native/api`（本机 HTTP API）。注意：

- handler 注解（`#[utoipa::path]`）与 `ToSchema` 派生需同步更新；`openapi.rs` 有漂移守卫测试，路由常量与注解不同步会跑挂。
- 完成后执行：`cargo run -p fluxdown_api --example gen_openapi > website-v2/public/openapi.json`（官网 /api-docs 渲染该文件）。
- wire 契约（`types.rs`）是 camelCase JSON，CLI/server/扩展共用，字段改名是破坏性变更。
- 语义区分：脚本接管入口走外部下载确认流程；aria2/管理 API 直接建任务无弹框。
