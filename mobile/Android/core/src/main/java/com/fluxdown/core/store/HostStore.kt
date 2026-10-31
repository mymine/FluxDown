package com.fluxdown.core.store

import com.fluxdown.core.host.HostEvent
import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.host.HostSession
import com.fluxdown.core.host.HostSignal
import com.fluxdown.core.host.HostSnapshot
import com.fluxdown.core.model.Category
import com.fluxdown.core.model.CloudDevice
import com.fluxdown.core.model.HostInfo
import com.fluxdown.core.model.LinkDevice
import com.fluxdown.core.model.RssSource
import com.fluxdown.core.model.Queue
import com.fluxdown.core.model.QueueOrder
import com.fluxdown.core.model.QueueOrderEntry
import com.fluxdown.core.model.RuntimeStats
import com.fluxdown.core.model.SeedingStatus
import com.fluxdown.core.model.SelectionRequest
import com.fluxdown.core.model.Task
import com.fluxdown.core.model.TaskGroup
import com.fluxdown.core.model.TaskRuntime
import com.fluxdown.core.model.TaskStatus
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** 连接状态（UI 的断连横幅 / 只读态据此判定）。 */
sealed interface Connection {
    data object Connecting : Connection
    data object Live : Connection
    /** 800ms 宽限后仍未恢复：只读。 */
    data object Stale : Connection
    data class Failed(val error: HostException) : Connection
}

/** 单任务实时量：只来自 TaskProgress 事件，非下载中为 0。 */
data class LiveSpeed(val down: Long, val up: Long)

/**
 * 任务表相位：UI 判定「加载态 / 空态 / 计数」的唯一入口。
 * [Loading] 期间 `HostState.tasks` 不是完整列表（可能为空），不得当作“没有任务”。
 */
sealed interface TaskTable {
    /** 任务表正在分页载入；[total] = 主机上的任务总数。 */
    data class Loading(val total: Long) : TaskTable

    /** 任务表完整（可以为空）。 */
    data object Ready : TaskTable
}

/** UI 观察的不可变主机状态。 */
data class HostState(
    val connection: Connection = Connection.Connecting,
    val info: HostInfo? = null,
    /** 按创建顺序（快照顺序 + 新增追加）。 */
    val tasks: List<Task> = emptyList(),
    val runtime: Map<String, TaskRuntime> = emptyMap(),
    val speeds: Map<String, LiveSpeed> = emptyMap(),
    val queues: List<Queue> = emptyList(),
    /** 排队顺序（按引擎排序键的名次 = 「排队 #n」）；顺序未变时前后状态是同一实例。 */
    val queueOrder: QueueOrder = QueueOrder.Empty,
    val groups: List<TaskGroup> = emptyList(),
    val stats: RuntimeStats = RuntimeStats(),
    val priorityTaskId: String? = null,
    val selections: List<SelectionRequest> = emptyList(),
    val speedHistory: SpeedHistory = SpeedHistory.Empty,
    val taskSpeedHistory: Map<String, SpeedHistory> = emptyMap(),
    val config: Map<String, String> = emptyMap(),
    val configRevision: Long = 0,
    val rssSources: List<RssSource> = emptyList(),
    val cloudDevices: List<CloudDevice> = emptyList(),
    val linkDevices: List<LinkDevice> = emptyList(),
    val categories: List<Category> = Category.BUILTIN,
    /** 通用分区（键见 Rust `sections.rs`）：协议 serde wire 的 JSON 字符串。 */
    val sections: Map<String, String> = emptyMap(),
    /**
     * 任务表正在分页载入（协议 v17）：[tasks] 是空的 / 不完整，**不是**完整列表。
     * 此期间 UI 只能显示加载态，不得据 [tasks] 判定空态、计数、批量范围、「任务已被删除」。
     */
    val tasksLoading: Boolean = false,
    /** 主机上的任务总数（仅 [tasksLoading] 时有意义）。 */
    val taskTotal: Long = 0,
) {
    val isReadOnly: Boolean get() = connection != Connection.Live

    /** 任务表相位：[TaskTable.Loading] 时 [tasks] 不完整，空态 / 计数 / 批量范围 / 删除推断都必须让位于加载态。 */
    val taskTable: TaskTable get() = if (tasksLoading) TaskTable.Loading(taskTotal) else TaskTable.Ready

    fun task(id: String): Task? = tasks.firstOrNull { it.taskId == id }

    /** 新建任务的默认队列：配置 `default_queue_id`（仍存在时）→ 主队列 → 首个队列；无队列为空串（= daemon 默认）。 */
    fun defaultQueueId(): String =
        config["default_queue_id"]?.takeIf { id -> queues.any { it.queueId == id } }
            ?: queues.firstOrNull { it.queueId == Queue.MAIN }?.queueId
            ?: queues.firstOrNull()?.queueId
            ?: ""
}

/**
 * 主机投影仓库：按 `native/protocol` 的 reducer 规则（`event.rs` apply_daemon_event / apply_engine_message）
 * 应用信号，发布不可变 [HostState]。
 *
 * 性能：事件在单协程内就地合并进工作副本，发布按 [publishIntervalMs] 合帧（结构性信号立即发布），
 * 高频 TaskProgress 不会造成逐事件的整表拷贝与重组。
 *
 * 线程约束：[scope] 必须是串行调度（如 `Dispatchers.Default.limitedParallelism(1)`），
 * 工作副本只在其上读写。
 */
class HostStore(
    private val scope: CoroutineScope,
    private val clock: () -> Long = System::currentTimeMillis,
    private val publishIntervalMs: Long = 100,
) {
    private val _state = MutableStateFlow(HostState())
    val state: StateFlow<HostState> = _state.asStateFlow()

    private val _notices = MutableSharedFlow<HostEvent.Notice>(
        extraBufferCapacity = 64,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )

    /**
     * 一次性通知（不进快照、不触发发布）：插件登录 / 组件安装进度、会话吊销等由页面按需订阅。
     * 无订阅者时直接丢弃（通知不是状态，错过即无意义）；订阅者跟不上时丢弃最旧的，不阻塞信号泵。
     */
    val notices: SharedFlow<HostEvent.Notice> = _notices.asSharedFlow()

    private var w = Working()
    private var publishJob: Job? = null
    private var sessionJob: Job? = null

    /** 绑定一个主机会话（切换主机时先 [detach] 再绑定新会话）。 */
    fun attach(session: HostSession) {
        detach()
        w = Working()
        _state.value = HostState()
        sessionJob = scope.launch {
            var fatalSeen = false
            try {
                session.signals.collect {
                    if (it is HostSignal.Fatal) fatalSeen = true
                    apply(it)
                }
                // 信号流正常结束：被 detach 取消时这里抛出取消、不改状态；Rust 先发 Fatal 再结束则保持该原因。
                // 其余（会话驱动意外退出 / panic）不能停在最后一帧不报错——那可能是加载态骨架，界面会一直转圈。
                ensureActive()
                if (!fatalSeen) {
                    apply(HostSignal.Fatal(HostException(HostErrorCode.Unavailable, message = "host session ended unexpectedly")))
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                apply(HostSignal.Fatal(e))
            } catch (e: Exception) {
                // 绑定层 / 映射 / 归约的意外异常（含 UniFFI 的 InternalException，它不是 RuntimeException）：
                // 此作用域没有异常处理器，逃逸即结束整个进程（含后台下载）。转成失败态展示并保留原因，进程照常运行。
                apply(HostSignal.Fatal(HostException(HostErrorCode.Internal, message = "signal pump failed: $e", cause = e)))
            }
        }
    }

    fun detach() {
        sessionJob?.cancel()
        sessionJob = null
        publishJob?.cancel()
        publishJob = null
    }

    /** 测试与同步场景：直接应用一个信号。 */
    fun apply(signal: HostSignal) {
        when (signal) {
            is HostSignal.Snapshot -> {
                w = Working.from(signal.snapshot, w)
                w.connection = Connection.Live
                publishNow()
            }
            is HostSignal.Event -> {
                val structural = applyEvent(signal.event)
                if (structural) publishNow() else schedulePublish()
            }
            HostSignal.Stale -> {
                w.connection = Connection.Stale
                w.runtime.clear()
                w.speeds.clear()
                publishNow()
            }
            is HostSignal.Fatal -> {
                w.connection = Connection.Failed(signal.error)
                w.speeds.clear()
                publishNow()
            }
        }
    }

    /** @return true = 结构性变化（立即发布）。 */
    private fun applyEvent(e: HostEvent): Boolean {
        when (e) {
            is HostEvent.TaskChanged -> {
                w.tasks[e.task.taskId] = e.task
                settleRuntime(e.task.taskId, e.task.status)
                return true
            }
            is HostEvent.TaskDeleted -> {
                removeTask(e.taskId)
                return true
            }
            is HostEvent.TaskProgress -> return applyProgress(e)
            is HostEvent.TaskRuntimeChanged -> applyRuntime(e.runtime)
            is HostEvent.QueuesChanged -> {
                w.queues = e.queues
                return true
            }
            is HostEvent.QueuePositionsChanged -> w.queueEntries = e.order
            is HostEvent.QueueOrderChanged -> w.queueEntries = QueueOrder.apply(w.queueEntries, e.removed, e.added)
            is HostEvent.GroupsChanged -> {
                w.groups = e.groups
                return true
            }
            is HostEvent.FileMissingChanged -> {
                for ((id, missing) in e.updates) {
                    val t = w.tasks[id] ?: continue
                    w.tasks[id] = t.copy(fileMissing = missing)
                }
            }
            is HostEvent.PriorityTaskChanged -> w.priorityTaskId = e.taskId?.ifEmpty { null }
            is HostEvent.RuntimeStatsChanged -> {
                w.stats = e.stats
                w.history = w.history.record(clock(), e.stats.totalDownloadBps, e.stats.totalUploadBps)
            }
            is HostEvent.DaemonConnectionChanged -> {
                if (!e.connected) {
                    w.runtime.clear()
                    w.speeds.clear()
                }
                return true
            }
            is HostEvent.SelectionPending -> {
                w.selections.removeAll { it.requestId == e.request.requestId }
                w.selections.add(e.request)
                return true
            }
            is HostEvent.SelectionResolved -> {
                w.selections.removeAll { it.requestId == e.requestId }
                return true
            }
            is HostEvent.ConfigChanged -> {
                w.config = e.values
                w.configRevision = e.revision
                return true
            }
            is HostEvent.RssSourcesChanged -> {
                w.rssSources = e.sources
                return true
            }
            is HostEvent.CloudDevicesChanged -> {
                w.cloudDevices = e.devices
                return true
            }
            is HostEvent.LinkedDevicesChanged -> {
                w.linkDevices = e.devices
                return true
            }
            is HostEvent.CategoriesChanged -> {
                w.categories = e.categories
                return true
            }
            is HostEvent.SectionChanged -> {
                w.sections = w.sections + (e.name to e.json)
                return true
            }
            // 一次性通知：不进 state，也不触发发布。
            is HostEvent.Notice -> _notices.tryEmit(e)
        }
        return false
    }

    private fun applyProgress(e: HostEvent.TaskProgress): Boolean {
        // status == 4 && errorMessage == "deleted" 表示任务已被删除
        if (e.status == TaskStatus.Failed.wire && e.errorMessage == "deleted") {
            removeTask(e.taskId)
            return true
        }
        val prev = w.tasks[e.taskId] ?: return false
        val status = TaskStatus.of(e.status)
        settleRuntime(e.taskId, status)
        w.tasks[e.taskId] = prev.copy(
            status = status,
            downloadedBytes = e.downloadedBytes,
            totalBytes = e.totalBytes,
            errorMessage = e.errorMessage,
            uploadedBytes = e.uploadedBytes,
            seedingStatus = SeedingStatus.of(e.seedingStatus),
            fileName = e.fileName.ifEmpty { prev.fileName },
        )
        val down = if (status == TaskStatus.Downloading) e.speed else 0L
        w.speeds[e.taskId] = LiveSpeed(down, e.uploadSpeed)
        val h = w.taskHistory[e.taskId] ?: SpeedHistory.Empty
        w.taskHistory[e.taskId] = h.record(clock(), down, e.uploadSpeed)
        return prev.status != status
    }

    private fun applyRuntime(rt: TaskRuntime) {
        val task = w.tasks[rt.taskId] ?: return
        if (!task.status.retainsRuntime) return
        val prev = w.runtime[rt.taskId]
        if (prev != null && prev.sampleSequence != 0L && rt.sampleSequence <= prev.sampleSequence) return
        var next = if (rt.segments.isEmpty() && prev != null) rt.copy(segments = prev.segments) else rt
        if (!task.status.isActive) next = inactive(next)
        w.runtime[rt.taskId] = next
    }

    /** 任务状态变化后的运行时收尾（对应 Rust `settle_runtime`）：完成即删除条目，其余非活跃状态清零读数。 */
    private fun settleRuntime(id: String, status: TaskStatus) {
        if (!status.retainsRuntime) {
            w.runtime.remove(id)
            w.speeds.remove(id)
        } else if (!status.isActive) {
            clearActiveRuntime(id)
        }
    }

    private fun clearActiveRuntime(id: String) {
        val rt = w.runtime[id] ?: return
        w.runtime[id] = inactive(rt)
        w.speeds.remove(id)
    }

    private fun inactive(rt: TaskRuntime) = rt.copy(
        activeTransfers = 0,
        connectedPeers = 0,
        segments = rt.segments.map { if (it.active == false) it else it.copy(active = false) },
    )

    private fun removeTask(id: String) {
        w.tasks.remove(id)
        w.runtime.remove(id)
        w.speeds.remove(id)
        w.taskHistory.remove(id)
    }

    private fun schedulePublish() {
        if (publishJob?.isActive == true) return
        publishJob = scope.launch {
            delay(publishIntervalMs)
            publishNow()
        }
    }

    private fun publishNow() {
        publishJob?.cancel()
        publishJob = null
        _state.value = w.toState()
    }

    /** 可变工作副本：仅在 store 协程内访问。 */
    private class Working {
        var connection: Connection = Connection.Connecting
        var info: HostInfo? = null
        val tasks = LinkedHashMap<String, Task>()
        val runtime = HashMap<String, TaskRuntime>()
        val speeds = HashMap<String, LiveSpeed>()
        var queues: List<Queue> = emptyList()
        var queueEntries: List<QueueOrderEntry> = emptyList()
            set(value) {
                field = value
                queueOrderCache = null
            }
        private var queueOrderCache: QueueOrder? = null
        var groups: List<TaskGroup> = emptyList()
        var stats = RuntimeStats()
        var priorityTaskId: String? = null
        val selections = ArrayList<SelectionRequest>()
        var history = SpeedHistory.Empty
        val taskHistory = HashMap<String, SpeedHistory>()
        var config: Map<String, String> = emptyMap()
        var configRevision = 0L
        var rssSources: List<RssSource> = emptyList()
        var cloudDevices: List<CloudDevice> = emptyList()
        var linkDevices: List<LinkDevice> = emptyList()
        var categories: List<Category> = Category.BUILTIN
        var sections: Map<String, String> = emptyMap()
        var tasksLoading = false
        var taskTotal = 0L

        /** 顺序变化才重建不可变视图；其余发布复用同一实例。 */
        private fun queueOrder(): QueueOrder =
            queueOrderCache ?: QueueOrder.of(queueEntries).also { queueOrderCache = it }

        fun toState() = HostState(
            connection = connection,
            info = info,
            tasks = tasks.values.toList(),
            runtime = HashMap(runtime),
            speeds = HashMap(speeds),
            queues = queues,
            queueOrder = queueOrder(),
            groups = groups,
            stats = stats,
            priorityTaskId = priorityTaskId,
            selections = selections.toList(),
            speedHistory = history,
            taskSpeedHistory = HashMap(taskHistory),
            config = config,
            configRevision = configRevision,
            rssSources = rssSources,
            cloudDevices = cloudDevices,
            linkDevices = linkDevices,
            categories = categories,
            sections = sections,
            tasksLoading = tasksLoading,
            taskTotal = taskTotal,
        )

        companion object {
            /** 快照替换投影；速度清空（只来自事件），速度历史保留（同一主机重同步不应抹平曲线）。 */
            fun from(s: HostSnapshot, prev: Working): Working = Working().apply {
                info = s.info
                s.tasks.forEach { tasks[it.taskId] = it }
                runtime.putAll(s.runtime.filter { (id, _) -> tasks[id]?.status?.retainsRuntime == true })
                queues = s.queues
                queueEntries = s.queueOrder
                groups = s.groups
                stats = s.stats
                priorityTaskId = s.priorityTaskId
                selections.addAll(s.pendingSelections)
                config = s.config
                configRevision = s.configRevision
                rssSources = s.rssSources
                cloudDevices = s.cloudDevices
                linkDevices = s.linkDevices
                categories = s.categories
                sections = s.sections
                tasksLoading = s.tasksLoading
                taskTotal = s.taskTotal
                history = prev.history
                // 载入中的快照没有任务行：速度曲线整体保留，等完整快照到达再按存活任务裁剪。
                prev.taskHistory.forEach { (k, v) -> if (s.tasksLoading || k in tasks) taskHistory[k] = v }
                if (!s.daemonConnected) runtime.clear()
            }
        }
    }
}
