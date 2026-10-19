# FluxDown — AI 工作契约（核心）

多协议下载管理器（IDM 的免费替代）。官网 <https://fluxdown.com>（旧域名 `fluxdown.zerx.dev` 长期保留；域名与服务器站点见 `.omp/knowledge/ops.md`「域名与服务器站点」）。Rust 发行物版本由 CI 按 `v*` tag 注入 `FLUXDOWN_APP_VERSION`，运行期基准为 `fluxdown_protocol::APP_VERSION`；protocol 与引擎本地版本均回退 `CARGO_PKG_VERSION`。GPUI macOS 打包默认版本从 `cargo metadata` 的 `fluxdown_ui_app.version` 取得。
**一套 Rust 下载引擎 `fluxdown_engine` + 多宿主 + 多客户端**：PC 为 GPUI（`fluxdown-desktop → fluxdown-agent → fluxdownd → fluxdown_engine`）；移动端为 `mobile/Android`（Jetpack Compose + Flux Lumen，零 Material）与 `mobile/FluxDown`（SwiftUI + Liquid Glass），均经 UniFFI（`native/mobile`）连接同一 Rust 核心。NAS/服务器端 = `fluxdown-agent --server` + `fluxdownd`（React Web SPA 经 agent `/rpc`）。另有 CLI、WXT 浏览器扩展、Tampermonkey 用户脚本、JS 插件系统与 MCP/REST/aria2 API。旧 Flutter 工程及 hub 已移除；共享 `assets/`、现有客户端需要的旧数据/主题/升级兼容与官网旧主题编辑器保留。Android 正式发布使用 `ANDROID_NATIVE_*` 签名，不能覆盖安装旧 Flutter APK。`native/server` 已冻结（不构建/发布，新实现不得依赖它）。

---

## 0. 本文件的边界 · 深挖索引

本文件只收**必须每回合在场**的东西：架构缝、硬不变式、命令、红线、坐标。
枚举性、可从源码复原的细节一律下沉到 `.omp/knowledge/*`（随仓分发，见下方「知识文档只能放 `.omp/`」），**需要时再 `read`**：

| 要查什么 | 读哪个 |
|---|---|
| 架构全图、顶层目录树（哪个目录管什么） | `.omp/knowledge/README.md` |
| 状态码 / DB 表与字段语义、6 种协议、引擎子系统（auto_proxy、RSS、segment_coordinator…）、插件系统、受管组件 | `.omp/knowledge/engine.md` |
| HTTP API 路由组与鉴权、agent / cli / nmh / updater、headless server（agent `--server`）env 与路由 | `.omp/knowledge/hosts-and-api.md` |
| GPUI、原生 Android（`mobile/Android`）与 iOS（`mobile/FluxDown`）、主题兼容、云同步、扩展、用户脚本、Web SPA、官网 | `.omp/knowledge/clients.md` |
| 日志系统细节、发布流水线矩阵、仓库维护自动化（ZerxLabBot 服务器侧：分诊 / 审查 / 自动合并门 / 修复，及与 `ci.yml` 的同步点）、设计文档实现状态（已实现 vs 仅设计，含命名歧义澄清） | `.omp/knowledge/ops.md` |
| **「要加 X 改哪里」全表 —— 动手前先查这张** | `.omp/knowledge/extension-points.md` |

维护约定：
- 只有**架构 / 契约 / 不变式**变化才改本文件；改一个普通文件不必回来更新它。
- 任何文档都**不**维护「完整文件清单 / 完整设置项清单」这类每次提交都漂移的枚举——源码是唯一事实源（`read <dir>` 看结构，`grep` 查事实）。
- 事实层（版本号、协议数、路由、env、DB 列、设置键）以代码为准，文档只给坐标。
- **知识文档只能放 `.omp/`**：`docs/` 与 `.agents/` 都在 `.gitignore` 里（本地目录，**零文件入库、不随仓分发**），`.omp/` 才是随仓分发的 AI 工具链目录。所以附录在 `.omp/knowledge/`；`docs/*.md` 只是本机设计草稿，别把契约写进去。
- **同改矩阵**：改本文件时同一回合把配套面过一遍，别只改一个文件。

| 改了什么 | 同回合必须过一遍 |
|---|---|
| 架构缝 / 不变式（§3–§5） | `.omp/knowledge/*` 对应附录；需要执行期拦截的再加 `.omp/rules/*.md` |
| 任何 `.omp/{RULES.md,WATCHDOG.md,rules,agents,skills}` | 运行 `python3 .augment/sync_from_omp.py` 重新生成 `.augment/{rules,agents,skills}`（Auggie 用；`--check` 校验漂移）；`.augment/hooks/ttsr.sh` 运行时直接读 `.omp/rules`，无需同步。`rule://<名>` 即 `.omp/rules/<名>.md` |
| 红线 / 授权 / 禁用命令（§6–§7） | `.omp/RULES.md`（粘性红线）、`.omp/rules/forbidden-commands.md`、`.omp/WATCHDOG.md`（严重级映射） |
| 跨仓契约 / 仓库地图 | 工作区根 `../AGENTS.md`（cwd=根时自动加载的是那份） |
| 长期结论 / 踩过的坑 | memory：`memory://root/MEMORY.md` + `memory_summary.md` |
| 章节标题（别处会引用） | `grep "AGENTS.md"` 扫全工作区，修掉悬空引用（skills / rules / 源码注释都引过） |

**单一事实源坐标**（要改契约先来这里）：

| 契约 | 位置 |
|---|---|
| 设置键 | `native/protocol/src/{daemon_config,settings}.rs` 的规范目录；下载设置存引擎 `db.rs` 的 `config` 表，客户端偏好由 agent 管理 |
| DB schema | `native/engine/src/db.rs`：`SQLITE_SCHEMA` + `POSTGRES_SCHEMA` + `add_column_if_missing` |
| HTTP 契约 | `native/api/src/types.rs`（wire，camelCase）+ `routes.rs`（路径常量）；规范文件 `website-v2/public/openapi.json`（官网 = `website-v2/`） |
| 本机服务协议基线 / daemon 双向鉴权 | `native/protocol/src/{rpc,handshake}.rs`：角色、版本、挑战应答与会话 HTTP 凭据；daemon 保留旧 agent 静态 Bearer 兼容，新 agent 不发送长期密钥 |
| 慢方法调度 | `fluxdown_protocol::method::SLOW_DAEMON_METHODS`：daemon 并发调度与 agent 独立通道共用，禁止另立清单 |
| 日志文本脱敏 | `fluxdown_logfile::SANITIZE_PATTERNS`：引擎日志与 agent 导出共用唯一正则规则源；结构化快照脱敏另见 `native/daemon/src/log_redact.rs` |
| 移动 FFI | `native/mobile` 的 UniFFI 导出与 DTO；Kotlin / Swift 绑定由各原生工程脚本生成，不手改生成物 |
| headless env / 访问密钥策略 | `native/agent/src/server_mode.rs`（`ServerConfig::from_lookup`、`validate_access_key`） |
| i18n 基线 | 共享 `assets/i18n/{en,zh}.json`，GPUI / Web / Android / iOS 共同消费 |

---

## 1. 执行目录：两种打开方式都要可靠

**本文件所有路径以 `FluxDown/` 为根**，命令按 cwd=`FluxDown/` 书写。

- **cwd = `FluxDown/`**（本文件自动加载）：路径与命令照抄即可。
- **cwd = 上级 `FluxDownProject/` 工作区**（那里的 `AGENTS.md` 自动加载，本文件按需 `read`）：所有路径前置 `FluxDown/`；命令必须带目录限定（bash 工具 `cwd="FluxDown"`、`cd FluxDown && …`、`--manifest-path FluxDown/…`），git 一律 `git -C FluxDown …`。**工作区根既不是 git 仓库也不是工程根**，不带限定必然报错；bash 的 cwd 不跨调用保持。

两份文档分工，互不复制：
- 跨仓地图与跨仓契约（FluxCloud `/api/v1`、插件索引仓、主题仓、发布镜像 secret）→ 上级 `../AGENTS.md`。
- FluxDown 内部架构 / 不变式 / 命令 → 本文件；**内部技术细节以本文件为准**（离代码最近）。
- 两边都写的红线（git 授权门槛、分支模型、禁用命令、i18n 基线）语义必须一致；FluxDown 被单独 clone 时本文件自洽，不依赖上级文件存在。

---

## 2. 命令速查

```bash
# ── 构建 / 静态检查 ──
cargo check -p <crate> --lib          # 验证编译按 crate（不要整 workspace）
cargo fmt --check && cargo clippy --workspace --exclude fluxdown_server --all-targets -- -D warnings   # 提交前必过；含测试目标，冻结的 server 不构建

# ── 测试（按 crate/过滤，不要 --workspace）──
cargo nextest run -p fluxdown_engine <filter>   # 引擎单测（协议/分段/DB）
cargo test -p fluxdown_api            # HTTP API（axum/aria2/MCP/OpenAPI 漂移守卫）
cargo test -p fluxdown_agent          # agent（Gateway / server 模式 / 兼容 API）；server 模式带 SPA：--features web-ui
cargo test -p fluxdown_cli            # CLI（退出码/尺寸解析 doctest）
cargo test -p fluxdown_mobile         # 原生移动端核心（会话游标 / 重连 / DTO 映射 / 进程内本机主机端到端）
PG_TEST_URL=postgres://postgres:pw@localhost/postgres cargo test -p fluxdown_engine -- --ignored pg_smoke
# 插件相关（feature 门控）：
cargo test -p fluxdown_engine --features plugins,components --test plugin_ffmpeg   # 真实执行经 FLUXDOWN_TEST_FFMPEG=<abs> 注入
cargo test -p fluxdown_engine --features plugins,components --test plugin_ytdlp    # FLUXDOWN_TEST_YTDLP=<abs>
cargo check -p fluxdown_engine        # 不带 feature：验证 mobile 关插件时主链路零变化

# ── 各宿主/客户端运行 ──
cargo build -p fluxdown_daemon && cargo run -p fluxdown_agent --features web-ui -- --server   # headless 服务器（fluxdownd 须与 agent 同目录；env 见 .omp/knowledge/hosts-and-api.md）
cargo run -p fluxdown_cli -- ping     # CLI 探活（子命令同上文件）
cargo run -p fluxdown_cli -- add <url> --local   # B 模式：内嵌引擎独立下载

# ── 前端/官网/扩展 ──
cd web && bun run dev                 # Web SPA localhost:5173（/rpc、/api、/ping、/demo 代理到 :17800 的 agent --server）；bun run build → web/dist
cd website-v2 && bun run dev          # 官网 Astro localhost:4321
cd fluxDown && npm run dev            # 扩展开发（Chrome）；dev:firefox / build / zip
cd mobile/Android && ./gradlew :core:testDebugUnitTest :bridge:testDebugUnitTest :app:assembleDebug   # 原生 Android（JAVA_HOME = Android Studio 自带 JBR；:bridge 经 cargo-ndk 编译 fluxdown_mobile）
mobile/Android/scripts/package.sh --version X.Y.Z[-后缀] --signing <签名属性文件>   # 原生 Android release 签名 APK（universal + 各 ABI）；CI = android-package.yml（见 .omp/knowledge/ops.md「原生 Android 打包」）
mobile/FluxDown/scripts/build-core.sh [--release] [--flavor store|full]   # 原生 iOS：编 fluxdown_mobile 为 xcframework + 生成 Swift 绑定（改 Rust 后必须重跑；产物 gitignore）；默认 full，store = 追加 --no-default-features 剔除 BT/eD2K
cd mobile/FluxDown/FluxKit && xcodebuild test -scheme FluxKit-Package -destination 'platform=iOS Simulator,name=iPhone 18 Pro'   # iOS 领域层 + 真实 FFI 冒烟
cd mobile/FluxDown && xcodebuild build -project FluxDown.xcodeproj -scheme FluxDown -destination 'generic/platform=iOS Simulator' && python3 scripts/check-i18n.py   # iOS App 构建 + 文案键校验
mobile/FluxDown/scripts/testflight.sh --version X.Y.Z --build N [--upload]   # iOS App Store 签名 IPA（固定 store flavor）；缺省只 altool 校验，CI = ios-testflight.yml（见 .omp/knowledge/ops.md「iOS TestFlight」）
mobile/FluxDown/scripts/sideload.sh --version X.Y.Z --build N   # iOS 完整版（full flavor）未签名 IPA，供用户侧载自签；CI = ios-sideload.yml

# ── OpenAPI / 图标 / 发布 ──
cargo run -p fluxdown_api --example gen_openapi > website-v2/public/openapi.json   # 改 API 后重生成
bun scripts/gen_icons.ts              # 改 assets/logo/fluxdown_logo.svg 后全平台图标一键生成
git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z   # 触发发布流水线（稳定版从 stable，预览 -rc.N 从 main；见 §6）
```

---

## 3. 架构缝：三个引擎自有 trait

**一个引擎，多个宿主，多个客户端。** 所有下载逻辑集中在 `fluxdown_engine`（`native/engine`，零 UI/FFI 依赖），经三个 trait 与外界解耦：

| Trait | 定义位置 | 方向 | 职责 |
|---|---|---|---|
| `EventSink` | `engine/src/events.rs` | 引擎→宿主 | 进度/分段拆分/队列变化/组变化等事件推送 |
| `HostSelection` | `engine/src/selection.rs` | 引擎→宿主（请求决策） | HLS 画质 / BT 文件 / 插件 variant 选择 / 「文件已存在」询问（tristate：用户选/超时默认/无 selector 短路；新方法给默认实现，无交互宿主自动回退） |
| `ApiHost` | `native/api/src/service.rs` | 客户端→引擎（HTTP 契约） | REST/aria2/MCP 的能力面；必需方法 + 可默认降级方法 |

- 当前生产链路：GPUI 桌面 `fluxdown-desktop → fluxdown-agent → fluxdownd`、headless/NAS `fluxdown-agent --server → fluxdownd`；原生 Android/iOS 经 `native/mobile` 复用进程内 daemon + agent 或远端 `/rpc`。`fluxdown_api` 只依赖 `&dyn ApiHost`，agent 侧 `AgentApiHost` 转发 daemon RPC。CLI 默认 HTTP 连宿主，`add --local` 内嵌引擎。
- **服务边界**：`native/daemon` 是纯下载核心；`native/agent` 常驻承载账户/云同步/设备协同、官方 UI Gateway 与系统外壳（托盘、关闭 UI 后的驻留策略；GPUI 界面进程本身不驻留）；两者共享 `native/protocol` 的 JSON-RPC 语义。`native/server` 已冻结：不构建、不发布、不接收任何改动，新实现不得依赖它。
- **并发模型**：current_thread tokio actor 串行化写；每个下载 spawn 独立 task + CancellationToken；插件 resolve 永不阻塞 actor（off-actor spawn + 通道回流）。
- 客户端捕获三条并行前端进同一本机 RPC（`:17800/download`）：扩展、用户脚本、桌面确认框。

---

## 4. crate 边界与硬不变式

**crate 边界**
- `fluxdown_engine`：零 UI/FFI/axum 依赖，只经 `EventSink`/`HostSelection` 与宿主解耦。协议/分段/DB/队列/组/插件全在这里。
- `fluxdown_api`：依赖 `fluxdown_protocol` 的规范 DTO 与 `&dyn ApiHost`，只定义 HTTP 路径/服务器/兼容层，零引擎依赖。
- `crates/{i18n,theme,icon_pack,components,shell,downloads,settings,account,rss,extensions,plugin_ui,command_palette,app}`：GPUI PC 迁移层；`app` 是唯一 composition root，所有 capability 只依赖本地端口和 protocol DTO。新增页面与 capability 的 crate 边界、目录归属、依赖方向见 `rule://gpui-crate-architecture`。`icon_pack` 是与主题解耦的文件图标包（格式 / 回退链 / 内置包，见 `.omp/knowledge/clients.md`「文件图标包」）。`plugin_ui` 承载插件界面两层：跨端可移植视图渲染器（daemon 常驻宿主产出数据树、帧同步）与仅 GPUI 的桌面脚本（gpui-kit GPUI Shell 运行 view 的 `desktop.entry`）；插件后台逻辑、hook 与常驻宿主只在 daemon，贡献点装配在 `crates/app/src/plugin_contributions.rs`，见 `.omp/knowledge/clients.md`「插件界面」。
- `fluxdown_protocol`：唯一传输无关 wire 层；只能依赖序列化/纯类型能力，不依赖引擎、运行时、数据库、HTTP 或 UI。
- `fluxdown_engine_protocol`：引擎模型与 protocol DTO 的无状态、无损转换边界；宿主使用命名函数，API/agent/UI 不依赖它。
- `fluxdown_daemon`：`fluxdownd` 纯下载核心；独占 engine/下载 DB，拥有任务、队列、组、下载设置、RSS、插件、Webhook 与选择。
- `fluxdown_agent`：`fluxdown-agent` 官方 UI Gateway 与 FluxCloud owner；拥有 Token、设备身份、同步、远程任务、捕获与兼容 API，只经 protocol RPC 调 daemon。
- `fluxdown_mobile`（`native/mobile`）：原生移动端（Android / iOS）唯一 Rust 入口，UniFFI 导出 `FluxCore` / `HostSession`；本机主机 = 进程内 `fluxdown_daemon::runtime::run_with` + `fluxdown_agent` 嵌入模式（`start_embedded` / `LocalConnection`，不起 TCP 网关、NMH、自启、子进程），远端主机 = WebSocket `/rpc` + 访问密钥。握手、epoch/sequence 游标、缓冲、重同步、重连退避与 800ms 离线宽限都在这里，Kotlin / Swift 只应用已接受的信号。手写零 unsafe，UniFFI 宏胶水为独立审计边界。
- `fluxdown_link`（`native/link`）：局域网直连 L1 协议（身份 / 配对 SAS / mDNS / 直连传输 / 地址解析），持久化经 `LinkStorage` trait 注入；agent 为生产宿主。零引擎、零数据库、零 UI 依赖。
- `fluxdown_logfile`（`native/logfile`）：desktop 与 agent 共用的诊断日志文件（轮转 + 重复限流 + panic hook），零第三方依赖；`log` / `tracing` 适配留在各宿主。细节见 `.omp/knowledge/ops.md`「日志系统」。
- **feature 门控**：`plugins`、`components`（默认关；desktop/server 开，mobile/CLI 关）。**关插件时下载主链路零行为变化**（注入 no-op `PluginManager`）。`bt`、`ed2k`（engine / engine_protocol / daemon / cli 默认开）与 `fluxdown_mobile` 的 `p2p`（default 唯一成员）：iOS 商店 flavor 以 `--no-default-features` 编译期剔除 librqbit 与 eD2K，建此类任务返回 `BT_DISABLED_MESSAGE` 类明确拒绝；Swift 侧对应条件 `FULL_P2P`（store 不定义，商店包不得残留 BT/eD2K 入口、scheme、文档类型，Apple 2.3.1 / 5.2.3），远端主机上的 BT 任务仍可显示与控制。细节见 `.omp/knowledge/clients.md` iOS 段。

**编译期陷阱**
- rquickjs：根 `[patch.crates-io]` 把 crates.io `rquickjs` 换成 gpui-kit 同 commit 的兼容外壳（指向 Longbridge `quickjs-jit` 分叉，gpui-shell 的 LLRT 标准库要求同一 VM 类型），**engine 插件运行时随之使用同一分叉**；升级 gpui-kit 时外壳与 `gpui-shell` 必须同 rev，并重跑 `cargo nextest run -p fluxdown_engine --features plugins,components plugin`。engine 侧仍禁止叠加 `rust-alloc`/`allocator`（会让 `set_memory_limit` 静默失效）；必带 `parallel`（`AsyncRuntime`/`AsyncContext` 的 Send/Sync 依赖它）。gpui-shell 的 LLRT crypto 对 `rsa` 预发布版敏感，`Cargo.lock` 中 `rsa` / `pkcs1` 须与 gpui-kit 锁文件同版本（`cargo update -p rsa --precise …`）。
- `profile.release` **不**设 `panic="abort"`——`download_manager` 靠 `catch_unwind` 恢复 task panic。
- **iOS 构建依赖 `third_party/librqbit-dualstack-sockets`**（根 `Cargo.toml` `[patch.crates-io]`）：上游 0.7.0 在 iOS 上不编译（Apple 平台只放行 macOS 的按索引绑定）。删除补丁前先确认上游已修，并重跑 `mobile/FluxDown/scripts/build-core.sh`。
- **GPUI 核心经 `[patch.crates-io]` 换成 gpui-fast**（Retained Mode：未变 view 复用上一帧）：render 读的状态必须在 entity / global / `ListState` / `ScrollHandle` 内，否则改了要 notify 读者 view；父 view notify 不再重建子 view；渲染期不得无条件写 entity / global。gpui-kit 组件（`gpui-base` / `gpui-component` / `gpui-component-macros` / `gpui-kit-assets`）与 `gpui-shell` 统一 patch 到 gpui-kit main 的同一 commit；升级时 `gpui` / `gpui_platform` 照抄该 commit 的 `=` 版本，并确认 gpui-fast `compat/` 已跟到同一 `gpui-pre` 版本。细则见 `.omp/knowledge/clients.md`「GPUI 核心 = gpui-fast」。
- **headless 的 Web UI 是编译期内嵌的**：`fluxdown_agent` 的 `web-ui` feature 下 `native/agent/build.rs` 把 `FLUXDOWN_EMBED_WEBROOT`（缺省 `web/dist`）整棵目录递归全量 `include_bytes!` 进二进制，只在 `--server` 模式挂为 SPA fallback。改了前端**必须先 `cd web && bun run build` 再重编 agent**才能看到；`FLUXDOWN_WEBROOT` 是可选的磁盘覆盖。构建时目录缺失只 warning + 运行期 503 提示页。Web 构建经 Vite 别名引用仓库根的 `assets/i18n` 与 `website-v2/src/lib/gpui-theme`，打包上下文必须包含这两处。

**运行期不变式**
- 持有引擎的宿主 actor **必须** drain `resolve_rx`（off-actor 插件解析回流）与 `plugin_retry_rx`，否则命中 resolver 的下载永久挂起。
- pg 字节列必须 `BIGINT`（`INTEGER` 会在 >2GB 静默截断）；新表要同时进 `SQLITE_SCHEMA` + `POSTGRES_SCHEMA` + 迁移。
- **每个 console 子进程 spawn 都要包 `proc::no_console_window`**（ffmpeg/ffprobe/yt-dlp/tar/探版），否则 Windows 闪黑窗。
- **anyhow 错误一律 `{e:#}`**：`{e}` 只输出最外层 context，会整个吞掉根因（BT 的 `dht.json` 撞 Windows 端口排除区间导致全部 BT 任务 status=4，就是这么被吞掉的；DHT 持久化是纯缓存，`SharedBtSession::new` 三级兜底）。
- **BT 判定只认 `magnet:` 与 `torrent-file://` 哨兵**。HTTP 的 `.torrent` 直链不会走 BT，会被当普通文件下回一个种子文件；要变成真下载必须先抓字节再以 `NewTaskSpec::torrent_file_bytes` 建任务（RSS 就是这么做的）。
- **「复制链接」类 UI 一律读 `origin_url`，空则回退 `url`**（torrent 任务的 `url` 是哨兵）。
- **RSS 是无人值守链路**：任何「需要用户点一下才能继续」的东西都是 bug。建任务即落全选 + `unattended=1`（否则启动时会弹 N 次文件选择框）；引擎内部自发建任务也必须由宿主发布完整任务事实，不能仅发缺少队列等字段的进度；手动「重新下载」对**任何**状态放行。
- **一个 data_dir 同时只有一个引擎写入者**：`Db::open_exclusive` / `connect_exclusive` 持 `<data_dir>/engine.lock`（PG 另加 advisory lock），第二个打开者得 `DbError::WriterLeaseHeld`。CLI `--local` 直接报「App 正在运行」退出；**`fluxdown_nmh` 冷启动只拉 `fluxdown-agent`**，不得回退旧 Flutter 可执行（升级目录可能残留 `flux_down`，启动它会抢走锁让 fluxdownd 失效）。
- **空闲静默（NAS 硬盘休眠）**：无活动/排队任务、无做种、无到期 RSS、无客户端主动请求时，daemon/agent 不得周期性读写 save_dir 或 data_dir（不 fsync 的写也会被内核回写唤醒机械盘）。新增周期任务必须事件驱动、受空闲判定门控，或长周期且比对后仅在内容变化时写；定时文件跟踪扫描受 `idle_file_scan`（默认关）门控，新鲜度靠客户端获焦 / 页面可见时 `daemon.task.rescan`。落点与阻碍项清单见 `.omp/knowledge/hosts-and-api.md`「空闲静默」。
- **宿主自身下载只走统一出口**（应用更新、受管组件、插件包、规则包、tracker / server.met 订阅等，新增同类下载同样适用；FluxCloud / 统计等自家 API 调用不在此列）：daemon / engine 内用 `fluxdown_engine::egress`，agent 经 `daemon.egress.plan` + `native/agent/src/egress.rs`，此类下载禁止裸 `reqwest::Client::builder()`。与下载任务同一全局代理；只有内容有独立来源哈希 / 签名的载荷才按下载任务的 resolver 规则交给插件改写（GitHub 加速等），改写失败或内容不符回退原地址；校验和 / 索引 / 版本清单不交给插件。细节见 `.omp/knowledge/engine.md`「egress.rs」。
- 引擎学习/遥测类 config 键（`cdn_node_health`、`auto_route_health`、`cdn_pending_reports`、`domain_conn_caps`）**UI 不读写**。
- **遥测只有两条匿名部署事件**（`app_installed` 一次 + `app_active` 每日，`analytics_enabled` 门控），**绝不**采集下载/任务信息——不要新增遥测点。
- **云端推送不是遥测**：agent 把任务事件（`DaemonEvent::TaskNotice`，与 webhook 同源）交给 FluxCloud `/api/v1/notifications/events` 代发，是用户为自己开的投递功能。硬约束：本设备上报默认关、开启必须经 UI 显式同意、与 `analytics_enabled` 完全无关；默认只发文件名 / 大小 / 状态 / 错误 / 队列名，下载地址与保存目录各自单独开关（默认关）；`TaskNotice` 只由 agent 消费、不转发给 UI；云端保留 7 天。额度、渠道、投递全在云端（FluxCloud entitlement `notify*` 字段）。
- 命名歧义：`tracker_subscription.rs` / `ed2k/server_subscription.rs` 是 BT tracker 列表 / ED2K `server.met` 订阅，与 `rss/` 的 feed 订阅无关；官网 `api/webhooks/github` 是 GitHub 接收器，与 `engine/src/webhook.rs` 的任务事件推送无关。

---

## 5. 镜像契约：改一处必须同步另一处

| 改这里 | 必须同步 |
|---|---|
| `engine/src/rss/filter.rs` | `web/src/pages/rss/filter.ts`（两份逐条对齐，`filter.test.ts` 复用 Rust 用例；预览与实际下载必须一致） |
| `native/engine/src/naming/{disposition,charset}.rs`（Content-Disposition 解析、旧式字节打分解码、TLD / 页面字符集先验） | `fluxDown/utils/filename.ts`（`parseContentDispositionFilename` / `decodeLegacyBytes`）逐条对齐；跨实现用例 `native/engine/src/naming/fixtures/content_disposition.json` 由 Rust 测试与 `filename.test.ts`（Chrome / Firefox 两种头值模型）共用，改规则先改用例。扩展发来的非空名按显式名处理（只 sanitize），拿不准（脚本端点名等）必须发空串，交给引擎推断并在完成期按实际 GET 响应定名（`tasks.name_inferred`，见 `.omp/knowledge/engine.md`「naming/」） |
| `crates/downloads/src/model/new_download.rs::capture_entry`（捕获名与 URL 末段百分号解码后相同则省略 `out=`） | `web/src/pages/downloads/dialogs/model.ts::captureEntry`：同一判定逐条对齐（`a%20b.zip` 与 `a b.zip` 视为相同），两端新建框显示一致 |
| `crates/downloads/src/model/new_download.rs::parse_entries` / `parse_input` + `model/aria2c.rs`（aria2c 命令切词 / 续行合并 / 镜像 URI 只取首个、`-Z` 拆分 / 带值选项表 / 头部回填 `plan_field_fill`） | `web/src/pages/downloads/dialogs/model.ts::parseEntries` + `aria2c.ts`（`parseCommand` / `joinContinuations` / `planFieldFill`）逐条对齐，`aria2c.test.ts` 覆盖同组行为；TXT 导入两端都用「原文保留、按 URL 去重追加」（`append_entries` ↔ `appendEntries`） |
| `crates/downloads/src/components/task_table.rs::TaskMenuFacts::{can_change_url, can_change_segments, can_verify_integrity}`（`task_menu_facts_for` 为右键 / 应用菜单与详情常规页按钮共用事实源）+ `pages/task_edit.rs`（`open_change_url_dialog` / `open_change_segments_dialog`，下载页与 `task_detail.rs` 常规页共用；失败任务换链 / 改线程数成功后串行 Resume）+ `pages/task_verify.rs`（BT 完整性校验结果通知：损坏文件列前 3 个、其余折叠计数，`conflict` = 已在校验） | `web/src/pages/downloads/model/taskEdit.ts::{canChangeUrl, canChangeSegments, canVerifyIntegrity, parseTaskSegments}` + `table/menus.ts` 与 `detail/GeneralTab.tsx` 两处入口 + `dialogs/TaskEditDialogs.tsx`（`resumeIfFailed`）+ `model/actions.ts::verifyTaskIntegrity`；换链源仅 http(s)/ftp（对齐引擎 `change_task_url`） |
| `crates/downloads/src/model/dispatch.rs::remote_action_applies`（远程命令适用状态） | `web/src/pages/downloads/model/batchPlan.ts::remoteCan`；未知状态一律不可控制 |
| `crates/downloads/src/model/view_prefs.rs::compare`（智能排序档位 / 平局 / 自然序 / 表头三档）+ `model/row_order.rs`（行顺序保持期） | `web/src/pages/downloads/model/viewPrefs.ts::compareViews` + `rowOrder.ts`（逐条对齐，`viewPrefs.test.ts` / `rowOrder.test.ts` 覆盖同一组行为） |
| `crates/downloads/src/components/task_table.rs` 宽松密度列表视图（`DownloadColumnKind::shown` 并列规则 / `relaxed_meta` / `relaxed_status_detail`） | `web/src/pages/downloads/model/viewPrefs.ts::columnShown` + `table/text.ts::relaxedMeta` / `relaxedStatusDetail` + `table/cells.tsx`：主列恒显示、大小 / 进度 / 速度 / 剩余时间并入主列、元信息字段与顺序逐条对齐 |
| `crates/downloads/src/model/source_composition.rs`（详情常规页「来源构成」区块：P2P 判定 / 超额按比例缩放 / 行纳入规则 / 百分比格式；常规页信息列 300 + 来源区块 360 按可用宽度折行） | `web/src/pages/downloads/model/sourceComposition.ts` + `detail/GeneralTab.tsx` / `SourcesSection.tsx`（逐条对齐，`sourceComposition.test.ts` 复用同一组用例）；数据源 `tasks.src_{cdn,proxy,nic}_bytes` 只记加速路径，源站 = 已下载 − 三者之和，进度复位时同步清零 |
| `crates/theme` 的 token 注册表 / 解析 / 迁移 / 导出（`registry.rs`、`resolve.rs`、`migrate.rs`、`document.rs`、`flutter.rs`） | 重跑 `cargo run -p fluxdown_ui_theme --example gen_theme_registry` → `website-v2/src/lib/gpui-theme/registry.json` + `website-v2/public/schemas/gpui-theme.v2.json` + fixtures `*.resolved.json`；`website-v2/src/lib/gpui-theme/*.ts` 逐项对齐并过 `cd website-v2 && bun test tests`。token **只加不改**，改名只走声明式 rename 迁移，已发布 fixtures 永不删除 |
| `crates/icon_pack`（`pack.rs` 解析 / `svg_is_safe` / 匹配，`registry.rs` 回退链）、`assets/icon-packs/*`（内置包由 `bun scripts/gen_icon_packs.ts` 生成，勿手改；`kinds.json` 扩展名 → 大类） | `web/src/lib/icon-pack/{pack,registry}.ts` 逐条对齐，Rust `tests/fixtures.rs` 与 TS `pack.test.ts` 共用 `crates/icon_pack/tests/fixtures/cases.json`；格式只加不改（`schemaVersion` 递增、旧字段保留），偏好值规则 = `fluxdown_protocol::is_icon_pack_ref` ↔ TS `isPackRef` |
| `crates/shell/src/chrome_layout.rs`（chrome 布局模型：六区域 / 承载条、归一化、拖放落点）+ `crates/app/src/chrome_layout.rs::catalog`（条目 id、默认区域、可进入的承载条） | `web/src/shell/chromeLayout.ts` 逐条对齐，Rust `crates/shell/tests/chrome_layout.rs` 与 TS `chromeLayout.test.ts` 共用 `crates/shell/tests/fixtures/chrome_layout_cases.json`（改语义先改用例）；目录 ↔ `web/src/shell/chromeCatalog.ts`（同 id 同默认位置，Web 无 `shutdown`）。布局两端各存一份、不同步：GPUI 设备本地偏好 `ui.chrome_layout`，Web localStorage `fluxdown.chromeLayout` |
| `agent/src/server_mode.rs::validate_access_key` | `web/src/lib/token-policy.ts` + OpenWrt LuCI `packaging/openwrt/luci/www/luci-static/resources/view/fluxdown.js::validateToken`（UCI `token` → `FLUXDOWN_TOKEN` + `FLUXDOWN_TOKEN_FORCE=1`） |
| `crates/downloads/src/model/devices.rs::other_devices`（云设备 + 直连设备合并：`cloud_device_id` 命中云名册即挂接为一条、在线取任一、直连在线优先走直连下发、revoked 排除、合并后同名消歧；侧栏只列云条目含挂接） | Web `web/src/pages/downloads/model/devices.ts` + `dialogs/target.ts`、iOS `FluxKit/Sources/FluxDomain/Protocol/Devices.swift`、Android `mobile/Android/core/.../protocol/Devices.kt` 逐条对齐，各端测试覆盖同一组行为；直连设备不注册 FluxCloud、不计入云设备数 / 在线名额 |
| `engine/src/webhook.rs` 的 `WebhookEventKind` | protocol 与各客户端事件选择器（含 TS `WEBHOOK_EVENTS`），wire 名逐字一致 |
| `engine/src/webhook.rs` 的 `EndpointSpec` 宽松解析（`lenient` / `reload_endpoints`） | GPUI `crates/settings/src/sections/webhook.rs::parse_endpoints` + Web `web/src/pages/webhooks/endpoints.ts::parseEndpoint`：非对象元素跳过、字段类型不符回退默认值；端点写入两端都走「取最新值重算、冲突重放」（GPUI `SettingsStore::mutate_daemon` ↔ Web `patchEndpoints`），不得写回整份旧数组 |
| `native/protocol/src/event.rs::merge_webhook_deliveries` / `WebhooksCleared` | `web/src/lib/rpc/apply.ts::mergeWebhookDeliveries` 与清空事件：按 deliveryId 合并、时间降序、封顶；空增量不清空 |
| `native/api` 契约 | 重跑 `gen_openapi` 覆盖 `website-v2/public/openapi.json` |
| `native/protocol` 的 DTO / 方法 / 事件 / `ErrorReason`（`agent.rs`、`event.rs`、`error.rs`、`method.rs`、`rpc.rs` 版本） | `web/src/lib/rpc/protocol/*.ts` 手写镜像 + `apply.ts`；新增严格事件枚举升协议版本；`settings.rs` 同步目录变化会被 Web `syncGroups.test.ts` 核对 |
| 任一 UI 文案 | 只补 **en + zh 基线对**：GPUI/Web SPA/原生 Android/iOS 共用 `assets/i18n/{en,zh}.json`（Web 经 `web/src/i18n` 查表；Android 构建期生成 `R.string.<键>`，禁止在 `res/` 手写同名文案；iOS 经 FluxKit 符号链接打包同一 JSON、`L("键")` 查表，`scripts/check-i18n.py` 校验漏键）；官网主站 `website-v2/src/i18n/messages/<ns>.ts`（`defineMessages({ en, zh })`）；`fluxDown/utils/locales/{en,zh-CN}.ts`。社区语言（`ja` 等）由 Weblate 维护，**不碰**。删共享键前检查 `crates/`、`web/src/` 与 `mobile/` 全部消费者 |
| web 设置项 / 对话框字段归属 | **基准 = GPUI 桌面客户端**：Web 设置分类与字段顺序、对话框分区对齐 `crates/settings` / `crates/downloads`（`web/src/pages/settings/categories.ts` ↔ `crates/settings/src/view.rs::build_pages`）。桌面专属项（托盘、自启、关联、剪贴板、打开文件/所在目录、进度窗口）在 Web 省略，其余不得各自措辞或另立分类 |
| 「一键分类目录」的目录名推导 | 当前客户端须与 `web/src/lib/category-dir.ts` 的 `sanitizeCategoryDirName` / `categoryDirUnder` 保持相同分隔符归一与清洗规则；内置分类显示名共用 `assets/i18n` 的 `categoryVideo/...` 键，避免同机桌面与 Web 各建一套目录 |
| 开机自启与旧版迁移 | `native/agent/src/platform/autostart.rs`：「已启用」尊重系统级禁用（Windows `StartupApproved`、XDG `Hidden` / `X-GNOME-Autostart-enabled`），启动时自动迁移只改启动目标、**绝不**改系统启用状态；见 `.omp/knowledge/clients.md`「开机自启与旧版迁移」 |
| 文件跟踪重扫节流（`crates/downloads/src/model/file_rescan.rs::RescanThrottle`） | `web/src/lib/rescanThrottle.ts`（`RescanThrottle`）：逐条对齐 10s 冷却 / 尾沿排队 / 合并 / 尾沿后重计冷却；测试复用同组用例（`rescanThrottle.test.ts`） |
| 线程数上限与风险档（`native/protocol/src/daemon_config.rs`：`MAX_TASK_SEGMENTS` = 512 / `HIGH_SEGMENTS_WARN_ABOVE` = 64 / `SEVERE_SEGMENTS_WARN_ABOVE` = 256） | 引擎 `segment_coordinator::MAX_SEGMENTS`（显式线程数硬上限；Auto 仍由 advisor / `HINT_UNCAP_MAX` 封顶 64）+ `web/src/lib/threadsRisk.ts` + Web `config.ts` 与移动端 `SettingsCatalog` 的 `default_segments` 范围；GPUI / Web 在设置、队列、新建下载三处 > 64 动态展示风险提示（`RevealCallout`） |
| 应用内更新界面判定（`crates/settings/src/update_view.rs`：阶段 → 文案键、手动原因 / 失败键、`can_install` / `can_cancel` / 手动下载地址） | `web/src/lib/update.ts` + 原生 Android `mobile/Android/core/.../update/UpdateView.kt`：逐条对齐（`update.test.ts` / `UpdateViewTest` 覆盖同一组；Android 仅 Ready / Installing / 用户取消 / 商店安装换用 `mobileUpdate*` 文案键）；状态与流程只归 agent `native/agent/src/update/`（Android 经 `native/mobile` 的 `AppUpdater` 复用同一 `UpdateService`，Kotlin 只做安装 / 调度 / 界面），发布资产 / 校验和哨兵 / 双域名依赖见 `.omp/knowledge/hosts-and-api.md`「应用内更新」 |
| 更新说明展示规则（`crates/settings/src/sections/about.rs`：`localized_release_body` 按界面语言切段、`strip_version_heading` 去掉正文开头与标题重复的版本标题、`release_date` 取发布日期；标题统一为「v版本 + 发布日期」，关于页与新版本提醒窗口共用） | `web/src/lib/releaseNotes.ts` + 原生 Android `mobile/Android/core/.../update/UpdateView.kt`（`localizedReleaseBody` / `stripVersionHeading` / `releaseDate`）逐条对齐，`releaseNotes.test.ts` / `UpdateViewTest` 与 Rust 测试为同一组用例；Web 不引入 Markdown 渲染器，`releaseNoteBlocks` 只拆标题 / 列表 / 段落 |
| 云端推送界面判定（`crates/settings/src/model/cloud_notify.rs`：catalog 开关与标签可见性、卡片状态优先级、用量 0 = 不限 / 满额与重置时间、隐私摘要、默认标签） | `web/src/pages/webhooks/cloudLogic.ts`：逐条对齐（`cloudLogic.test.ts` 与 model 测试共用同组样例）。云端只有 email / telegram（依赖 FD 的 SMTP 与官方机器人）；不依赖 FD 基础设施的推送服务一律做成引擎自托管 Webhook 预设（`engine/src/webhook.rs::Preset`），不进云端 |
| 套餐购买与推介判定（`crates/account/src/billing.rs`：金额格式化、生效价 / 可购买过滤、卡片状态优先级 Current > TierBlocked > CreditBlocked、应付与划线价、倒计时、订单轮询动作、推荐码规范化与创建校验、validate reason / 购买类 `ErrorReason` → 文案键；当前套餐以 `user.plan` 为准） | `web/src/pages/settings/sections/account/billing.ts`：逐条对齐（`billing.test.ts` 与 Rust 测试为同一组用例）。推介页入口 = 设备本地偏好 `ui.show_settings_referral`（不进同步目录，缺省 false）且已登录，GPUI 设置侧栏与 Web `categories.ts` 同位（紧随「账户」） |
| `native/protocol/src/agent.rs::CustomCategoryDto::builtin_defaults`（内置分类基线） | `mobile/Android/core/.../model/Category.kt::BUILTIN` + `mobile/FluxDown/FluxKit/Sources/FluxDomain/Model/TaskCategory.swift::builtin`（同序同扩展名；仅作主机未下发分类时的展示基线，匹配规则来源仍是主机快照） |
| `native/protocol/src/event.rs` 的 reducer（`apply_daemon_event` / `apply_engine_message`） | `mobile/Android/core/.../store/HostStore.kt` + `mobile/FluxDown/FluxKit/Sources/FluxDomain/Store/HostStore.swift`（移动端渲染子集，`HostStoreTest` / `HostStoreTests` 覆盖同一组：删除哨兵 / 旧采样丢弃 / 非活跃清段 / Stale 只读 / 合帧）；游标与重同步在 `native/mobile`，Kotlin / Swift 只应用已接受的事件 |
| `native/mobile/src/dto.rs`（UniFFI DTO）与 `native/mobile/src/sections.rs`（通用分区 / 通知键） | `mobile/Android/core/.../model/*.kt` + `host/HostSession.kt` 与 `mobile/Android/bridge/.../Mapping.kt`（`:bridge` `MappingTest` 覆盖）；iOS `mobile/FluxDown/FluxKit/Sources/FluxDomain/{Model,Host}/*.swift` 与 `FluxBridge/Mapping.swift`（字段逐一对应；生成绑定只在 `FluxRustBindings` 内部模块，App 不直接引用 `*Dto`）。**通用通道**：`HostSession.call(method, params_json)` 只放行 `daemon.*` / `agent.*`；`HostSnapshotDto.sections` + `HostEventDto::{SectionChanged,Notice}` 承载类型化 DTO 之外的全部 `AgentSnapshot` / `DaemonSnapshot` 字段与一次性通知，键常量 = `sections.rs` 的 `pub const` ↔ iOS `FluxDomain/Protocol/HostSections.swift` ↔ Android `core/.../protocol/HostSections.kt`；方法名 = `native/protocol/src/method.rs` ↔ iOS `FluxDomain/Protocol/Methods.swift` ↔ Android `core/.../protocol/HostMethod.kt`（一一对应，同 `web/src/lib/rpc/protocol/method.ts`）。新增 / 改名分区或通知同回合改这四处；设置目录（`daemon_config.rs` / `settings.rs` 的同步目录）↔ iOS `SettingsCatalog.swift` ↔ Android `core/.../protocol/SettingsCatalog.kt` |
| 插件 manifest 规则（`native/engine/src/plugin/manifest.rs`：`main` / `contributes` / 权限 / `risk()` / `network_hosts()` / sniffRules 内容） | 先改跨实现夹具 `native/engine/src/plugin/fixtures/manifest_cases.json`；SDK `packages/plugin-sdk/src/manifest.ts` 逐条对齐（`manifest.test.ts` 共用夹具），再 `bun run sync-index --index <插件索引仓>` 更新索引仓 `vendor/fluxdown-plugin-manifest.mjs`；官网 `website-v2/src/content/docs/{en,zh}/plugins/{manifest,permissions}.md` |
| 插件视图协议（`native/protocol/src/plugin_view/`：`normalize_view` / `diff_view` / `apply_view_ops`；节点表 `plugin_view_schema.json`、色调 `plugin_view_tones.json`） | Web `web/src/lib/rpc/protocol/pluginView.ts::applyViewOps` 与 Rust 共用 `native/protocol/tests/fixtures/plugin_view_apply.json`；渲染器 GPUI `crates/plugin_ui/src/portable/render.rs` ↔ Web `web/src/pages/plugins/view/nodes/*`（色调表两端测试对照同一 JSON）；SDK `bun run gen` 重新生成 `src/types/view.d.ts`（CI `gen:check` 守漂移）；宿主 JS API（`quickjs_host.rs::HOST_PRELUDE`）↔ SDK `src/types/flux.d.ts`；桌面 `fluxdown` 模块 ↔ `cargo run -p fluxdown_ui_plugin_ui --example gen_plugin_typings` → `packages/plugin-sdk/types/desktop.d.ts` |
| 插件贡献点与授权判定（`crates/plugin_ui/src/contrib.rs::Contributions::from_plugins`、`crates/extensions/src/market.rs::{risk_of, consent_matches, consent_flow}`） | Web `web/src/pages/plugins/contributions.ts`（同纳入条件、活动栏 id `plugin:<identity>:<view>` 同 GPUI 并进 `chromeCatalog.ts`）+ `web/src/pages/settings/sections/extensions/logic.ts::{riskOf, consentMatches, postInstallConsent}`，Rust 与 TS 共用 `crates/extensions/tests/fixtures/consent_cases.json`；引擎 `PluginManifest::risk()` 是同一规则的服务端事实源 |
| 插件嗅探规则层（`native/engine/src/plugin/sniff_pack.rs`：id 改写、sequence 只增、`epoch` 随序号持久化并在数据库重置后改变、聚合体积 ≤ 512 KiB、规则文件按 mtime 缓存；agent `nmh.rs` 的 `sniff_rules.{plugin,pluginSequence,pluginEpoch,deferred}` / `capabilities.{pluginRulesSequence,pluginRulesEpoch}`，`have.pluginEpoch` 与 daemon 不同则无视 `have.plugin`，应答超 900 KiB 时后面的文档置 null 并列入 `deferred`；`PluginSniffRulesDto` 同步 `web/src/lib/rpc/protocol/plugin.ts`） | 扩展 `fluxDown/rules/{schema,validate}.ts`（`plugin` 层：`TIER_RANK` local < official < plugin < community < builtin、community 全部限制 + 禁 request headers）与 `background/{host,rule-sync,rules-store}.ts`（同 epoch 内低序号视为回滚拒收、epoch 变化整体替换 / 清空 plugin 层、`deferred` 补取；`ruleSyncEnabled` 关闭时只发 `pluginOnly` 请求） |
| 扩展规则校验 / 求值（`fluxDown/rules/{schema,validate,engine,glob,url}.ts` 与 `builtin.json`；社区规则产出地址 / mirror 站点范围约束） | 规则仓 `zerx-lab/fluxdown-sniff-rules` 的 `vendor/fluxdown-sniff-rules.mjs`：`bun scripts/sniff-rules/sync-repo.ts vendor --repo <规则仓>` 重新打包并提交到规则仓（`--check` 校验漂移），规则仓 CI、登记仓库收录与作者自检（`author-check` 可复用工作流）才与扩展判定一致。站点专属规则只进规则仓 `rules/official/`；官方包发布后 `sync-repo.ts seed` 刷新扩展快照 `fluxDown/rules/official.json`（勿手改）。社区 id 命名空间（`community.<ns>.*`，扩展「提交到社区」固定 `community.misc.*`）↔ 规则仓 `scripts/{lib,submit}.mjs` |

---

## 6. git · 分支 · 发布

- **git 写操作的门槛是「用户授权」**：用户在本会话要求过（含 `/commit`、「提交」「推一下」「发版」）→ 视为已授权，核对前置条件后**直接做完**，不再征询；用户没要求、你自己想顺手做 → 停手先问。授权按动作粒度计（提交 ≠ 推送，打 tag ≠ 发布）。
- **分支模型**：`main` = 开发分支（超集 / 最新），`stable` = 稳定分支（子集）。日常一律在 `main`；`stable` 只经合并/cherry-pick `main` 前进；hotfix 直进 `stable` 必须**同回合**同步回 `main`。一致性判据 `git log stable --not main` **恒为空**。
- **tag**：稳定 `vX.Y.Z` 只从 `stable`，预览 `vX.Y.Z-rc.N` 只从 `main`；CI 有分支守卫，打错分支整条流水线失败。推送 `v*` tag **立即触发全平台发布，不可逆**。
- 主干门禁由 `.github/workflows/ci.yml` 承载：main push / pull_request 按变更触发 Rust fmt、排除冻结 server 的 workspace clippy、按 crate 分组 nextest，以及 Web SPA / 官网 / 扩展构建；不等发布 tag 才验证。
- 发布流水线是**组件变更检测 + 统一 release** 式（`changes` job 映射路径→`app`/`extension`/`server`/`mobile`/`cli`，组件互不阻断，失败组件经 `workflow_dispatch` 补发）；发版前可手动勾选 `rehearsal` 以尚不存在的 tag 演练整条流水线（全量构建 / 签名 / 公证 / 凭据探活，不建 release、不上传、不推镜像与商店）。矩阵、补发与演练细节见 `.omp/knowledge/ops.md`。

---

## 7. 代码风格与强制规则

**Rust**
- Edition 2024；Clippy **deny**：`unwrap_used`/`expect_used`/`wildcard_imports`。非测试代码禁 `.unwrap()`/`.expect()`，用 `?` + `thiserror`；根 `clippy.toml` 允许测试 unwrap/expect，其余 lint 同样执行 `--all-targets` 门禁。禁 `use foo::*`。Rust 默认安全，只有经审查、无**语义等价**安全替代的平台 FFI 及其必要 ABI/借用/输出读取边界可例外（`rule://no-unsafe-in-rust`）；必须最小 unsafe + SAFETY + 健全类型/生命周期 + RAII/线程约束。workspace deny `unsafe_op_in_unsafe_fn` 与 `clippy::undocumented_unsafe_blocks`，禁止消音绕过。
- 禁 `todo!`/`unimplemented!` 与静默忽略错误（`let _ = <Result/Future>`、`.ok();`、裸 `fallible();`）：根 `[workspace.lints]` deny `clippy::{todo,unimplemented,let_underscore_must_use,let_underscore_future,unused_result_ok}` + rustc `unused_must_use`，所有活动 crate 经 `[lints] workspace = true` 继承（冻结的 `native/server` 除外）。禁止忽错 `allow`、`drop(Result)` 或空错误分支绕过；关键操作必须传错或停止，正常生命周期退出需明确处理，细则见 `rule://no-ignored-errors-in-rust`。
- snake_case 函数/变量，PascalCase 类型，SCREAMING_SNAKE_CASE 常量；公开 API `///` + doctest。
- 异步优先，同步阻塞走 `spawn_blocking`；重试指数退避（MAX=3，base=2s）；task panic 用 `AssertUnwindSafe` + `catch_unwind`。
- 日志宏：`use crate::logger::log_info; log_info!("[mod] ...")`（Rust 2024 无 `#[macro_use]`，每文件显式 use），格式 `HH:MM:SS.mmm [Tag] message`。

**原生移动端**
- Android 使用 Compose Foundation / Flux Lumen，iOS 使用 SwiftUI / Liquid Glass；分层、平台约束与命令见 `.omp/knowledge/clients.md`。
- 共享状态与 wire 由 `native/mobile` / protocol 承担；Kotlin / Swift 不复制游标、重连或云状态机。文案使用共享 i18n，禁硬编码用户文案（快捷键/单位/品牌名/语言自称除外）。

**通用门槛**
- **禁止新增 dependency**，需要时先说明理由等确认；**禁止手编 `Cargo.toml` 版本号**，用 cargo 命令。
- 改动前 `cargo check -p <crate> --lib`；提交前 `cargo fmt --check && cargo clippy --workspace --exclude fluxdown_server --all-targets -- -D warnings`；测试用 `cargo nextest run -p <crate> <filter>`，**测试禁 `--workspace`**。
- 优先复用已有 trait/error 类型，不平行造轮子。单文件 >600 行考虑拆分，单函数 >80 行需说明。
- 查文档优先级：`cargo path <crate>` 本地源码 > docs.rs > web 搜索。
- **命中以下任一项前先读 `rust-router` skill**：新增/改 public API/trait/error 类型、unsafe/FFI/性能关键路径、新增 crate/调 workspace、写 doc comment。仅改名/格式/加日志可跳过。
