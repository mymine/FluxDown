package com.fluxdown.core.model

/*
 * 主机投影的 Kotlin 侧模型：字段与 `native/protocol` 的 wire DTO 一一对应（camelCase 同名），
 * 由主机适配层（`:bridge` 的 UniFFI 绑定适配）在边界一次性转换：
 * u64→Long、u32→Int、i32 状态码→枚举、Unix 秒字符串→Long。UI 只读这些不可变类型。
 */

/** `TaskDto.status`（0–5）。未知值映射到 [Unknown]，一律不可控制。 */
enum class TaskStatus(val wire: Int) {
    Pending(0), Downloading(1), Paused(2), Completed(3), Failed(4), Preparing(5), Unknown(-1);

    /** 活跃 = 下载中或准备中（与 `event.rs` 的 `matches!(status, 1 | 5)` 一致）。 */
    val isActive: Boolean get() = this == Downloading || this == Preparing

    companion object {
        fun of(wire: Int): TaskStatus = entries.firstOrNull { it.wire == wire } ?: Unknown
    }
}

/** `TaskDto.seedingStatus`（0–8）。 */
enum class SeedingStatus(val wire: Int) {
    None(0), Seeding(1), RatioReached(2), TimeReached(3), UserStopped(4), TaskDeleted(5),
    SessionReleased(6), InactiveStop(7), QueuedForSlot(8), Unknown(-1);

    companion object {
        fun of(wire: Int): SeedingStatus = entries.firstOrNull { it.wire == wire } ?: Unknown
    }
}

/** 协议种类：由 URL / 哨兵推导，仅用于图标与徽标。 */
enum class TaskProtocol { Http, Ftp, Bt, Ed2k, Hls, Plugin }

/** `TaskSourceBytesDto`：加速路径累计字节；源站 = 已下载 − 三者之和。 */
data class SourceBytes(val cdn: Long = 0, val proxy: Long = 0, val nic: Long = 0) {
    val total: Long get() = cdn + proxy + nic
}

/** `TaskDto`（不含速度 / 分段，速度只在事件里，分段在 [TaskRuntime]）。 */
data class Task(
    val taskId: String,
    val url: String,
    val originUrl: String,
    val fileName: String,
    val saveDir: String,
    val status: TaskStatus,
    val downloadedBytes: Long,
    /** 0 = 未知大小。 */
    val totalBytes: Long,
    val errorMessage: String = "",
    /** Unix 秒；0 = 无。 */
    val createdAt: Long = 0,
    val completedAt: Long = 0,
    /** "" = 主队列。 */
    val queueId: String = "",
    val groupId: String = "",
    val rssSourceId: String = "",
    val fileMissing: Boolean = false,
    val autoRoute: String = "",
    val sourceBytes: SourceBytes = SourceBytes(),
    val uploadedBytes: Long = 0,
    val seedingStatus: SeedingStatus = SeedingStatus.None,
    val seedingMessage: String = "",
    val seedingTimeSecs: Long = 0,
) {
    /** 复制链接：读 originUrl，空则回退 url（torrent 任务的 url 是哨兵）。 */
    val shareUrl: String get() = originUrl.ifEmpty { url }

    val protocol: TaskProtocol
        get() = when {
            url.startsWith("magnet:") || url.startsWith("torrent-file://") -> TaskProtocol.Bt
            url.startsWith("ed2k://") -> TaskProtocol.Ed2k
            url.startsWith("ftp://") || url.startsWith("ftps://") || url.startsWith("sftp://") -> TaskProtocol.Ftp
            url.substringBefore('?').endsWith(".m3u8") -> TaskProtocol.Hls
            else -> TaskProtocol.Http
        }

    /** 0..1；总大小未知时为 null（不确定进度）。 */
    val progress: Float?
        get() = if (totalBytes > 0) (downloadedBytes.toDouble() / totalBytes).toFloat().coerceIn(0f, 1f) else null
}

/** `TaskSegmentDto`：真实字节区间；[active] = null 表示未知，不得推断为空闲。 */
data class Segment(
    val index: Int,
    val startByte: Long,
    val endByte: Long,
    val downloadedBytes: Long,
    val active: Boolean?,
)

/** `TaskRuntimeDto`。 */
data class TaskRuntime(
    val taskId: String,
    val sampleSequence: Long,
    val activeTransfers: Int?,
    val connectedPeers: Int?,
    val totalBytes: Long,
    val segments: List<Segment>,
)

/** `QueueDto`。 */
data class Queue(
    val queueId: String,
    val name: String,
    val speedLimitKbps: Long = 0,
    val uploadLimitKbps: Long = 0,
    val maxConcurrent: Int = 0,
    val defaultSaveDir: String = "",
    val position: Int = 0,
    val isRunning: Boolean = true,
    val scheduleEnabled: Boolean = false,
    val scheduleStart: String = "",
    val scheduleStop: String = "",
    /** bit0 = 周一 … bit6 = 周日；127 = 每天。 */
    val scheduleDays: Int = 127,
) {
    companion object {
        const val MAIN = "main"
        const val LATER = "later"
    }
}

/** `GroupDto`；组进度由客户端汇总成员任务。 */
data class TaskGroup(
    val groupId: String,
    val name: String,
    val sourceUrl: String,
    val saveDir: String,
    val createdAt: Long,
)

/** `DaemonRuntimeStatsDto`。 */
data class RuntimeStats(
    val activeTasks: Int = 0,
    val pendingTasks: Int = 0,
    val totalDownloadBps: Long = 0,
    val totalUploadBps: Long = 0,
    /** null = 探测失败。 */
    val diskFreeBytes: Long? = null,
    val saveDir: String = "",
    val retryPendingTasks: Int = 0,
)

/** 引擎发起的选择请求（X1–X3），到期按 [defaultChoice] 处理。 */
data class SelectionRequest(
    val requestId: String,
    val taskId: String,
    val kind: SelectionKind,
    val defaultChoice: SelectionOutcome,
    val deadlineUnixMs: Long,
)

sealed interface SelectionKind {
    data class Hls(val options: List<HlsOption>) : SelectionKind
    data class Bt(val files: List<BtFile>) : SelectionKind
    data class Variant(val options: List<VariantOption>) : SelectionKind
    /** 多个启用插件的解析器都匹配同一 URL：让用户选一个（`candidates` 按 identity 升序）。 */
    data class Plugin(val site: String, val url: String, val candidates: List<PluginCandidate>) : SelectionKind

    /** 保存目录里已有同名普通文件（`file_exists_behavior = ask`）；字段由主机算好。 */
    data class FileExists(
        val fileName: String,
        val saveDir: String,
        /** null = 无法读取已有文件的大小。 */
        val existingSize: Long?,
        val existingModifiedUnixMs: Long?,
        /** null = 新下载的总大小未知。 */
        val incomingSize: Long?,
        /** 选「重命名」时实际会用的文件名。 */
        val renamePreview: String,
        /** 本次允许的动作（协议不支持跳过时不含 [FileExistsAction.Skip]）。 */
        val actions: List<FileExistsAction>,
    ) : SelectionKind
}

enum class FileExistsAction { Rename, Overwrite, Skip }

data class HlsOption(val index: Int, val bandwidth: Long, val width: Long, val height: Long)
data class BtFile(val index: Int, val path: String, val size: Long)
data class PluginCandidate(val identity: String, val name: String, val version: String, val description: String)
data class VariantOption(
    val index: Int,
    val label: String,
    val container: String,
    val bandwidth: Long,
    val width: Long,
    val height: Long,
    val totalBytes: Long,
)

sealed interface SelectionOutcome {
    data class Hls(val index: Int) : SelectionOutcome
    data class Bt(val indices: List<Int>) : SelectionOutcome
    data class Variant(val index: Int) : SelectionOutcome
    data class Plugin(val identity: String, val remember: Boolean) : SelectionOutcome
    data class FileExists(val action: FileExistsAction) : SelectionOutcome
    data object Cancelled : SelectionOutcome
}

/** `ServiceHello` 的 UI 相关子集。 */
data class HostInfo(
    val serviceName: String,
    val serviceVersion: String,
    val protocolVersion: Int,
    val capabilities: Set<String>,
)

/** 主机身份：本机（进程内 daemon + agent）或已保存的 `--server` 主机。 */
sealed interface HostRef {
    val id: String
    val displayName: String

    data class Local(override val displayName: String) : HostRef {
        override val id: String get() = ID

        companion object {
            const val ID = "local"
        }
    }

    data class Remote(override val id: String, override val displayName: String, val endpoint: String) : HostRef
}

/** RSS 订阅（`RssSourceDto` 的 UI 子集）。 */
data class RssSource(
    val sourceId: String,
    val name: String,
    val url: String,
    val enabled: Boolean,
    val autoDownload: Boolean,
    val intervalMinutes: Int,
    val lastSuccessAt: Long,
    val lastError: String,
    val failCount: Int,
    val unreadCount: Int,
)

/** 云端已信任设备（`CloudDevice` 子集）。 */
data class CloudDevice(
    val deviceId: String,
    val name: String,
    val platform: String?,
    val isOnline: Boolean,
    val isCurrent: Boolean,
    val appVersion: String?,
    /** 设备自报的默认下载目录（远程下发不填保存目录时目标使用它）。 */
    val defaultSaveDir: String? = null,
    /** 设备自报的路径风格 wire 名（`windows` / `posix`）；null = 未上报，按 [platform] 推断。 */
    val pathStyle: String? = null,
)

/** 局域网已配对设备（`LinkDeviceInfo`）；[name] 已是生效名（备注名优先）。 */
data class LinkDevice(
    val fingerprint: String,
    val name: String,
    val platform: String?,
    val online: Boolean,
    /** 设备自报的默认下载目录（远程下发不填保存目录时目标使用它）。 */
    val defaultSaveDir: String? = null,
    /** 设备自报的路径风格 wire 名（`windows` / `posix`）；null = 未上报，按 [platform] 推断。 */
    val pathStyle: String? = null,
    /** 本机备注名（`agent.link.rename`）；null = 未设置，[name] 即对端自报名。 */
    val customName: String? = null,
    /** 对端自报的客户端版本。 */
    val appVersion: String? = null,
    /** 对端自报的云设备 id：本机云名册里存在同 id 时与云条目合并展示。 */
    val cloudDeviceId: String? = null,
    /** 对端已解除与本机的配对：不可下发，只能删除或重新配对。 */
    val revoked: Boolean = false,
    /** 配对时间（Unix 毫秒）。 */
    val pairedAt: Long = 0,
    /** 最近一次确认在线的时间（Unix 毫秒，0 = 未知）。 */
    val lastSeenAt: Long = 0,
)
