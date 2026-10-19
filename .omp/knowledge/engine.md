# FluxDown internals · 数据模型 · 下载引擎 · 插件系统

> 本文件是 `FluxDown/AGENTS.md` 的深挖附录：只放**枚举性 / 可从源码复原**的细节，硬不变式与红线在 AGENTS.md。
> 路径以 `FluxDown/` 为根（cwd=工作区根时前置 `FluxDown/`）。事实层以源码为准，文档给坐标。

---

## 状态与数据模型

- **任务状态码**: 0=pending, 1=downloading, 2=paused, 3=completed, 4=error, 5=preparing。
- **文件类型分类**: all/video/audio/document/image/program/archive/other；分类规范由 protocol DTO 与当前客户端分类模型承载，用户可自定义匹配模式与保存目录。
- **时间分组**: today/yesterday/thisWeek/thisMonth/older。
- **任务组**: 多文件下载的纯逻辑聚合壳（N 独立子任务 + `task_groups` 行）；组进度由前端 SUM 聚合；空组自动回收（`gc_empty_groups`）。

### 数据库（`native/engine/src/db.rs`，sqlx `Any` 池）

**双后端**：URL scheme 选后端（`sqlite:`/`postgres:`）；两份 DDL 常量（`SQLITE_SCHEMA`/`POSTGRES_SCHEMA`）处理二进制、字节计数与自增主键的后端差异；运行时 SQL 统一 `$N` 占位符；`add_column_if_missing` 幂等迁移（新库建表即全列，旧桌面库经 ALTER 升级）。SQLite 侧 WAL + `synchronous=NORMAL`（进程崩溃不丢已提交事务，断电可能丢最后几笔）+ 外键 + busy_timeout=5000；批量写入仍要合并成单事务。

**当前表（列以 db.rs 为准，此处仅索引）**：
- `tasks`(id PK, url, file_name, save_dir, status, total/downloaded_bytes, segments, created_at, error_message, proxy_url, queue_id, checksum, ignore_tls_errors, bt_selected_files, bt_custom_name, orig_etag, orig_last_modified, audio_url, file_missing, `range_verified`（配额端点续传验证）, queue_order；迁移列：cookies, referrer, extra_headers, resolver_plugin_id, segments_epoch, completed_at, group_id, resolver_item, rss_source_id（RSS 溯源，空=非 RSS 来源）, `origin_url`（展示用真实来源；`.torrent` 任务的 `url` 是 `torrent-file://local` 哨兵，「复制链接」类 UI **一律**读它并空则回退 `url`）, `auto_route`（`ProxyMode::Auto` 的任务级最终链路，wire 标签见 `auto_proxy::route`；空=非 Auto）, `unattended`（无人值守创建标记，`NewTaskSpec::unattended_selection` 置位：RSS / 远程派发 / 系统打开链接或拖入 / 外部接管命中设备本地偏好「免打扰跳过二次选择」`download.silent_skip_selection`；start/resume 读它让 HLS/DASH 画质与插件变体静默取默认、「文件已存在」询问按重命名；BT 不读此列——建任务时已按「全选」写 bt_selected_files））
  - 来源归因迁移列 `src_cdn_bytes` / `src_proxy_bytes` / `src_nic_bytes`（BIGINT）：coordinator 在既有 DB flush 节拍按 `NodePool::source_bytes` 把「多 CDN 钉定节点 / Auto 代理路径 / 多网卡链路」的增量以 `segments_epoch` 守卫累加；源站 = `downloaded_bytes` − 三者之和，P2P 协议由客户端整体归 P2P。任何把 `downloaded_bytes` 复位为 0 的写入同语句清零三列。实时值走 `TaskRuntimeDto.sourceBytes`（含在途），持久值走 `TaskDto.sourceBytes`；客户端详情常规页「来源构成」区块的切片算法 GPUI / Web 逐条镜像（见 AGENTS.md §5）。
  - 命名来源迁移列 `name_inferred`（1 = 引擎推断名，HTTP 完成期可精修）/ `resp_name` / `resp_mime`（实际 GET 响应的命名证据），语义见下方 `naming/`。
  - 「文件已存在」逐任务决定迁移列 `exists_decision`（`''` / `rename` / `overwrite` / `skip`），语义见下方「文件已存在策略」。
  - 多插件冲突待选迁移列 `resolver_choice_pending`（1 = 多个 resolver 同时匹配、首次起跑前还要定案），语义见下方「多插件 resolver 冲突」。
- `task_segments`(复合 PK task_id+segment_index；旧库遗留 id AUTOINCREMENT 不再读)
- `task_groups`(id PK, name, source_url, save_dir, created_at)
- `config`(key PK, value)——**所有设置键**都存这里
- `torrent_files`(task_id PK, file_bytes BLOB)
- `queues`(id PK, name, speed_limit_kbps, upload_limit_kbps, max_concurrent, default_save_dir, position, default_segments, default_user_agent, is_running, schedule_enabled/start/stop, schedule_days 位掩码)
- `ed2k_blocks`(复合 PK task_id+block_index, state, downloaded_bytes, retry_count)
- `ed2k_hashset`(task_id PK, hashes BLOB)
- `task_artifacts`(复合 PK task_id+file_name；追踪 sidecar/产物文件供清理)
- `rss_sources`(id PK, `provider_id`/`provider_config`（订阅 provider 适配器与其不透明配置，旧数据默认 `rss`/空）, url, name, enabled, auto_download, start_paused, queue_id, save_dir, interval_minutes, include/exclude_pattern, use_regex, smart_episode, size_min/max_bytes, send_referer, notify_on_download, max_per_fetch, cookies, user_agent, proxy_url, last_fetch_at, last_success_at, last_error, fail_count, `seeded`（首轮是否已完成）, position)
- `rss_items`(复合 PK source_id+guid, title, link, enclosure_url/length/type（声明的下载目标 MIME）, `resolver_item`（插件二段解析标识，空 = 普通直链）, pub_date, fetched_at, status 0..5, task_id 回链, episode_key, reason 原因码；`fetch_failures` / `retry_after` 持久化种子抓取失败退避；`ON DELETE CASCADE` 于 rss_sources)
- `task_create_batches`(batch_id PK, results JSON, created_at BIGINT)：`daemon.task.createMany` 的幂等记录，与该块任务行**同一事务**写入；同键重放直接返回首次逐条结果；7 天前的记录在后续批量写入事务内顺带清理（无定时任务，符合空闲静默）。

**内置队列**: `main`（主）/`later`（稍后下载），播种于 `Engine::new`，不可删/改名；存量 `queue_id=''` 迁入 `main`。**主队列恒运行**（`model::is_always_running_queue`）：引擎 `stop_queue` / `set_queue_schedule` 对它不生效，daemon `daemon.queue.stop` 与带实际定时的 `daemon.queue.schedule` 返回 InvalidArgument（`field=queueId`；「关闭且无时刻」放行为空操作，兼容旧客户端保存流程）；`Db::seed_builtin_queues` 每次启动把旧版本留下的停止 / 定时主队列收敛为运行中、无定时（只在不符时写一行，暂停任务保持暂停）。各客户端对运行中的主队列不显示「停止」、不显示定时（只给 `mainQueueAlwaysRunningHint`）。删除命名队列后，任务归属、排队位置与持久排序都要同步：`TaskQueueChanged` 只带归属，`QueuePositionsChanged` 只带待排位置，`TasksSnapshot` 才是 `queue_order` 归零后的权威来源。

**任务活动与实时采样**：`task_activity.rs` 拥有源端活动代理，`db.rs` 的活动表/保留标记在 SQLite 与 PostgreSQL 同步维护。状态边沿、完整错误、实际重试、分段拆分及 CDN 语义事件先持久化再广播；高频进度不逐帧入库。ID 在同一数据库内跨重启单调，任务删除级联清理；保留约束及分页上限以 `db.rs` 常量为准，清理过的历史有逐任务截断标记。所有宿主的 `progress_reporter` 必须使用 `Engine::activity_sink`，否则绕过统一记录；关闭时冲刷有超时并报告真实失败。`transfer_activity.rs` 的 RAII 计数只覆盖真实传输生命周期，采样携带源端单调序号；分段几何、活跃传输、配置上限与 BT peers 是不同事实，不能相互推算。

**任务速度历史**（`speed_history.rs`，表 `task_speed_history` 在两套 schema 同步）：`progress_reporter` 对下载态（status=1）任务按墙钟逐秒累计平滑速度（与界面显示同源），横轴只计下载秒数，暂停 / 排队 / 做种不计入；两帧空窗超过 10s 只补 10s（进程挂起 / 休眠）。点数封顶 240，满额相邻两点合并、桶宽翻倍，存储有界且覆盖全程；峰值、累计秒数与传输字节为精确统计，不受合并影响。下载中的历史常驻内存（挂在 `Db` 上，进度报告器与 daemon 查询共享），每 30s 及离开下载态时落库，daemon 停机时 `Db::flush_speed_history` 冲刷——只在下载期间写库，符合空闲静默。「重新下载」（`restart_task`）经 `Db::reset_task_speed_history` 在同一把锁内清内存与库行，迟到的停止帧不会让旧历史复活；换源 / 尺寸变化导致的进度复位不清历史。任务删除级联清理。

**建任务管线**：`create_task` 与 `create_tasks_batch` 共用「内存准备（thunder 解码 / BT 禁用 / 去重 / checksum / 站点凭据 / UA 并入请求头 / 队列默认 / ED2K / 相对文件名）→ 单事务落库（每队列一次 `MAX(queue_order)` 后内存递增、每行一条全列 INSERT、一次 `IN` 回读）→ 发布」三段，单条与批量语义不能分叉。批量一块只读一次 `dedup_same_url` / 活跃 URL / 站点凭据库，块内重复链接解析为块内首个任务；发布顺序 = 先一条 `EngineEvent::TasksChanged`（wire `DaemonEvent::TasksChanged`），再逐条 webhook `task.created`、解析插件缺失报错、稍后下载探测 / 入队。逐条拒绝（`CreateRejection`：BT 禁用 / thunder 解码失败 / checksum 非法 / 请求头非法）不影响同块其他条目；整块事务失败则整块回滚且不留幂等记录。
**任务 HTTP client**：需要专属 client 的任务（单任务代理 / UA、队列默认 UA、忽略 TLS、Auto 走代理）按（代理指纹, 生效 UA, ignore_tls）缓存复用（超过 64 个整体清空）；构建一次约 70ms，禁止每任务重建。同键任务共享连接池与 cookie jar，与全局 client 语义一致。

---

## 下载引擎（`native/engine`）

- 平台存储 FFI（`disk_space.rs`、`segment_coordinator.rs`、`bt_sparse.rs`、`bt_downloader.rs`）保留实际分配/并发扩容、大卷宽度和饱和计算语义，不机械替换成 fs2；具体锁定版本差异见 `rule://no-unsafe-in-rust`。空间出参仅成功后读取，fadvise 直接错误码显式记录；Windows 属性读取走安全元数据，错误不能被当作 sparse 标记，测试区分磁盘占用与逻辑长度。

### 6 种协议（分发 = `download_manager::do_start_task`/`do_resume_task` 内单条 if/else 链，每臂 `catch_unwind`）

| 协议 | 判定谓词 | 入口 | 文件 |
|---|---|---|---|
| **HTTP/HTTPS**（默认兜底） | fallthrough | `segment_coordinator`（IDM worker pool） | `downloader.rs` / `segment_coordinator.rs` / `segment_advisor.rs` |
| **FTP** | `is_ftp_url` | `ftp_downloader::run_ftp_download` | `ftp_downloader.rs`（suppaftp 同步 + spawn_blocking） |
| **BitTorrent** | `is_bt_url`（magnet 或 .torrent 哨兵） | librqbit `SharedBtSession` | `bt_downloader.rs` / `tracker_subscription.rs` |
| **HLS** | `hls_downloader::is_hls_url` | `run_hls_download` | `hls_downloader.rs`（M3U8/多码率/AES-128） |
| **DASH / 音视频轨合并** | `is_dash_url` 或有 `audio_url` | `run_dash_download` | `dash_downloader.rs` |
| **ED2K（仅下载）** | `ed2k::link::is_ed2k_url` | `ed2k::run_ed2k_download` | `ed2k/`（mod,link,proto,hash,server,peer,client,server_subscription,upnp,kad/） |

- **HLS 分片调度与测速**（`hls_downloader.rs`，点播视频轨与独立音轨共用）：分片下载完即归还连接许可，数据进 `ReorderWindow` 乱序窗口（领先写盘 ≤ 并发数 × 8 段、缓冲 ≤ 32MiB，队首段永远放行），单一 writer 按序追加到唯一 `.fdownloading` 临时文件——没有分片临时文件、没有合并拷贝，队首慢不再让其余连接空等（旧版许可随写盘释放，8 段成批等最慢一段）。进度 = 落盘字节；测速与宿主流量 = `SegmentMeter` 的 body 接收字节（`ProgressUpdate::transfer_bytes`），限速作用在读取侧而非写盘侧。续传检查点按 `DB_SAVE_INTERVAL_SECS` 节流，暂停 / 出错 / 收尾前补写最新前缀（检查点只会落后：续传截断到检查点、重下其后几段）。直播录制仍按轮询窗口 `buffered` 并发，共用同一计量与读取侧限速。
- **HLS 独立音轨与封装**：master 变体的 AUDIO 组若有 `EXT-X-MEDIA` 独立 URI，`external_audio_uri` 优先选 `DEFAULT=YES` 音轨。开跑前探测可用 ffmpeg；可用时音轨与视频并行下载，各自保留续传检查点，进度汇总两轨字节，收尾经 `mux_video_audio` + `dash_downloader::ffmpeg_copy_to_mp4` 以 `-c copy`（不转码）封装 MP4。音轨下载的真实错误会取消整任务；ffmpeg 不可用时仅下载视频并写 warning 活动，mux 失败则保留视频、尝试另存音轨 sidecar 并写 warning；mux 取消保留两轨临时文件与检查点供恢复。
- **HLS TS→MP4**：`remux_ts_to_mp4` 优先用同一 ffmpeg 流复制入口，不整文件读入内存，也不受内存转换的 192MiB 上限约束；无 ffmpeg 或执行失败才尝试有上限的 `ts2mp4` 内存转换，空间不足 / 转换失败保留 TS，取消不触发内存兜底。`EXT-X-MAP` 的 fMP4 输出不做 TS 转换，只按占名规则改名为 MP4。
- BT 任务绕过 pending 队列，且**不计入** http/ftp 并发计数（`max_concurrent`）。
- **BT 判定只认 `magnet:` 与 `torrent-file://` 哨兵**（`is_bt_url`）。HTTP 的
  `.torrent` **直链不会走 BT**——会被当普通文件下回来一个种子文件。要让直链
  变成真下载，必须先把字节抓下来再以 `NewTaskSpec::torrent_file_bytes` 建任务
  （RSS 订阅就是这么做的，见 `rss/` 的两段式）。
- **DHT 持久化是纯缓存，`SharedBtSession::new` 对它三级兜底**：带 `dht.json`
  起 → 失败则删掉它重试 → 再失败则关 DHT 起（tracker + PEX 仍可用）。起因是
  一份钉着 `addr: 0.0.0.0:58686` 的 `dht.json` 撞上 Windows 动态端口排除区间
  （`netsh interface ipv4 show excludedportrange protocol=udp`；`netstat` 看不到
  占用但 bind 返回 `WSAEACCES` 10013），导致**所有** BT 任务永久 status=4 而
  用户无从自救。**anyhow 错误一律用 `{e:#}` 打印**——`{e}` 只输出最外层
  context，会把这类根因整个吞掉。
- **BT peer 上限 `BT_PEER_LIMIT` = 512**：librqbit 的 `peer_limit` 信号量同时计
  「连接中」与「已连接」，出站拨号从开始就占许可，许可满时入站连接被直接丢弃。
  上游默认 128 在公网 swarm 里会被不可达 peer 的连接尝试占满（#825 实测约 120/128，
  可用 peer 仅 5–10）；连接中数量由 `connect_rate` × 超时另行约束。下调前先实测。
- **uTP（`bt_enable_utp`，默认开）**：开启时监听模式为 `TcpAndUtp`，出站并发拨 TCP + uTP；
  #825 实测在上限 512 基础上稳态再翻倍，代价是 UDP 收发的系统态 CPU 明显更高（上游仍标为
  experimental）。改动经 daemon 重建共享会话生效。librqbit 来自 `zerx-lab/rqbit` 的 `main`
  （上游 main + MSE + Android HTTPS + 握手超时下发修复），升级上游时先在该 fork 的 main
  合并上游，再改根 `Cargo.toml` 的 rev。
- **BT 会话保活与完成校验**：`maybe_release_bt_session` 在存在「已暂停的未完成
  torrent（句柄在册）」时**不拆**共享会话——resume 走 `unpause`（Paused→Live
  零校验秒恢复）；全部 BT 任务终态化（完成/删除且无做种）才释放。暂停撞上
  librqbit 初检（只能从 Live 暂停）时经世代号「延迟暂停」兜底，防幽灵下载。
  Windows 上 staging 文件经 `bt_sparse` 打 FSCTL_SET_SPARSE（免整体簇预留 +
  免 VDL 零填充写放大）。完成期全量重哈希只在 fastresume 污点时执行（add 时
  存在既有 `.bitv` / 经缓存句柄跨暂停恢复 / 完成重试）；无污点任务的 have-bits
  全部有磁盘依据（全量初检读盘 / Live 写盘后读回校验），完成即时。
- **已完成 BT 任务完整性校验**（`daemon.task.verify`，慢方法，右键「完整性校验」）：
  `bt_verify.rs` 只读、不改任务状态、不碰 librqbit 会话。元数据按缓存句柄
  `torrent_bytes` → DB `torrent_files` → 会话目录 `{info_hash}.torrent`（磁力，校验
  info-hash）取，全无则 `unverifiable`。落盘映射优先 parts 边车（选中文件最终路径 +
  未选文件边界字节，`bt_partfile::load_sidecar_layout`），无边车按完成布局推断，缺字节的
  边界 piece 记跳过而非损坏。复用 `bt_downloader::verify_pieces_core`（spawn_blocking，
  逐 piece 检查取消）；`IntegrityChecks` 同任务互斥（`conflict`），删除任务 / 关停 /
  调用方丢弃 future 即取消。
- **ED2K**：eDonkey2000 纯 leech。源发现 = 服务器 `GETSOURCES`（手动 `ed2k_server_list` + 订阅 `server.met` 缓存）+ Kad DHT 兜底 + UPnP-IGD 争 HighID + LowID 回调中继。逐块 MD4 + hashset 自校验（违规拉黑 peer）；分块 MD4 root hash（PART_SIZE=9.28MB，幻影尾处理）。进程级共享 `Ed2kClient` 持久服务器会话：读循环处理 IDCHANGE/EOF；`OP_CALLBACK_FAIL` 不带 client_id，按服务器顺序立即失败最早的待决回调；有活跃 ED2K 任务时断线后台重连（1→30s 指数退避封顶），无活跃任务不重连，不发协议外心跳。链接大小上限 256 GiB；建块/预分配前做磁盘余量预检（不足即失败、不建文件与块行）。块状态以内存快照为准供 200ms 上报读取，DB 只在启动恢复与终验坏块时全量读；块完成按批事务落库，且数据 `sync_data` 与 hashset 落库先于 verified 标记提交。
- **HTTP 临时文件独占**：多段与单流写临时文件前按规范化路径（Windows 忽略大小写）取进程内 RAII 独占；同一下载运行的嵌套调用与 worker 共享，另一任务撞同一路径时以 `DownloadError::Io(WouldBlock)` 失败进入错误态，不截断/覆盖对方数据，占用方结束后可重试。
- **文件已存在策略**（config `file_exists_behavior` = `rename` / `overwrite` / `skip` / `ask`；纯决策 `file_exists.rs::decide`，副作用在启动序幕 `finalize_start_file_name`）：只在目标路径已有**普通最终文件**时生效。优先级：跳过（逐任务决定或全局 skip，只看最终文件）→ 硬冲突（`.fdownloading` / 兄弟预订 / 同名目录，恒编号改名）→ `tasks.exists_decision` → 全局策略 → `ask` 降级（协议不可询问 HLS / BT / DASH 轨对、`tasks.unattended=1`、`HostSelection::can_prompt()=false` 均按重命名）→ 询问。可询问：HTTP/HTTPS/FTP（含跳过）、ED2K 与 DASH `.mpd`（无跳过）；非 GET 请求不询问。询问时任务**让出并发槽**挂起（`awaiting_decision`，DB 状态保持 0，`has_capacity` 不计），序幕 task 经 `HostSelection::select_file_exists`（60s）等待，答复经既有 `TaskDone` 通道以 `TaskDonePhase::{AwaitingDecision, DecisionReady}` 回流（宿主无需新增 drain 通道，只有 `Finished` 表示任务结束）；取消 = 暂停，其余答复落 `exists_decision` 后以队首重排，重入不重复触发 onStart / `task.started`。覆盖授权 `OverwritePolicy::{Never, Any, Only(name)}`：逐任务覆盖只绑定询问时的名字（完成期精修成别名则不覆盖），旧文件只在完成认领时替换。跳过 = 「采纳」已有文件：任务记 `exists_decision='skip'`，不拥有该文件（重新下载 / 删除任务和文件不删它，不打 MOTW、不发插件 Done，仍发 webhook completed）。用户改名与重新下载清空决定。
- **从未起跑的任务走启动序幕**：恢复路径（稍后下载、排队中暂停后恢复、重启自动恢复、重新下载、答复后重入）遇到无任何落盘进度的非 BT 任务（`never_started`）按全新起跑处理，命名 / dedup / 跳过 / 询问统一生效；已有进度的续传不再经过序幕，因此不会重复询问。HLS 起跑 dedup 同时避让同名 `<stem>.mp4` 成品。
- **启动序幕与下载器共用一轮探测**：fresh、非 hint、无音轨对的 http(s) GET 类任务名字未知时，序幕（`download_manager::probe_start_meta`）不再跑 `meta_prober` 的 8s 裸 HEAD，而是用与下载相同的 client / spec 跑 `downloader::resolve_file_info_with_ua_fallback`（HEAD ∥ Range 0-0，conclusive 206 后 HEAD 只再给 1s 宽限，含抖动重试与 UA/Referer 自适应），名字由 `ProbedMeta::from_file_info` 派生（仅 Content-Disposition 为权威名，其余 `name_inferred`；text/html 不产出名字），结果连同自适应 spec（序幕已落库）经 `DownloadParams.preprobed`（`PreprobedInfo`）交给 `run_download_inner` 跳过第二轮探测。序幕探测失败 / 取消 → `preprobed=None`，下载器自己探测（旧行为）。hint、续传、HLS/DASH/FTP/BT/ed2k、非 GET 仍走原路径；`meta_prober` 仅保留给排队态后台探测与非 HTTP 协议。

### 引擎子系统（一句话职责）
- `download_manager.rs`（~7300 行）：任务生命周期、并发、队列（内置 + 命名，启停/每日定时边沿触发/顺序）、任务组、自动重试、协议分发、off-actor 插件解析插桩、速度平滑（EMA α=0.4，1s 采样窗；`ProgressUpdate::transfer_bytes` 存在时以接收字节测速并随转发帧发宿主本地 `EngineEvent::TaskTransferred`，两种计数切换时重设基线）、WAL checkpoint。
- `downloader.rs`：共享原语（`DownloadError` 含 Ed2k/Ed2kIntegrity/Cancelled、`RequestSpec`、HTTP 探测与下载执行、完成期原子占名）。
- `naming/`：**文件命名管线**（唯一的名字推断 / 编码解码 / sanitize 实现，其余协议与宿主一律调用它）。证据权威顺序：显式名（用户 / API `out` / 扩展发来的非空名，只 sanitize；缺类型扩展名时补扩展名，见下）> 实际响应 `Content-Disposition`（RFC 6266 受限 quoted-pair、RFC 8187 任意 WHATWG 标签、RFC 2231 续行、RFC 2047 encoded-word、百分号 `filename=`）> URL 查询 `response-content-disposition`/`rscd` > URL 路径末段 > `Content-Type` / 文件头魔数（`sniff_extension`）补或换扩展名。未声明字符集的原始字节走 `decode_legacy_bytes`：严格 UTF-8 → 无打分器的页面字符集 / `.kr` EUC-KR → GBK/Big5/Shift_JIS/windows-1252 按原始字节「常用字区段」打分（TLD +1、页面字符集 +4）。规范与跨实现用例在 `naming/fixtures/content_disposition.json`，扩展 `fluxDown/utils/filename.ts` 逐条镜像（AGENTS.md §5）。
  - **来源与完成期定名**：`tasks.name_inferred` = 引擎自行推断的名字（无 CD 的探测名、URL 占位）；`tasks.resp_name` / `resp_mime` = 实际 GET 响应的 CD 名与 Content-Type（非 hint 在 probe 后、hint 在单流首响应 / 多段起点连接写入，先写者胜，换链清空）。HTTP 完成 rename 前，推断名按「CD 名 → 类型证据」精修后进入原子占名环（冲突重 dedup、落库、完成信号带新名）；显式名与用户改名（`set_task_file_name_with_source(.., false)`）主名与自带扩展名永不改写。HEAD 探测只在 2xx 时产出名字；hint 任务（扩展给了大小）启动与排队期都不发 HEAD，只落 URL 占位名（一次性签名 URL 会被任何请求消耗）。
  - **显式名补扩展名**：显式名缺「类型扩展名」（`needs_type_extension`：无像样扩展名，或尾巴纯数字如 `Ubuntu 24.04`）时补扩展名，**不看请求链接路径后缀**，主名与用户自带扩展名不改；名字已含该扩展名段（`x.7z.001`）不补。三级依次兜底：① **建任务即定名**（`create_task` → `complete_extension_offline`，无网络）：参照名 `NewTaskSpec.name_hint`（wire `DaemonCreateTaskParams.nameHint`；agent 捕获确认时表单改了名就把浏览器定下的原始名放进来）> URL 查询 `response-content-disposition` / `rscd`（GitHub release / 预签名链接）；② 启动序幕（fresh 非 hint HTTP GET，与下载器共用同一轮探测）借探测完整名的扩展名（`complete_extension_from_name`：CD > 重定向后的 CDN 路径 > 脚本端点名按 Content-Type 换出的扩展名；脚本端点 / html / 纯数字扩展名不借，压缩 tar 整体借 `tar.gz`），在「文件已存在」判断与 dedup 之前落库；③ 完成期按 GET 证据补（`complete_explicit_extension`：CD 名扩展名 > 魔数 / Content-Type），覆盖 hint 任务与无扩展名 CDN 对象。
- `segment_advisor.rs`：按文件大小 + CPU 推荐连接上限（HTTP 是上限，coordinator 逐步爬升）。
- `segment_coordinator.rs`（~8500 行）：IDM 式动态分段（按需分配、按 ECF 挑选预计完成最晚的在传分段并按持有者/帮手速率比例均衡拆分、连接复用、per-domain 连接策略学习——负面上限 + 正面起步提示双观察面、`fallocate` 预分配）。拆分计入帮手的请求建立耗时（`SplitCost`：`send→响应头` EWMA + 逐段速率缺失时的「近期总吞吐/在传段数」估计）：持有者在帮手收到首字节前就能下完的余量不拆，拆分点按 `keep = h·(setup·g+R)/(h+g)` 补偿帮手起步延迟（`path_scheduler::split_keep_with_setup`）。子模块 `segment_coordinator/request_path.rs`：SYS 租约的 Range 请求复用首个 206 学到的**重定向终点**（不再每段重放源站 302），终点任一失败（非 2xx / 传输错误 / 头超时）即本任务永久退回原始 URL，不消耗段重试、不当作服务器拒绝；域名连接缓存仍按原始 URL 记。拒绝降级：首个 403/429 乘性减半额度后，降额前已在途的超额连接被拒只退休 worker（`shedding_excess`），只有额度内仍被拒才升串行 / 写单连接缓存。Auto 默认档扩容到天花板时若增益接近线性（吞吐增幅 ≥ 连接增幅 × 0.8，同 Ghost Downloader 自动提速口径）且剩余字节够喂饱，任务内把天花板翻倍（`in_task_uncap_ceiling`，上限同 `HINT_UNCAP_MAX`，负面 cap / 显式段数不解封）。子模块 `segment_coordinator/multipath.rs`：每 ramp 窗口的连接稳态采样（以租约真实开始时刻/字节建立基线；首个完整窗预热，短初窗累计，按实际时长算速率；限速窗不计，只有完整零增量窗判停滞）→ 路径估计、冷路径探索 worker、完成时间抢占（分段子令牌取消 → `WorkerEvent::Preempted` → 余量回 Pending 立即续派）、Auto 主导链路标签与先验回写。
  - **连接调度补充**：`RAMP_INITIAL_WORKERS` = 4（首窗后照常倍增）。段内重试预算按「连续无进展失败」计——本次尝试有字节落盘即重新计数。段内重试耗尽的瞬时错误（判据同管理器自动重试 `download_manager::is_retriable_error`）在仍有其它已验证响应在传、非串行 / 非重连敌对 / Range 已验证时，段回 Pending、退休该 worker，由下个 ramp tick 按额度补新连接（每段至多 `SEGMENT_TRANSIENT_REQUEUE_MAX` = 3 次，超出按原语义整任务失败）。持续劣化收缩带验证：收缩后 `RAMP_SHRINK_EVAL_WINDOWS` 个完整窗内吞吐须超过收缩时 × `RAMP_IMPROVE_FACTOR`，否则判为链路噪声、撤销收缩并在本任务停用该机制。任务内解封（`in_task_uncap_ceiling`，仅 Auto 默认档、尚无拒绝时）越过 advisor 天花板后的首个连接级拒绝（403/429/400）视为越界：回退到解封前天花板并以其为 shedding 额度，不计拒绝、不写域名负面上限，本任务不再解封。最后一条在途连接也收到 429 且已有进度时不立即失败：段回 Pending、进入串行，按 `ALL_REJECTED_REVIVE_BASE_DELAY` × 次数（5s / 10s / 15s）退避后由 ramp tick 派单连接续传，至多 `ALL_REJECTED_REVIVE_MAX` = 3 次，用完才走原致命出口（USTC 镜像按 IP 限并发 / 请求窗口的实测场景）。
  - **版本一致性（不发 If-Range）**：续传与分段只发纯 Range（If-Range 会作废一次性签名 URL），版本安全靠 206 的 ETag/Last-Modified 后验。单流续传（`download_single_once`）的 206 validator 与 DB 基线致命不符时，丢弃旧前缀、不带 Range 重发完整请求并回写新 validator（与 416 / 不可信 206 同一出路），不再以 `VersionChanged` 终态失败（旧基线会让之后每次续传撞同一错误）。多段全程没有任何响应带 ETag（CDN 剥离）且最终有多段时，coordinator 完成前由 `segment_coordinator/splice_audit.rs` 向服务器重取文件首/中/尾各 4KiB 与落盘逐字节比较：确凿不符 → `VersionChanged`（Path B 清盘单流重下）；审计请求失败、非对齐 206、压缩、超时等一律视为无结论放行。
  - **落盘**：周期落库不再在读循环里同步 fdatasync——`FileSyncGate::kick_background` 用同文件独立句柄在阻塞线程池合并刷写（全局每 `MIN_SYNC_GAP` 至多一次），worker 只在下个落库周期把「已被某次已完成 fsync（起始时刻 ≥ 快照时刻）覆盖」的最早挂起快照提交为 durable 水位；后台 fsync 失败经 `failure()` 让所有 worker 上抛。段完成仍走同步 `sync_covering`（与后台共享状态，可复用其结果）。`seg_states` 按段索引升序，worker 每 chunk 用二分定位（`seg_state_pos`）。
- `downloader::resolve_file_info`：HEAD ∥ `Range: bytes=0-0` 并发探测；Range GET 给出决定性 206（带 `Content-Range`）后 HEAD 只再等 `PROBE_HEAD_GRACE`（1s），不让挂起的 HEAD 拖满 15s 探测超时。两路都断在传输层（无任何 HTTP 响应）时前几轮交回重试循环退避重探，只有最后一轮才走无 Range 的 plain GET 兜底（兜底会把任务判成单连接）。
- `speed_limiter.rs`：全局 token bucket（Arc 可克隆，limit==0=不限）。
- `meta_prober.rs`：队列任务后台探测文件名/大小（8s；HTTP HEAD（仅 2xx 产出名字）/ FTP SIZE / magnet dn= / torrent 跳过；hint 任务不调用），返回名字来源供 `name_inferred` 落库。`spawn_meta_probe` 经全局 8 许可 `Semaphore` 限流（许可持有整个探测），上万条导入不会同时打出上万个 HEAD。
- `proxy_config.rs`：无/系统（Windows 注册表）/手动/**自动**（`ProxyMode::Auto`）；HTTP/HTTPS/SOCKS4/5；`test_proxy_connection` 测延迟。
- `egress.rs`：**宿主自身外网请求的唯一出口**（受管组件、插件市场包、tracker / server.met / nodes.dat 订阅；agent 的自更新与嗅探规则包经 `daemon.egress.plan` 取同一方案）。客户端一律 `egress::client_builder`（= 下载任务同一装配，跟随全局代理，`Egress` 每次读最新配置）；内容有独立来源哈希 / 签名的载荷经 `Egress::route_verified` 按下载任务同一 resolver URL 规则交给插件改写（如 GitHub 加速，多插件命中按无人值守规则定案、不弹框），只接受单一 https 直链，改写地址失败或内容不符回退原地址。校验和、索引、版本清单等完整性锚只走代理、不交给插件。下载任务的拦截器阶段不作用于宿主自身下载。插件 `flux.fetch` / yt-dlp 的代理在 `set_proxy_config` 时随之重建（`EngineBridge::set_proxy_config`）。
- `auto_proxy.rs` + `path_scheduler.rs` + `cdn/node_pool.rs`：`ProxyMode::Auto` 是**多路径调度**，没有「采样→一次性热切换」状态机。直连（SYS + 多 CDN 钉定节点）、手动代理、系统代理（同端点去重）都是同一 `NodePool` 的路径；探索即真实分段下载（字节写入文件、零丢弃）。租借规则（`path_scheduler` 纯判据）：冷路径只在 ≥1MiB 的工作上分 1 条探索连接（coordinator 额外放出至多 1 个探索 worker）；失败只降排序不降速率估计（连续 3 次才踢）；单连接估计低于最优一半的路径出竞争集；竞争集内按 cap 分散、按 score 择优；开放式首段/plain GET 只留起飞路径。在途连接按完成时间判据（剩余/本连接速率 > 2×剩余/最优实证速率 + 1s；不设剩余字节下限——建连开销已在交接侧计入，下限会让极慢连接握住拆分最小片以下的尾部碎片拖尾；最优基准只认本任务窗口样本或完成租约，不认先验）在当前字节处抢占交接，整窗零字节即判停滞。代理路径错误归因路径本身（含 Range 失效/错位，翻译为 `CdnNodeFailed` 回收重派），validator 不一致（`VersionChanged` 或 206 路径的 `Other("validator mismatch")`）立即踢除该路径并记 NoSwitch；SYS 上的传输层失败在仍有其它路径存活时同样回收重派（每任务 8 次配额，耗尽后按原语义上抛）；交接窗口抑制 ramp 评估/收缩，用过备选路径的任务不学习域名连接上限/起步提示。跨任务先验 `route_health.rs`：每 (host, 路径) 的单连接速率对数折扣均值（半衰 12h，config `auto_route_health` v2，网络指纹 epoch——换网整表丢弃、**离线=unknown 不清表**），只用于起飞排序（代理先验须领先直连 1.5× 才以代理起飞）与备选路径初值，实测首窗即覆盖。云端只提供 CDN hints 排序先验，不参与路由决策、不新增遥测。failover **独立于通用重试配额**：手动代理、系统代理、本地直连在一个自动恢复周期内各尝试至多一次（先验更快的代理优先），三路均失败后只服从通用重试，杜绝 ping-pong。主导链路（窗口累计字节最多的路径）落 `tasks.auto_route` + `EngineEvent::TaskRouteChanged`（wire 标签不变：`direct[:sampled|:pinned|:failover]` / `proxy:{sampled,cached,failover}:{manual,system}`）。代理设置变更同点清 `route_health` + `domain_conn_caps` + failover 状态。
- `multi_nic.rs`（config `multi_nic_enabled`，默认关，仅桌面 target 编译 `if-addrs`）：多网卡聚合下载。manager 只折算任务级输入（走代理/Auto 多路径 → `blocked_by_proxy`）；coordinator 起飞后后台 `prepare_links`（解析目标 → UDP connect 探主链路 → 枚举网卡 → 纯判据 `plan_links`），首个完整 ramp 窗口经 `NodePool::add_links` 挂入 `RoutePath::Link(ifindex)` 冷槽位，首连接不等待。规划拒绝：fake-IP（198.18/15，TUN 代理）、局域网/CGNAT 目标、主链路是隧道/点对点（防绕开 VPN）；额外链路排除隧道/虚拟网卡、与已选链路同子网或同 /64（同一路由器 = 同一上游）、目标不具备的地址族、5 分钟内被踢过的网卡（进程内记忆，键含本地地址）。出口绑定：Linux/macOS `interface`（SO_BINDTODEVICE/IP_BOUND_IF），Windows `local_address`（强主机模型，单地址族），链路 client 的 DNS 只返回该链路地址族。调度：链路之间按独立容量注水（容量 = 单连接估计 × 上窗连接数，新租约给「容量/(在途+1)」最高者，≥5% 总容量的空闲链路保底 1 条），不走竞争集；链路最后一条连接只在停滞时抢占；链路路径不写 `route_health` 先验、计入 `alternates_used`（不学域名连接上限）。事件：`TaskCdnEvent` kind `links`/`links_off`（原因码 `multi_nic::off_reason`），节点标签 `NIC:<网卡名>`；单节点池无 sink 时事件暂存，由 `Multipath::poll_links` 每窗转发。`native/server` 不接线（废弃路径），生产宿主为 daemon；GPUI（`crates/settings` 下载页「连接与性能」，`SettingsRow::explain` 点击「?」出说明对话框）提供开关。
- `disk_space.rs`：跨平台余量查询（HLS remux/DASH mux ENOSPC 预检）；Unix 原始块数与块大小经 `Into<u64>` 无损扩展后饱和相乘，兼容不同 ABI 的字段宽度，避免同类型强转触发 Clippy。
- `proc.rs`：`no_console_window` —— **每个 console 子进程 spawn 都必须包裹**（ffmpeg/ffprobe/yt-dlp/tar/探版），防 Windows 闪窗。
- `data_dir.rs`：数据目录解析（Windows 便携 `<exe>/portable_data` via `portable` 标记 vs 安装 `%LOCALAPPDATA%`；Linux XDG；macOS App Support；Android files dir）+ 旧版迁移。已有数据迁移不随旧客户端工程退役删除。
- `logger.rs`：全局文件日志宏 `log_info!`/`log_error!`（`#[macro_export]`，`$crate` 前缀跨 crate 安全；每文件顶显式 `use`）。
- `model.rs`/`events.rs`/`selection.rs`：领域类型 / `EngineEvent`（`#[non_exhaustive]`）+ `EventSink` / `HostSelection`。
- `site_auth.rs`：站点 HTTP Basic 凭据存于设备本地 config `site_auth_credentials`。`site_key` 仅支持 HTTP/HTTPS，键为 `scheme://host[:port]`（host 小写、默认端口省略），HTTP 与 HTTPS 隔离；`parse_store` 读取旧 `host` / `host:port` 键时补 `https://`，同站点新旧键并存以新键优先。`download_manager::apply_site_auth` 把凭据注入任务 `extra_headers`，显式用户名/密码优先；无显式凭据且已有 Authorization 时不覆盖。
- `webhook.rs`：`WebhookDispatcher::emit` 同步筛选端点并入队，网络 IO 在端点 worker，失败不影响下载状态。每端点一个有界 FIFO（待投递上限 256），保留事件按 `emit` 入队顺序串行发送；满队列丢最旧待投递项并累计丢弃数，worker 优先写汇总失败投递记录（0 次尝试），不是每条事件派生一个等待任务。全局出站并发 4；dispatcher 释放时关闭队列、唤醒 worker 退出。端点表 `webhook.endpoints` 宽松解析：整份不是 JSON 数组才保留旧表，非对象元素逐项跳过，字段类型不符回退默认值（`lenient` 模块；GPUI `parse_endpoints` / Web `parseEndpoint` 同规则）。`task.created` / `task.started` 早于探测，`download_manager::webhook_task_event` 对空 `file_name` 用 `webhook_provisional_file_name`（http(s)/ftp 路径末段、磁力 `dn=`）补临时名，不带 query。
- `rss/`（`model`/`parser`/`filter`/`mod`）：订阅自动下载。`RssManager` 挂在 `DownloadManager.rss` 上；宿主只提供 60s 节拍（`tick_rss_sources()`）与回流 drain（`on_rss_event()`），抓取 off-actor，建任务仍收敛到 `create_task`。`subscription::SubscriptionProvider` 是 RSS 与插件订阅共用的 provider 分支：内置 `rss` provider 保持原流程；插件在 manifest 的 `subscriptions` 声明 `providerId` + `entry`，由 `PluginManager` 动态路由到 `globalThis.subscribe(ctx)`，返回规范化 `ParsedFeed`，并可通过 `flux.fetch` 请求平台接口。provider 不负责调度、退避、去重、过滤、落库或建任务。三层去重：guid → 单轮上限（超额留 `New` 下轮从旧到新续派）→ 智能剧集去重（识别失败即放行）。

  **两层失败退避不能混为一谈**：订阅 feed 抓取按 `effective_interval_secs` 使用「配置间隔 × `2^fail_count`」，封顶 6h 但绝不短于配置间隔，成功清零，**不自动停用订阅**。条目的种子抓取失败由 `record_torrent_failure` 保持 `New` 并写 `torrent_fetch_failed`，退避为 `600s × 2^(fetch_failures-1)`、封顶 24h；`rss_items.fetch_failures` / `retry_after` 落库，重启不丢退避。`db::rss_dispatchable_items` 仅选到期条目，未失败条目优先，避免失效种子长期占住 `max_per_fetch`；建任务成功或用户手动下载清零条目退避，手动下载不受其限制。

  **enclosure MIME 参与种子识别**：`parser::extract_enclosure` / `map_entry` 保存下载目标声明的 `enclosure_type`（RSS enclosure / media:content、Atom enclosure link；没有 enclosure 时取回退链接的 type），去参数、去空白并转小写。`application/x-bittorrent` 即使 URL 没有扩展名也走「先抓种子字节 → 建 BT 任务」，不只看 URL 形态；magnet 不走种子抓取，二段 resolver 条目的计划不沿用 enclosure MIME。

  `filter.rs` 是纯函数单测主战场，**Web TS 有一份逐条对齐的镜像**（`web/src/pages/rss/filter.ts`）供规则预览用——改任一侧必须同步两处。

  **「无人值守」是 RSS 的核心不变式**——订阅可能半夜抓到 5 集，任何需要用户点一下才能继续的东西都是 bug：① BT 条目建任务时 `NewTaskSpec.unattended_selection=true`，**启动前**落库全选与 `tasks.unattended=1`（HLS/变体选择也静默），否则条目已标「已下载」却仍在等选择；② 引擎内部创建任务也必须通过宿主发布完整任务事实，不能只推缺少队列等字段的进度；③ 手动「重新下载」对**任何**状态（含已下载）都放行。

  **删除任务不触发自动重下**：单删/批删在同一事务清空 RSS 条目的任务回链，将原 `Downloaded` 改为 `Ignored`（已读），其余处置状态保留；不能退回 `New`，否则下一轮自动抓取会重新派发。提交后仅向受影响订阅广播条目快照及源计数，手动下载仍对任何状态开放。

### 受管组件子系统（`components/`，`components` feature）
外部二进制 **ffmpeg + yt-dlp** 的按需安装器/解析器（**不打包**，合规边界——用户在设置「组件」页触发下载）。解析优先级 `manual`（config path）→ `managed`（`<data_dir>/bin/`）→ `system` PATH，wire 为 `ComponentSource{Manual,Managed,System,None}`。ffmpeg = BtbN 静态归档（取单文件，macOS 不支持受管）；yt-dlp = 单平台二进制（全平台）。版本列表经官方镜像 `fluxdown.com/api/components`（旧版客户端用旧域名 `fluxdown.zerx.dev`，两域同源）+ GitHub 兜底。**被两处消费**：插件 `flux.ffmpeg`/`flux.ytdlp` 能力面 + 设置「组件」UI。

---

## 插件系统（`native/engine/src/plugin`，`plugins` feature）

**可选、可失败的下载任务中间层**，JS 编写（rquickjs 沙箱），声明式设置项（双端自动生成表单）。两个正交能力平面 + 门控工具面：

- **订阅平面**：manifest `subscriptions:[{providerId,entry,timeoutMs}]` 声明 provider，脚本导出 `globalThis.subscribe(ctx)`，通过 `flux.fetch` 自主完成平台请求/解析，返回 `{title,link,items:[{guid,title,link,enclosureUrl,resolverItem,enclosureLength,pubDate}]}`（`providerId` 不得占用内置 `rss`；单条非法跳过并记日志，全部非法才算失败；`ctx.providerConfig` 空值归一为 `{}`）；宿主自动把它挂到公共订阅调度，插件安装/启停后由动态路由读取最新快照。
- **Resolver 平面**：`resolve(url,ctx)→{url}|{manifest}|null`。协议判定**之前**惰性执行、**off-actor**（防冻结 actor），命中后 fail-closed（失败进 status=4，绝不把 HTML 当视频存）。惰性 = 每次 start/resume 重跑，天然防直链过期。支持两段式：初段返 manifest 清单 → 引擎裂变为任务组；二段（`ctx.resolverItem`）返直链。`multi:true` 触发新建对话框前置预解析（`begin_resolve_preview` 只读）。
- **通知平面**：onStart/onDone/onError/onMetaProbed，全 fire-and-forget（失败仅记日志/超时/`try_acquire`，绝不影响任务状态）；仅 onError 内可 `flux.task.requestRetry`。
- **门控工具面**（manifest `permissions` 声明才注入）：
  - `flux.auth`（`permissions:["auth"]`）：宿主持久化插件认证档案；`save/get/remove` 管理
    Cookie、Bearer、Basic 或自定义 Header。站点键**含 scheme**（`{scheme}://host[:port]`，
    `auth::site_key`）——同 host 的 http/https 是两个不同站点，https 登录态绝不隐式复用到
    http 请求（H-3，防明文 MITM 窃取）；裸 host 输入默认按 https 规范化，插件要登记明确
    允许明文的站点必须自己传 `"http://host"`。`flux.fetch` 按显式 `authRef`（空串视同未传，
    M-5）或插件+站点默认引用自动注入（仅对声明 `auth` 权限的插件推导默认引用），认证档案
    绑定插件和目标站点，过期时隐式引用跳过注入、显式 `authRef` 返回 `authentication_required`。
    档案按 `plugin.<identity>.auth.<site>` 独立配置键保存；旧版整表键 `plugin_auth_profiles`
    仅作迁移兼容，解析失败时记日志丢弃而非 fail-closed 卡死 save/remove/purge（M-1）。
    `flux.fetch` 返回的同名多值响应头以换行符（`\n`）拼接。auth 平面独立信号量
    （容量 2）+独立预算（`auth.timeoutMs` 可选，默认 30s，30s 硬顶），不与 resolve 共享、
    不计入 resolve 熔断（M-2）。凭据清理只挂用户主动 `uninstall`，安装失败的回滚
    `purge`（含其内部 `plugin.<id>.` 前缀清理）不删凭据（M-4）。
  - 登录入口（manifest `auth.entry`）：宿主 RPC `daemon.plugin.auth` 以
    `begin`/`poll`/`cancel`/`logout`/`status` 调用 `globalThis.authenticate(ctx)`；插件返回二维码挑战，
    成功后用 `flux.auth.save` 提交凭据。`logout` 在插件被禁用时也放行——不跑插件 JS，
    直接走宿主兜底删除凭据（M-3）；`poll` 回包省略 `challenge`/`challengeType` 时客户端
    保留上一帧。
  - `flux.ffmpeg`/`flux.ffprobe`（`permissions:["ffmpeg"]`）：近乎全量 argv，**封网 + 封越牢路径**（拒 URL scheme/绝对路径/`..`），牢笼 = 产物目录（仅 onDone 类有产物钩子可用），sema=2，300s/1800s 超时。
  - `flux.ytdlp`（`permissions:["ytdlp"]`）：允许网络抓站，但参数走 `bridge::ytdlp_option_kind` **白名单**，不是危险开关黑名单。只接受列出的完整长选项（带值选项可用 `--name=value`）和独立短选项，拒绝长选项缩写、合并 / 紧贴值的短选项、未知开关及参数数量不匹配；位置参数仅允许 HTTP/HTTPS URL。路径型参数（含 `-o` / `-P` 的类型前缀与 `--print-to-file` 的目标文件）必须是牢笼内相对路径，拒绝绝对路径、盘符、`..`、家目录 / 环境变量展开；`--js-runtimes` 只收裸运行时名，不收可执行路径。执行 / 配置 / 插件加载、浏览器 / netrc 凭据、任意下载器 / 后处理参数等能力不在允许表内。宿主注入 `--ignore-config`、可信 `--ffmpeg-location`（插件不能覆盖）和牢笼内 `--cache-dir`；resolve + 全 hook 可用。
  - `flux.fs`：per-plugin 通用临时文件读写（扁平安全名 + 单文件 8MB/总量 64MB/文件数 100 上限 + unix 0600），取代"每种输入给工具加类型化字段"的反模式。

**插件工作区**：`bridge::plugin_workspace` 统一生成 `<data_dir>/plugins-work/<encoded_id>/`，供 `flux.fs`、`flux.ytdlp` 的牢笼 / cwd 及卸载清理共用（yt-dlp 可另选牢笼内安全 subdir）。identity 按字节作无碰撞编码：`[A-Za-z0-9-]` 原样保留，其余字节一律写成 `_XX`（两位小写十六进制），包括 `_` → `_5f`、`@` → `_40`、`.` → `_2e`；不能用「特殊字符统一替换成下划线」的有碰撞目录名。

**manifest v2（`main` / `contributes`）**：`main = {entry: *.js, events ⊆ VALID_EVENTS, background}`（`background:true` 需 `background` 权限）是 daemon 常驻宿主入口；`contributes = {views ≤16, commands ≤32, sniffRules?}`（空数组等同未声明）。view `{id ^[a-z][a-z0-9-]{0,31}$, title 1–40 字符, location activity|settings|taskDetail|window（settings ≤1）, desktop?: {entry *.js|*.mjs, networkHosts ≤64, width/height [240,4096] 仅 window}}`；无 `desktop` 的 view 与全部 command 要求有 `main`；带 `desktop` 要求 `desktop` 权限；`sniffRules` 为 `*.json` 且要求 `browserRules`，加载时校验 ≤256 KiB、`format=="fluxdown-sniff-pack"`、≤500 条。`desktop.entry` 只由 GPUI gpui-shell 执行，**引擎从不编译它**，加载时只核对存在。权限全集 `ffmpeg|ytdlp|auth|tasks|browserRules|desktop|background`；风险 `PluginManifest::risk()`：含 `desktop` = full，含其余任一 = elevated，否则 standard。跨实现夹具 `plugin/fixtures/manifest_cases.json` 由 Rust `manifest_fixture_cases`、SDK `packages/plugin-sdk/src/manifest.ts` 共用；索引仓用 SDK 构建产物 `vendor/fluxdown-plugin-manifest.mjs`（改规则后 `bun run sync-index`）。下发：`PluginDto.{contributions, risk, network_hosts, consent_pending, consent_pending_hosts, package_dir}`。

**授权与审计（`plugin/{consent,audit}.rs`）**：授权存配置键 `plugin.<id>.consent = {permissions, networkHosts, version, grantedAt, source: user|migration}`；`requested − consent` 非空即 `DisabledReason::ConsentRequired`（不写 enabled 键，授权后直接生效），`set_enabled(true)` 被拒；`daemon.plugin.consent` 只接受与当前 requested 完全相等的集合。首次 `load_all` 一次性迁移（`plugin.consent_migrated`）把已装插件现有能力记为 `migration` 授权。审计表 `plugin_audit`（SQLite + PG，90 天 / 20000 条修剪），`AuditLog` 后台批量落盘（2 s / 100 条，无事件不写盘）；`AuditingBridge` 记 fetch（去 query / userinfo）/ auth / fs 写删 / ffmpeg / ffprobe / ytdlp / task.retry / task.artifact，`PluginManager` 记生命周期，宿主记 `tasks.*` / `settings.update`，桌面脚本经 `daemon.plugin.audit.record`（origin 强制 desktop、action 须 `desktop.` 前缀）。插件嗅探规则聚合在 `plugin/sniff_pack.rs`（`daemon.plugin.sniffRules`，id 改写 `plugin.<identity>.<id>`、tier `plugin`，sequence 只增不减并持久化在 `plugin.sniff_rules.{sequence,hash}`）。

**QuickJS 实现**：crates.io `rquickjs` 被根 `[patch.crates-io]` 换成 gpui-kit 的兼容外壳（Longbridge `quickjs-jit` 分叉，API 同 rquickjs 0.12），因 gpui-shell 要求全工作区同一 VM 类型；daemon 不启用其 JIT（`jit-abi` 只在桌面界面进程的 gpui-shell 依赖图里）。

**常驻宿主（`main`，`plugin/{host_manager,quickjs_host}.rs`）**：manifest `main` 的插件由 daemon 内一个 actor 常驻（独占 `AsyncRuntime`+`AsyncContext`，64 MiB、每次调用 2 s 中断预算——`flux.fetch` / `flux.tasks` 等挂起时长补回预算——与 30 s 墙钟；rquickjs 类型仅允许出现在 `quickjs.rs` 与 `quickjs_host.rs`）。JS 门面 `flux.ui.{view,h,Fragment,toast}`、`flux.commands.register`、`flux.rpc.handle`、`flux.events.on`、`flux.timers`、`flux.settings.update`、（`tasks` 权限）`flux.tasks.*` 经 `globalThis.__fluxHost.dispatch(kind,json)` 串行分发，返回 `{ok,v}|{ok:false,e}` 信封。`PluginHost::call` 在返回 future 前同步入队，所以 `view_event` 的先后顺序就是处理顺序。`host_manager.rs` 是 `manager.rs` 的 `#[path]` 子模块（直接访问 `PluginManager` 私有字段）：激活 = 打开视图 / 命令 / invoke / `notify` 命中 `main.events` / `background:true` 启动；回收 = 非 background 宿主「无会话 + 无定时器 + 无进行中调用」5 min、会话 90 s 无 keepalive、`load_all` 尾部 `reconcile_hosts` 对 manifest + `main` 源码指纹对账（禁用 / 卸载 / reload_dev / 变化立即 dispose，会话收到 `PluginViewClosed{pluginUnloaded}`）；节拍任务只在有宿主时存在。同会话渲染最少间隔 100 ms（`ctx.refresh()` 合并、事件结果只留最新一棵，票号丢弃迟到旧树）。超时 / OOM 计入 `timeout_streak` 熔断并释放宿主（JS 状态不可信）。`flux.tasks.create` 每插件每分钟 ≤20（`tasks.create rate limited`），`tasks.*` / `settings.update` 写审计；daemon 侧 `plugin_port.rs` 白名单只放行 `daemon.task.{list,get,create,pause,resume,delete}`。渲染结果 `EngineEvent::PluginViewRendered` → daemon `PluginViewStore`（`plugin_views.rs`，挂在 `DaemonEventHub` 上与 RPC 共享）`normalize_view`+`diff_view` → `DaemonEvent::PluginViewFrame`（协议 v13，ops > 1 MiB 退化为根 `Replace`，失败发 `Error` 帧并落到存储树以便下一帧清除 `__error`）；agent 对三个 v13 插件事件原样转发给全部 UI 连接。

**模块**：`auth`（受控认证档案存储、站点绑定、过期判断和请求注入）、`manifest`（校验器 + subscription/provider 声明 + `main` / `contributes` / `interceptors` + 8 个权限 + `auth.entry`）、`semver`、`runtime`（**无 rquickjs 类型**——可换 deno_core；含 Spec/Outcome 跨界结构 + `HostContext`）、`quickjs`（v1 唯一 impl，rquickjs 限在此文件；memory_limit + interrupt + timeout 三重兜底 + 连续 3 次熔断）、`bridge`（网络出口 SSRF 守卫 + flux.* 面）、`manager`（`RwLock<Arc<Vec>>` 整表原子替换，含 `authenticate` 登录入口 + 动态订阅路由 + `interceptors.rs` 任务拦截器）、`dependencies`（权限→组件依赖：ffmpeg→[ffmpeg]，ytdlp→[ytdlp,ffmpeg]，**提醒式非阻断**）、`install`（.fxplug zip：zip-slip + 压缩炸弹防护 + 单层剥壳）、`market`（去中心化市场：Git 版本化联邦索引 `zerx-lab/fluxdown-plugin-index`、内容寻址 `contentHash=sha256(zip)`、多源 failover、per-index sequence 防回滚；条目 `networkHosts` 须覆盖包内 `network_hosts()`；v1 无作者签名，schema 预留）。

**拦截平面（`native/engine/src/intercept/`，始终编译；插件侧在 `plugin/interceptors.rs`，权限 `intercept`）**：manifest `interceptors` 单对象，插件只实现 `globalThis.intercept(ctx)` 并按 `ctx.stage` 分派；线协议以 `intercept/ctx.rs` 的 serde 结构为准（SDK `flux.d.ts` 与之同形）。`mod.rs` = `TaskInterceptor` trait / `NoopInterceptor`（无插件时零调用、不构建上下文）/ `InterceptStage` + `STAGE_NAMES` / `InterceptAbort`；`patch.rs` = `RequestPatch` 原子应用（同主机、协议类别不变、`BLOCKED_REQUEST_HEADERS` 不可设、FTP 拒请求头类字段、`fileName` 仅首跑生效）；`drive.rs` = 各阶段链式执行器（`onFailure=continue` 经 `tolerate` 记活动后跳过，`fail` 与 `probed.fail` / `finalize.fail` 中止）。接入点：`download_manager::{run_http_family_task→prepare_request, spawn_meta_probe, on_task_done(failed)}`、`downloader::{run_download_inner(probed), finalize_with_interceptors}`、`hls_downloader`（选画质 / 点播 `filter_vod_track` / 直播 `run_live_track`）、`dash_downloader`（Period / 画质 / 分片）、`ftp_downloader`（probed / 连接上限）。不变式：①改写必须每次运行重放——resolver 旧 `extraHeaders` 与新 `request` 都收进 `QueuedTask.resolver_patch`，续传同理，重试路径都经 `do_resume_task`；②`RequestSpec.pins{user_agent,referrer}` 锁定后，`spec_with_*` 改写函数与 `escape_bot_challenge` 一律不动 UA / Referer；③`persist_adapted_spec` 只把**未锁定**的 UA / Referer 写回 DB 基础上下文，插件叠加的请求头、method、body、`origin_headers` 绝不落库；④`segments` 覆盖用户线程数，`maxConnections` 经 `DownloadSpec.connection_cap` / `cap_connections` 作硬上限；⑤HLS 过滤指纹存任务级 config 键 `hls_filter_{id}` / `hls_audio_filter_{id}`（`db.rs` 清理任务 config 时同删），指纹变化令续传作废；⑥插件活动统一写 `task_activity` kind `plugin`，只记字段名 / 文件名，禁写请求头值、Cookie、请求体；⑦BT / eD2K 不接入拦截。
**一次性鉴权（拦截平面第二轮）**：阶段共 9 个（新增 `refresh`，位于 `probed` 之后）。`intercept/refresh.rs` = `Refresher`（`for_params` 未声明 `refresh` 时为 `None`，零开销）+ `MAX_UNCONFIRMED_REFRESHES = 5`（连续换发都未换来成功响应即停；各请求点成功时调 `DownloadSpec::confirm`，被拒身份此前成功过则计数归零，故每请求一个令牌可反复刷新）；`DownloadSpec` 持有运行内可替换的请求身份（`current() -> Arc<RequestSpec>`、`refresh_after_rejection`：仅 401/403/410、单飞合并、同代际被拒不重复询问、`route_url`），刷新结果不落库；`DownloadError::PluginAbort` 为插件中止的类型化错误，段级 / 请求级重试遇到即上抛；`RequestPatch.ephemeral/totalBytes/rangeSupported` 让运行跳过元数据探测；`SegmentsResult.rewrite` 逐片改写地址（`drive::run_segments` 返回 `SegmentDecision{dropped,rewrites}`）；`failed` 阶段的插件重试有独立额度（5 次），不受 `max_auto_retries` 开关影响；锁定 UA/Referer 后探测自适应不再重发相同请求。插件侧 `refresh` 超时 `min(timeoutMs, 5s)`。

**市场下载诊断与代理（2026-10-01 修复）**：`MarketClient` 复用 `downloader::build_client_builder`，索引与插件包继承应用的 None/System/Manual、代理认证与 `no_proxy` 规则；daemon 每次取最新下载配置，manager 使用当前已应用代理。每个源/镜像失败记录插件 ID、版本、域名/路径和完整根因链，URL 去除 userinfo/query/fragment。哈希不符或超限仍尝试后续镜像，最终保留内容验证错误而非统统折叠为网络失败；包下载的逐跳 HTTPS / 公网字面量 IP 守卫保持启用。

**市场包声明核对**：`install_entry` 经 `PluginManager::install_from_zip_checked` 在替换旧版本前执行 `verify_package_claim`：包内 manifest 的 identity / version 必须与条目完全相等、permissions 必须是条目 `permissions` 的子集（授权界面展示的是条目权限，fork 索引不可信），否则 `PluginError::ManifestInvalid` → `PluginPackageInvalid`，旧版本不动。

**官方索引发布（`fluxdown-plugin-index`，思源集市式）**：作者在自己的 GitHub 仓库维护插件，`registry.json` 一次登记 `id ↔ owner/repo`（PR 由 `registry-check` 机器人演练准入并评论，人工合并）；`stage.yml` 每小时把登记仓库 Latest Release 的唯一 `.fxplug` 收录为新分片（sha256 对照 GitHub digest、解包按客户端规则校验、identity 等于登记 id、版本严格递增、权限抄自 manifest、镜像 = Release 附件地址），更新全自动，失败开 `stage-fail` issue。分片/包 append-only、历史 sequence 只向高水位以上修复。索引仓库 `scripts/package-lib.mjs` 镜像引擎 `native/engine/src/plugin/{install,manifest}.rs` 的准入规则：改 manifest schema（字段、权限、事件、设置矩阵）或安装限制时同回合同步该文件。

**市场撤回强制执行**：daemon 处理 `daemon.plugin.market.list` 拉到索引后，把 `yanked` 为 `vulnerable` / `malicious` 的 `(plugin_id, version)` 交给 `PluginManager::apply_market_revocations`：命中的已装（非 dev）插件写 `enabled=false` + `disabled_reason=Revoked` + `plugin.<id>.revoked_version`，重载并发 `PluginAutoDisabled { reason: "Revoked" }`（客户端用 `pluginDisabledRevoked` 徽标 / `pluginRevokedToast`）；不再命中时只清 `revoked_version`、保持禁用。被撤回版本 `set_enabled(true)` 报错，安装不同版本时 `finish_install` 自动启用并清标记。agent `plugin_revocation.rs` 启动 120s 后每 24h 触发一次（有非 dev 插件才拉索引；daemon 无插件能力即退出；embedded 不启动）。

**off-actor 惰性 resolve 接线**：`create_task` 经 `PluginManager::route_resolver` 绑定 → 落 `tasks.resolver_plugin_id`（仅存 ID）+ 跳过 meta_prober。`do_start/resume_task` 体首守卫：resolver 非空且未解析 → 占位 active_tasks + off-actor spawn → return。worker 经 `resolve_rx` 回流，actor `select!` 分支 `on_resolve_ready`（复查生命周期 → 用解析后 url 重算五路协议分派）。**宿主 actor 必须接线 `resolve_rx` + `plugin_retry_rx`**。

**多插件 resolver 冲突**（`plugin/routing.rs`）：多个已启用插件的 resolver 同时匹配时，建任务绑首个候选（identity 升序）并置 `tasks.resolver_choice_pending=1`（二段解析任务不置）；worker 在首次 resolve 前 `choose_task_resolver` 定案并 `set_task_resolver` 改绑清 0。优先级：用户确认过且覆盖当前全部候选的站点默认插件 > 无人值守（`tasks.unattended`）回退（站点记录插件 > 已绑回退 > 首个候选，并写未确认 `automatic` 记录）> `HostSelection::select_resolver_plugin` 询问（`SelectionKind::Plugin`，60s 超时取回退，可勾「记住」写确认记录；关闭 = 以 `UserCancelled` 取消任务、保持待选）。同站点选择框经每站点锁串行，记住后排队任务直接沿用。站点键 = 主机名小写去 `www.`；候选出现记录外的新插件即重新询问。卸载插件：待选任务不清绑定（起跑时在剩余候选里重选），站点记录移出该插件、选中它或剩余 <2 的记录删除。RPC `daemon.plugin.siteDefaults` / `daemon.plugin.setSiteDefault`（只能确认 / 改选记录里的候选或删除）。

**config 命名空间**：`plugin.<identity>.enabled`/`.disabled_reason`（`CircuitBreaker` / `Revoked`）/`.revoked_version`/`.setting.<key>`/`.kv.<key>`；`plugin.dev.<identity>`（devMode 路径）；`plugin.site_default.<site>`（多插件冲突的站点记录 JSON `{identity, candidates, automatic}`）；`market.<index_id>.sequence`。identity 格式 `^[a-z0-9_-]+@[a-z0-9_-]+$`。
