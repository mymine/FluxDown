---
description: protocol 事件/帧判定/队列增量/任务表分页/DTO 改动须同步 agent、GPUI、移动端与 Web 的手写镜像
condition: '[\s\S]'
scope: "tool:edit, tool:write"
globs:
  - '**/native/protocol/src/**'
  - '**/web/src/lib/rpc/**'
  - '**/native/agent/src/{daemon_client,event_hub,event_pump}.rs'
  - '**/native/daemon/src/{event_hub,event_pump}.rs'
  - '**/crates/app/src/{agent_client,session}.rs'
  - '**/native/mobile/src/{projection,driver}.rs'
  - '**/mobile/Android/core/src/main/java/com/fluxdown/core/store/HostStore.kt'
  - '**/mobile/Android/core/src/main/java/com/fluxdown/core/model/QueueOrder.kt'
  - '**/mobile/FluxDown/FluxKit/Sources/FluxDomain/Store/**'
interruptMode: never
---

你正在改动本机服务 wire 协议（`native/protocol` 与各端消费方）的一侧。下列条目里，标题是事实源一侧、「→」后是必须同回合同步的镜像端：改任一侧都要把另一侧过一遍；有共享夹具的先改夹具用例，再改两侧实现，两侧测试都要跑（`/verify` 第 3 步）。只改了一侧就交付 = 不完整。

## `native/protocol/src/event.rs::merge_webhook_deliveries` / `WebhooksCleared`

→ `web/src/lib/rpc/apply.ts::mergeWebhookDeliveries` 与清空事件：按 deliveryId 合并、时间降序、封顶；空增量不清空

## `native/protocol/src/event.rs::judge_frame`（帧覆盖区间 `(previous, sequence]`，可越过游标）与 `coalesce.rs::FrameQueue` 合并规则

→ 全部消费方同一判定：agent `daemon_client.rs::admit_frame`、GPUI `crates/app/src/{agent_client,session}.rs`、`native/mobile/src/projection.rs`、Web `web/src/lib/rpc/cursor.ts::judgeFrame`（`cursor.test.ts` 与 Rust `judge_frame_rules` 同组用例）；合并规则只在 protocol 一处，daemon / agent 转发器（`event_pump.rs`）只调用它

## `native/protocol/src/event.rs::apply_queue_order`（队列增量：先删后按 `order` 插入、名次重写）

→ Web `web/src/lib/rpc/apply.ts::applyQueueOrder`（`apply.test.ts` 对照参考实现）；GPUI `TaskStore` 有序键 / Android `core/model/QueueOrder.kt` / iOS `FluxDomain/Store/QueueOrder.swift` 按同语义维护键与名次；排队序号一律按 `order` 名次计算，不得为队首出队重建全部排队行

## `native/protocol` 的 DTO / 方法 / 事件 / `ErrorReason`（`agent.rs`、`event.rs`、`error.rs`、`method.rs`、`rpc.rs` 版本）

→ `web/src/lib/rpc/protocol/*.ts` 手写镜像 + `apply.ts`；新增严格事件枚举升协议版本；`settings.rs` 同步目录变化会被 Web `syncGroups.test.ts` 核对

## `native/protocol/src/{event.rs,table_sync.rs}` 的**任务表分页载入**（`DaemonSnapshot.{task_total,queue_total}`、`DaemonEvent::TablesReloading`、`TABLE_INLINE_LIMIT`、`method::SYSTEM_TASK_PAGE` = `system.taskPage`、`TablePage*` / `TableSync`；`TaskDto` 默认值字段 `skip_serializing_if`，`daemon.task.list` 无参超 5000 任务即拒绝、带 `{cursor,limit}` 分页）。**快照与整表替换永远不内嵌任务表**：`system.snapshot` / `SnapshotReplaced` / `DaemonSnapshotReplaced` 只带头，`Engine(TasksSnapshot)` / `QueuePositionsChanged` 超过 2000 行在广播边界（`DaemonEvent::into_wire`，daemon `event_hub.rs` 与 agent `event_hub.rs` 的 `publish` 各一处）转成 `TablesReloading`，服务端投影内部仍存完整表；游标 = 任务 id / 队列 `order`，与翻页期间增删无关

→ 所有客户端按同一算法载入（缓冲后续帧 → 逐页 `system.taskPage` → 装入表 → 以基准帧序号为游标按 `judge_frame` 回放，期间不得宣布就绪，`tablesLoading` 期间 UI 只显示加载态、计数 / 全选 / 「任务已删除」推断一律跳过）：agent `daemon_client.rs`（`load_snapshot_tables` 握手载入、`reload_tables` 运行中就地重载并把宣告还原成完整整表事件交给 agent 投影）、GPUI `crates/app/src/{agent_client,session}.rs`（`TablesLoaded`）+ `crates/downloads` `TaskStore` 载入态、`native/mobile/src/driver.rs` + `projection.rs`（`HostSnapshotDto.{tasks_loading,task_total}`）、Web `web/src/lib/rpc/{tableSync,client,apply,normalize}.ts` + `protocol/table.ts`（`TaskDto` 缺省归一化，测试 `client.paging.test.ts` / `apply.paging.test.ts` / `tableSync.test.ts`）、Android `core/.../store/HostStore.kt`（`taskTable`）、iOS `FluxDomain/Store/HostStore.swift`（`tasksLoading`）；agent 内部消费者（`background_effects` / `task_events`）对整表替换帧不读帧内任务，改读 hub 完整投影对账；`MAX_INBOUND_RPC_MESSAGE_BYTES` = 32 MiB，单帧随任务数不再增长（Rust 测试 `two_hundred_thousand_tasks_never_need_a_frame_over_16_mib`）
