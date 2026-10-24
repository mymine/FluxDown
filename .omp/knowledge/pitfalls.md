# FluxDown 踩坑清单

> **用途**：这里记的是「踩过才知道」的约束。**改某个领域前先读对应小节；新踩的坑，修复时同回合补一条（带坐标）。**
> 收录标准：必须有代码坐标、回归测试或修复提交为证，无证据的推测不写。
> 与 `AGENTS.md` 的分工：AGENTS.md §4 是硬不变式的**权威源**，本文件不复制其条目，只在各节首交叉引用；
> 本文件也不维护会漂移的枚举清单（见 `rule://agent-docs-maintenance` 维护约定）。
> 条目格式：`- **约束一句话**：为什么（触发现象） → 坐标 path::symbol / 回归测试`。路径以仓库根为基准。

---

## 引擎：下载与分段

另见 AGENTS.md §4：anyhow 错误一律 `{e:#}`、pg 字节列必须 `BIGINT`、一个 data_dir 只有一个引擎写入者、RSS 无人值守链路、宿主下载走统一出口。

- **换链接后的续传必须对新链接真实探测，并零容差比对新旧文件大小**：沿用库存大小当 hint 会跳过探测——任务被换成更小文件时分段越界、持续 416；新版本仅大几百 KB 时被当作 CDN 误差拼成坏文件。大小一致（如签名过期换新链接）才续传，否则清空分段与旧临时文件重下 → `native/engine/src/db.rs::task_source_changed`、`native/engine/src/download_manager.rs`（`source_changed` 分支）、回归 `native/engine/tests/engine/change_url_resume.rs::switching_to_a_different_file_restarts_from_zero` / `switching_to_a_resigned_link_of_the_same_file_keeps_progress`。
- **纯 Range 续传也必须校验服务器版本标识**：不发 If-Range 时若不验 validator，会把旧任务进度与新文件分段静默拼接 → 回归 `native/engine/tests/engine/realtest.rs::validated_plain_range_rejects_changed_version`（`#[ignore]`，需 `--ignored`）。
- **多段下载取消退出前要把已落库的分段进度单调回写任务级进度**：多段运行期只周期写任务级 `downloaded_bytes`，开头几秒内暂停会显示 0% 或旧值。回写受 `segments_epoch` 守卫，防止与「重新下载」删段归零交错复活旧进度 → `native/engine/src/db.rs::sync_task_progress_from_segments`、回归 `db.rs::task_progress_syncs_from_segments_monotonically`。
- **段内落库偏移只能记已被 fdatasync 覆盖的字节（`durable_offset` 不变式「DB 偏移 ≤ 已持久化字节」）**：周期落库在读循环里同步等 `sync_data` 会让所有连接每 3s 停读一次（macOS 上是 `F_FULLFSYNC`，256MB 脏页实测 242ms），所以改为后台 fdatasync + 快照覆盖提交；段完成路径则同步等覆盖 → `native/engine/src/segment_coordinator.rs::FileSyncGate`（`kick_background` / `sync_covering`）、回归 `sync_covering_returns_start_ge_snap_t` 等。
- **段截断（收到字节少于请求区间）必须返回 Err 走单段指数退避重试，不能当成功**：否则留下内容空洞，只被末尾聚合检查捕获，迫使整任务重启 → `native/engine/src/segment_coordinator.rs::do_segment`。
- **`rebuild_seg_states` 必须保留 worker 维护的 `downloaded_bytes`**：用 coordinator 内存映射里可能陈旧的值覆盖，会让活跃段进度倒退、向 `try_split_largest` 喂过期 split point → 回归 `native/engine/src/segment_coordinator.rs::rebuild_preserves_worker_downloaded_bytes`。
- **rename 前的 `sync_all` 必须在可写句柄上做，且重做只在多段路径**：Windows 的 `FlushFileBuffers` 要求写权限，只读句柄返回 `ERROR_ACCESS_DENIED`，下载会在 100% 后、rename 前失败；单流路径已 flush + `sync_data` 同一写句柄，再 reopen 只是重复整盘刷写 → `native/engine/src/downloader.rs::run_download_inner`（「F034」注释段）。
- **凭据刷新上限只计「连续无效换发」，不计累计次数**：多连接共用每请求一次性令牌时，「每次运行最多 5 次」很快耗尽，任务以 403 失败。各请求点成功后 `DownloadSpec::confirm` 记录身份可用；插件看到的 `refreshCount` 仍是累计值 → `native/engine/src/intercept/refresh.rs::MAX_UNCONFIRMED_REFRESHES`、`native/engine/src/downloader.rs`（`ledger.unconfirmed`）。
- **带子目录的文件名必须在落库前并入 `save_dir`**：下载器只把 `file_name` 当单个组件清洗（防穿越），否则子目录被压平成 `子目录_文件名`（#716）；BT 名字另有语义不拆 → `native/engine/src/download_manager.rs`（`is_bt` / `has_subdir` 分支）。
- **HLS 小响应拉取不得在共享 client 上设全局 timeout，播放列表首行带 BOM 要先剥**：全局 timeout 会误伤普通 HTTP 长下载；`m3u8_rs` 要求首行就是 `#EXTM3U` → `native/engine/src/hls_downloader.rs::fetch_small` / `trim_playlist_prefix`。
- **订阅源正文无有效 tracker 必须算该源失败**：HTML 错误页或空页若当「成功 + 空列表」，会覆盖宿主缓存并在刷新间隔内丢光订阅 tracker → `native/engine/src/tracker_subscription.rs::parse_source_body`。
- **浏览器 POST 接管的任务恢复 / 重试不得改发 GET**：请求体（OneDrive 打包下载含一次性 `access_token`）不落库，旧逻辑恢复时按 GET 发 HEAD + Range 探测，只收 POST 的端点恒 405（#970），表单端点则下回错误内容。建任务时只把非 GET 方法记入 `tasks.request_method`，恢复体首据此以 `POST_ONLY_RESUME_MESSAGE` 失败；以 GET 撞上 `405 + Allow` 只含 POST 时探测标 `405 post-only`、立即停止并报 `POST_ONLY_ENDPOINT_MESSAGE`。两条文案由 GPUI / Web / Android / iOS 按全文匹配本地化（`taskErrorPostOnly*`），改文案须同步各端镜像 → `native/engine/src/downloader.rs`（`allows_only_post` / `probe_post_only`）、`native/engine/src/download_manager.rs::fail_post_only_resume`、回归 `native/engine/tests/engine/post_only.rs`（`#[ignore]`，需 `--ignored`）。
- **传输失败不得靠错误文本关键词判定**：reqwest 0.12 的 `Display` 只有「error sending request for url (…)」，根因全在 `source()` 链；探测阶段旧代码 `format!("network-error: {}", e)` 丢链后，直连被拒 / 黑洞超时的任务既不触发 Auto 换路也不进通用自动重试，黑洞要等约 63s 才失败；中文 Windows 的 OS 错误描述还是本地化文本。一律经 `net_failure::classify`（`io::ErrorKind` / reqwest 标志 / 库固定前缀）分类并以 `[net:<code>]` 标签写进文案，消费端只解析标签 → `native/engine/src/net_failure.rs`、`downloader.rs::network_desc`、回归 `downloader.rs::refused_probe_error_is_classified_for_failover_and_retry`、`auto_probe.rs::refused_direct_is_taken_over_by_proxy_without_waiting`。

## BT / eD2K

另见 AGENTS.md §4：BT 判定只认 `magnet:` 与 `torrent-file://`、「复制链接」读 `origin_url`、`dht.json` 撞端口排除区间。

- **种子内相对路径必须过 `torrent_relative_path_is_safe`，且 Windows 规则只对 Windows 强加**：Windows 下组件内的 `:`（盘符前缀 / ADS）、控制字符、设备保留名会改写基路径或落到设备；这些在其他平台是合法文件名，不得误杀 → `native/engine/src/bt_downloader.rs::torrent_relative_path_is_safe`（含 `a:b` 非 Windows 放行断言）。
- **完成后的整目录 rename 在 Windows 上会因做种句柄存活而 ACCESS_DENIED，必须保留逐文件降级**：librqbit 对种子内全部文件持句柄直到任务删除，`pause()` 只转移不关闭（BUG-BT-DIR-RENAME-2X）；文件级 rename 对 FULL-share 句柄免疫且仍是同卷零拷贝 → `native/engine/src/bt_downloader.rs::move_path_with_file_replace` / `move_dir_recursive`（回归 `move_dir_recursive_continues_after_child_error` 等）。
- **完成布局的批内重名判定用小写折叠名**：种子内仅大小写不同的 basename 在 Windows 上是同一 dst，精确比较会漏判并互相覆盖 → `native/engine/src/bt_downloader.rs::compute_completion_layout`（`taken`）。
- **Windows 上 BT 新增选项必须经 `build_add_torrent_options` 注入 sparse 工厂**：新建任务不应物理预留全部空间 → `native/engine/src/bt_downloader.rs::build_add_torrent_options`、回归 `native/engine/tests/engine/bt_sparse_add.rs::fresh_add_creates_sparse_files_without_physical_reservation`（`cfg(all(windows, feature = "bt"))`）。
- **eD2K 根哈希的输入序列只能经 `build_root_input` 构造**：块数恰为 part_size 整数倍时的 off-by-one（尾部空块）集中在这一个转换点处理 → `native/engine/src/ed2k/hash.rs::build_root_input`、回归 `build_root_input_phantom_tail_appends_empty`。
- **Kad 收包循环遇 Windows `WSAECONNRESET(10054)` 必须忽略并继续等到 deadline**：向已死节点发包触发的 ICMP 端口不可达，会被 Windows 报在**下一次** `recv_from` 上；据此返回 `None` 会让一个死节点终止整个接收窗口、漏掉其后存活节点的回包 → `native/engine/src/ed2k/kad/node.rs::recv_within`。

## 插件运行时

另见 AGENTS.md §4：rquickjs 分叉补丁与 gpui-kit 同 rev、持有引擎的 actor 必须 drain `resolve_rx` / `plugin_retry_rx`、插件页面脚本只经 `userScripts`。

- **QuickJS 抛 `null` 要按异常值识别为 OOM**：内存紧贴上限时 QuickJS 连 `"out of memory"` 错误对象都分配不出，改抛 `null`，字符串匹配的 `reclassify_oom` 认不出，该次 OOM 不计入熔断；必须在仍持有异常值时判定（代价：插件主动 `throw null` 也计 OOM） → `native/engine/src/plugin/quickjs.rs::caught_error`、回归 `null_exception_sentinel_counts_as_memory_limit_exceeded`。
- **外部工具子进程要补全登录 shell 的 PATH**：macOS 从 Dock / 访达启动只继承 `/usr/bin:/bin:/usr/sbin:/sbin`，yt-dlp 找不到 node / bun，带 Cookie 解析 YouTube 报 "The page needs to be reloaded"；取 PATH 的等待不计入插件脚本墙钟 → `native/engine/src/plugin/tool_path.rs::apply_full_path_env`（`read_login_shell_path` / `merge_path_lists`）。
- **插件安装期校验必须覆盖拦截器入口脚本**：只校验主入口会让拦截器语法错误的插件装上后运行期才炸 → `native/engine/src/plugin/manager.rs`、回归 `native/engine/tests/engine/plugin_interceptors.rs::broken_interceptor_script_is_rejected_at_install`。
- **resolve 占位期间的重复恢复必须直接忽略**：此时 DB 仍是 error(4)/paused，并非终态残留；不拦会移除占位并再起一个并发 resolve → `native/engine/src/download_manager.rs::has_pending_resolve`（`resume_task_inner` 的判定）。
- **resolver 的 `audioUrl: ""` 是「无音轨」，不能当轨对**：`Some("")` 曾让任务走 DASH 下载器、把直链当 MPD 拉取解析而失败（YouTube「仅音频」档、变体切到无音轨档都会命中）；空串在落入 `QueuedTask` 时就要归一成 `None` → `native/engine/src/download_manager.rs::apply_resolve_to_queued`、回归 `native/engine/tests/engine/plugin_lazy_resolve.rs::explicit_empty_audio_url_downloads_single_file`。
- **`ephemeral` 结果的 `totalBytes` 只能给准确值**：引擎跳过探测并按它切 Range，估算值（码率 × 时长）偏大时尾段 416 反复重试直到任务失败；拿不到准确长度就给 0 / 不给 → `packages/plugin-sdk/examples/youtube-down/src/formats.js::toVariant`（YouTube 音画合一的 itag 18 没有 `contentLength`）。
- **在 QuickJS 里跑 yt-dlp-ejs 要放宽 resolver 堆上限**：冷解析约 3 MB 的播放器 JS（meriyah AST）峰值 160–180 MiB，默认 64 MiB 必 OOM；预处理产物缓存进 `flux.fs` 后热路径约 50–64 MiB → manifest `resolvers[0].memoryLimitMb`（`native/engine/src/plugin/manager.rs::resolve_budget_for`、`RESOLVE_MEMORY_CEILING`），示例 `packages/plugin-sdk/examples/youtube-down/src/player.js`。

## daemon / agent 本机服务

另见 AGENTS.md §4：本机端口默认宽松显式严格（WSAEACCES 回退）、远程控制凭据与会话撤销、空闲静默、遥测只有两条匿名事件。

- **回环 daemon 的 HTTP 客户端必须 `.no_proxy()`**：否则会话凭据与文件字节会经环境 / 系统代理离开本机 → `native/agent/src/capture.rs::DaemonBlobClient::new`。
- **客户端 WebSocket 的帧上限与消息上限必须一起放宽到 `MAX_INBOUND_RPC_MESSAGE_BYTES`**：tungstenite 默认 16 MiB；snapshot 与整表替换事件只带头部、任务表经 `system.taskPage` 分页，新增大应答不要回到「单帧带全量」。桌面、agent、移动端远端三处连接都必须应用同一常量 → `native/protocol/src/rpc.rs::MAX_INBOUND_RPC_MESSAGE_BYTES`、`crates/app/src/agent_client.rs`、`native/agent/src/daemon_client.rs`、`native/mobile/src/remote.rs`。
- **server 模式 Origin 同源校验：Host 不带端口时只比主机，带端口时必须等于 Origin 的有效端口，非 http(s) Origin 一律拒绝**：nginx `Host $host` 与 Nginx Proxy Manager 会丢掉对外非默认端口，严格比端口会把反代部署整体挡掉 → `native/agent/src/server_mode.rs::origin_allowed`、回归 `origin_must_match_host_when_present`。
- **跨进程文件锁争用要识别 Windows 的 `ERROR_LOCK_VIOLATION`**：std 不把它映射为 `WouldBlock`，只看 `kind()` 会把「已有写入者」误当 IO 故障 → `native/engine/src/db.rs::acquire_engine_file_lease`、`native/agent/src/state.rs::is_lock_contended`。
- **系统托盘：macOS 不挂常驻菜单、不设 tooltip，Linux 托盘 id 固定**：`NSStatusItem` 一旦挂菜单，任何点击都直接弹菜单，左键「显示窗口」失效，故右键时经事件循环投递 `ShowMenu` 由主线程临时挂菜单弹出；macOS 27 的状态栏 tooltip 收不到移出事件会常驻（#953），倒计时改显示在菜单项；Linux SNI Id 不能随 PID 变（GNOME 顶栏扩展靠它记位置），图标临时文件按进程分目录，避免并存实例退出时删掉对方仍引用的图标 → `native/agent/src/shell/host.rs`（`HostEvent::ShowMenu` / `pop_up_menu` / `TRAY_ID` / `linux::IconDir::for_process`）、`native/agent/src/shell/tray_model.rs::COUNTDOWN_IN_MENU`。

## GPUI（gpui-fast 保留模式）

另见 AGENTS.md §4：render 读的状态必须在 entity / global / `ListState` / `ScrollHandle` 内，父 view notify 不再重建子 view，渲染期不得无条件写 entity / global。

- **渲染期要「先读后写」，值没变就不 `update`**：gpui-fast 把渲染期的 entity 写入记为外部修改，每帧写会让读该状态的其他 view（如 shell 顶栏）每帧重建 → `crates/components/src/sidebar/mod.rs`（`sample` 后的只读分支）、`crates/downloads/src/components/sidebar_layout.rs`。
- **事件回调里写表格状态前先读，没变化就返回**：没有高亮行时每次左键都写表格，会让 gpui-fast 把表格记为已变化 → `crates/downloads/src/components/task_table.rs`（`right_clicked_row().is_none()` 早退）。
- **鼠标移动 / 滚轮路径不得写表格 entity**：每次写都会让随后的滚动 notify 变成整页重建；行序「保持期」只在刷新 / 重排定时器（非渲染路径）里读取 → `crates/downloads/src/model/row_order.rs`。
- **悬停 / 按下 / 动画的状态层必须是挂在元素状态里的小视图，且不被外层视图观察**：`Window::use_keyed_state` 会让外层视图观察状态实体，动画帧就会重建整页 → `crates/components/src/motion.rs`（模块头注释）。
- **不可缩放窗口不能用 gpui-component 的 `TitleBar` 自带窗口控件**：其 `WindowControls` 私有且在 Windows / Linux 总绘制最大化按钮，`is_resizable = false` 时最大化区域退化为拖动区、点了无效，因此自绘只含最小化 + 关闭的标题栏（Windows 靠 `WindowControlArea` 交原生非客户区命中测试）→ `crates/shell/src/window_controls.rs`。
- **Windows 上最小化的窗口边界报告为 `Windowed`，最大化状态要与平台标志取或**：只看 bounds 会把「最小化的最大化窗口」存成普通窗口 → `crates/app/src/windows/bounds.rs::resolve_maximized`、回归 `minimized_maximized_window_keeps_maximized`。
- **进度 / 完成窗口里「打开文件 / 文件夹」不等打开命令返回**：Windows 上它要等系统打开调用返回，可能比对方出现在前台还晚，窗口该关而不关；会关窗时点击即请宿主等对方接管前台 → `crates/downloads/src/pages/progress_window.rs::hand_off`。

## Web SPA

- **新建下载等处生成 UUID 不得用 `crypto.randomUUID()`，一律 `randomUuid()`**：浏览器只在安全上下文（HTTPS / localhost）暴露它，NAS / Docker 经 `http://<局域网 IP>:17800` 访问时缺失，调用即抛错；`getRandomValues` 无此限制 → `web/src/lib/uuid.ts::randomUuid`（使用方 `web/src/pages/downloads/dialogs/importRunner.ts`）。

## 浏览器扩展

另见 AGENTS.md §4：插件页面脚本只经浏览器 `userScripts` API 执行。

- **共享 NMH 端口是一问一答串行的，队首卡死必须断开换新端口，但「排在合法慢请求后面而超时」不得回收**：中继与 agent 连接串行，队首卡住后排队请求全部超时，原先超时不换端口会让任务面板与能力探测一直「未连接」直到 SW 重启；旧端口迟到的断开事件也不得波及新端口 → `fluxDown/utils/nmh-shared.ts::createSharedChannel`、回归 `fluxDown/utils/nmh-shared.test.ts`（三条用例：队首卡死回收 / 慢请求之后不回收 / 迟到断开不波及新端口）。
- **扩展必须跳过自己配置的远端 FluxDown 服务发起的「下载到本地」请求**：否则扩展拦截该下载、再回投同一远端，形成持续创建任务的死循环；判定依据含 referrer / URL 的 origin 与 server 文件导出端点路径（覆盖反代 / 不同主机名访问 Web UI） → `fluxDown/utils/settings.ts::isFluxDownServerDownload`、测试 `fluxDown/utils/settings.test.ts`。

## Android

- **前台服务停止必须用 `stopSelfResult(lastStartId)`，并在每次启动请求重新计时空闲宽限**：用 `stopSelf()` 会拆掉在途、尚未来得及 `startForeground` 的启动请求，系统以 `ForegroundServiceDidNotStartInTimeException` 杀整个进程；外部唤起建任务后也不能被旧计时停掉 → `mobile/Android/app/src/main/java/com/fluxdown/app/service/DownloadService.kt::stopIfIdle`。
- **FileProvider 要声明 `root-path` 覆盖 `/storage/`**：可移动存储卡上的文件（`/storage/6BF9-89B3/...`）否则生成不了 content URI，打开已下载文件会崩；文件不在声明根内时提示失败而不是抛异常 → `mobile/Android/app/src/main/res/xml/file_paths.xml`（`storage_volumes`）。
- **叠层返回键的优先级：菜单 / 对话框在 Sheet 之上**：后注册的 Sheet 返回处理器会抢先，菜单打开时返回键应交给 `FluxOverlayHost` → `mobile/Android/fluxui/src/main/java/com/fluxdown/fluxui/overlay/FluxSheet.kt`（`canGoBack` 条件）。
- **应用自更新不用 `PackageInstaller` 会话，交给「打开 APK」的默认安装程序，且只在界面可见时拉起**：会话的确认界面固定由系统安装器处理，无视用户设定的默认安装程序（小米机上总是小米安装器）；部分国行 MIUI 安装器对第三方会话直接回 `INSTALL_FAILED_ABORTED`（用户什么都没点，旧映射显示「已取消安装」）；从 `BroadcastReceiver` 拉确认界面在后台被系统静默拦截，状态永远停在「正在安装」。未持有 `UPDATE_PACKAGES_WITHOUT_USER_ACTION` 时会话本来也要确认。安装包经 FileProvider `app_update` 根（`../no_backup/updates/`，不可信应用不能硬链接）只读授权 → `mobile/Android/app/src/main/java/com/fluxdown/app/update/ApkInstaller.kt`、`UpdateEffects.kt`（交接 `LaunchedEffect`）、`res/xml/file_paths.xml`。

## iOS

另见 AGENTS.md §4：iOS 构建依赖 `third_party/librqbit-dualstack-sockets`、商店 flavor 以 `--no-default-features` 剔除 BT / eD2K。

- **iOS 上 Rust mDNS 不可用，局域网发现走 `NWBrowser`，且列表项要先解析成 IPv4 `host:port`**：本机主机（进程内）发现走 `PairingModel.browser`，远端主机才用它的 agent 发现 → `mobile/FluxDown/FluxDown/Features/Devices/DevicesScreen.swift::presentAddDevice`、`mobile/FluxDown/FluxDown/Features/Devices/LanBrowser.swift`。
- **列表行的滑动删除不得用 `role: .destructive`，用红色 tint**：`List` 会立即动画移除该行而数据源仍有这一行，`UICollectionView` 行数不一致崩溃 → `mobile/FluxDown/FluxDown/Features/Devices/RemoteTaskRow.swift`（`swipeActions` 注释；本地任务行同）。
- **`.searchable` 的 `placement` 必须恒定**：动态切换会在输入中丢焦点 → `mobile/FluxDown/FluxDown/Features/Downloads/DownloadsListScreen.swift`。
- **本机主机是进程级单例，多个会话共享**：测试里两个场景必须串行（`@Suite(.serialized)`）；仅用于发 RPC 的会话要把 `signals` 流读干净，否则快照 / 事件在会话存活期间无限堆积 → `mobile/FluxDown/FluxKit/Tests/FluxBridgeTests/LocalHostSmokeTests.swift`、`mobile/FluxDown/FluxDown/Features/Devices/HostPairingFlow.swift`。

## 构建与发布

另见 AGENTS.md §4：`profile.release` 不设 `panic="abort"`、headless Web UI 编译期内嵌、iOS 依赖 dualstack-sockets 补丁。

- **macOS 签名 / 公证 / 装订、TestFlight 上传对 Apple 侧瞬时错误必须退避重试，且公证拆为提交 / 等待 / 查询**：断网后只续等同一次公证提交，结论以 `notarytool info` 状态为准；TestFlight 只对瞬时错误（如 -19241 "A server error occurred"）重试，其余立即失败，避免掩盖真实签名错误 → `scripts/package_gpui_macos.sh::retry` / `stapler_retry`、`mobile/FluxDown/scripts/testflight.sh`（瞬时错误重试段）。
- **Windows 安装器判定旧版已移除，看 UI 入口文件是否还在，不依赖 HKLM 卸载键**：对旧版全用户安装的卸载等待因此加固（提交 322a3901）；等待上限 60 秒，卸载器起不来要写日志 → `installer/windows/setup.iss`（`LegacyUninstallTimeoutMs`、「legacy copy counts as removed」注释段）。

## Windows 专属

另见 AGENTS.md §4：每个 console 子进程 spawn 必须包 `proc::no_console_window`、本机端口 WSAEACCES 回退、BT `dht.json` 撞端口排除区间。其余按领域散落在上面各节（`sync_all` 写句柄、`ERROR_LOCK_VIOLATION`、WSAECONNRESET、BT 目录 rename、最大化判定），此处只收仅在 Windows 触发且不属于上述领域的条目。

- **Windows toast 线程不得逐条 `RoInitialize` / `RoUninitialize`**：headless `--server` 无其它 apartment，拆掉最后一个会卸载 WinRT 服务端，进程级缓存的激活工厂悬空，下一条通知 `0xC0000005` 崩溃。用从未初始化过 COM 的短命线程，让 windows-core 经 `CoIncrementMTAUsage` 常驻进程 MTA → `native/agent/src/notification/windows.rs`（`runtime::show` 调用处注释）、`native/agent/src/sound/backend.rs`（同模式）。
- **写 `Zone.Identifier` 会刷新 NTFS 主文件修改时间**：开启「使用服务器文件时间」时必须写标记后回写原时间；设置时间只申请 `FILE_WRITE_ATTRIBUTES` 避免共享冲突；后台 fdatasync 要先释放复制句柄再通知（最后一个句柄关闭时 NTFS 会把修改时间刷成当前时间，晚于 `apply_server_mtime` 会覆盖服务器时间）→ `native/engine/src/mark_of_the_web.rs::spawn_mark_downloaded`（`keep_mtime`）、回归 `zone_identifier_keeps_mtime_only_when_requested`，`native/engine/src/segment_coordinator.rs::FileSyncGate::kick_background`。
- **暂停 / 删除 / 重新下载清盘要给「写入方还没松手」留重试窗口**：`pause_task_silent` 只取消 token 不等 spawned task 退出，Windows 上被写入方持有的文件 `remove_file` 直接失败；残留的 `.fdownloading` 会被下一轮当成可续传数据，「从零重下」的承诺就破了 → `native/engine/src/download_manager.rs::remove_file_retrying`。
- **测试删目录前必须先关连接池**：Windows 上 sqlite 文件句柄未释放时 `remove_dir_all` 静默失败，残留目录曾在 temp 累积 1400+ 个，并导致陈旧库被继承的历史 flaky → `native/engine/src/db.rs::close_test_db`（测试辅助）。
- **自更新替换文件要把杀毒 / 搜索索引的短暂占用当瞬时错误重试**（拒绝访问、共享冲突、锁冲突、映射节被占用）→ `native/agent/src/update/install/replace.rs::is_transient`。
- **局域网对端下发的保存目录（接单目录）必须拒绝 UNC / 设备命名空间路径与 `..` 段**：Windows 上对 `\\host\share`、`\\?\`、`\\.\` 创建目录会主动触发对外 SMB 认证 → `native/agent/src/link/service.rs::is_plain_local_dir`（由 `resolve_receive_dir` 调用，云端接单与局域网互联共用）。
- **原生消息主机清单在 Windows 经 `%ComSpec% /d /c` 启动**：`ComSpec` 配置值不可用时浏览器起不来 NMH，诊断要能检出并修复 → `native/agent/src/diagnostics.rs`（`ComSpec` 段）。
