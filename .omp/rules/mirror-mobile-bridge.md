---
description: UniFFI DTO / 通用分区 / reducer / 内置分类基线改动须同步 Android 与 iOS 映射层
condition: '[\s\S]'
scope: "tool:edit, tool:write"
globs:
  - '**/native/mobile/src/**'
  - '**/native/protocol/src/{agent,event,method,settings,daemon_config}.rs'
  - '**/mobile/Android/core/src/main/java/com/fluxdown/core/{model,host,store,protocol}/**'
  - '**/mobile/Android/bridge/src/**'
  - '**/mobile/FluxDown/FluxKit/Sources/FluxDomain/**'
  - '**/mobile/FluxDown/FluxKit/Sources/FluxBridge/**'
interruptMode: never
---

你正在改动原生移动端桥接（`native/mobile` ↔ Kotlin / Swift）的一侧。下列条目里，标题是事实源一侧、「→」后是必须同回合同步的镜像端：改任一侧都要把另一侧过一遍；有共享夹具的先改夹具用例，再改两侧实现，两侧测试都要跑（`/verify` 第 3 步）。只改了一侧就交付 = 不完整。

## `native/protocol/src/agent.rs::CustomCategoryDto::builtin_defaults`（内置分类基线）

→ `mobile/Android/core/.../model/Category.kt::BUILTIN` + `mobile/FluxDown/FluxKit/Sources/FluxDomain/Model/TaskCategory.swift::builtin`（同序同扩展名；仅作主机未下发分类时的展示基线，匹配规则来源仍是主机快照）

## `native/protocol/src/event.rs` 的 reducer（`apply_daemon_event` / `apply_engine_message`）

→ `mobile/Android/core/.../store/HostStore.kt` + `mobile/FluxDown/FluxKit/Sources/FluxDomain/Store/HostStore.swift`（移动端渲染子集，`HostStoreTest` / `HostStoreTests` 覆盖同一组：删除哨兵 / 旧采样丢弃 / 非活跃清段 / Stale 只读 / 合帧）；游标与重同步在 `native/mobile`，Kotlin / Swift 只应用已接受的事件

## `native/mobile/src/dto.rs`（UniFFI DTO）与 `native/mobile/src/sections.rs`（通用分区 / 通知键）

→ `mobile/Android/core/.../model/*.kt` + `host/HostSession.kt` 与 `mobile/Android/bridge/.../Mapping.kt`（`:bridge` `MappingTest` 覆盖）；iOS `mobile/FluxDown/FluxKit/Sources/FluxDomain/{Model,Host}/*.swift` 与 `FluxBridge/Mapping.swift`（字段逐一对应；生成绑定只在 `FluxRustBindings` 内部模块，App 不直接引用 `*Dto`）。**通用通道**：`HostSession.call(method, params_json)` 只放行 `daemon.*` / `agent.*`；`HostSnapshotDto.sections` + `HostEventDto::{SectionChanged,Notice}` 承载类型化 DTO 之外的全部 `AgentSnapshot` / `DaemonSnapshot` 字段与一次性通知，键常量 = `sections.rs` 的 `pub const` ↔ iOS `FluxDomain/Protocol/HostSections.swift` ↔ Android `core/.../protocol/HostSections.kt`；方法名 = `native/protocol/src/method.rs` ↔ iOS `FluxDomain/Protocol/Methods.swift` ↔ Android `core/.../protocol/HostMethod.kt`（一一对应，同 `web/src/lib/rpc/protocol/method.ts`）。新增 / 改名分区或通知同回合改这四处；设置目录（`daemon_config.rs` / `settings.rs` 的同步目录）↔ iOS `SettingsCatalog.swift` ↔ Android `core/.../protocol/SettingsCatalog.kt`
