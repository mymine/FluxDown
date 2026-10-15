package com.fluxdown.core.host

import kotlinx.coroutines.flow.Flow

/**
 * 批量导入（Rust `native/mobile/src/import.rs`，与 GPUI `submission.rs` 同一套语义）：
 * 按 [CHUNK_SIZE] 条分块调用 `daemon.task.createMany`，结果不确定时同一幂等键退避重发，可取消。
 * 这里只放端口模型与纯规则，分块 / 重发 / 取消都在 Rust 侧。
 */
object ImportRules {
    /** 与 Rust `CREATE_CHUNK_SIZE` 相同（`:bridge` 的 `MappingTest` 守护两边一致）。超过它才显示导入进度。 */
    const val CHUNK_SIZE = 256
}

/** 一条待导入的链接；空串 = 由引擎推断 / 跳过校验。 */
data class ImportItem(val url: String, val fileName: String = "", val checksum: String = "")

/** 共享同一份建任务选项的一组链接；[common] 的 `url` / `fileName` / `checksum` 被忽略（由条目决定）。 */
data class ImportBatch(val common: CreateTaskRequest, val items: List<ImportItem>)

/** 等待重发的状态：第 [attempt] / [max] 次，[delaySecs] 秒后。 */
data class ImportRetry(val attempt: Int, val max: Int, val delaySecs: Int)

/** 导入进度：[done] = 已有结论（成功或失败）的条目数。 */
data class ImportProgress(val total: Int, val done: Int, val failed: Int, val retry: ImportRetry? = null) {
    /** 0..1；空导入视为已完成。 */
    val fraction: Float get() = if (total <= 0) 1f else (done.toFloat() / total).coerceIn(0f, 1f)
}

/** 导入汇总：已创建的任务保留（不撤销）。 */
data class ImportSummary(
    val total: Int,
    val succeeded: Int,
    val failed: Int,
    /** 取消 / 连接中断后放弃或结果无法确认的条目数。 */
    val notSubmitted: Int,
    val cancelled: Boolean,
    /** 首个失败原因（单条拒绝 / 整块失败）。 */
    val firstError: HostException?,
    /** 失败 + 未提交的条目（按提交顺序）：新建表单据此只保留它们供重试，已创建的不会重复。 */
    val unfinished: List<ImportItem> = emptyList(),
)

sealed interface ImportUpdate {
    data class Progress(val progress: ImportProgress) : ImportUpdate
    data class Finished(val summary: ImportSummary) : ImportUpdate
}

/** 一次进行中的导入。 */
interface ImportJob {
    /** 更新流（冷流，只能收集一次）：先若干 [ImportUpdate.Progress]，最后一个必为 [ImportUpdate.Finished] 后结束。 */
    val updates: Flow<ImportUpdate>

    /** 请求取消（幂等）：不再发送新分块，在途分块至多再等 3 秒；已创建的任务保留。 */
    fun cancel()
}

/** 汇总提示的结构（对齐 GPUI `local_notice`）；文案由 UI 层按共享 i18n 键填充。 */
sealed interface ImportNotice {
    /** 全部成功：沿用「已创建」提示。 */
    data object Created : ImportNotice

    /** 用户取消（[reason] 非 null = 取消前已有失败）。 */
    data class Cancelled(val added: Int, val total: Int, val reason: HostException?) : ImportNotice

    /** 一条都没建成：只给原因。 */
    data class Failed(val reason: HostException) : ImportNotice

    /** 部分成功。 */
    data class Partial(val added: Int, val total: Int, val failed: Int, val reason: HostException) : ImportNotice
}

/** 提示是否按错误样式展示。 */
val ImportNotice.isError: Boolean
    get() = when (this) {
        ImportNotice.Created -> false
        is ImportNotice.Cancelled -> reason != null
        is ImportNotice.Failed, is ImportNotice.Partial -> true
    }

/** 总数 = 已添加 + 失败 + 未提交（与请求总数相等）。 */
fun ImportSummary.notice(): ImportNotice {
    val total = succeeded + failed + notSubmitted
    val reason = firstError
    return when {
        cancelled -> ImportNotice.Cancelled(succeeded, total, reason)
        reason == null -> ImportNotice.Created
        succeeded == 0 -> ImportNotice.Failed(reason)
        else -> ImportNotice.Partial(succeeded, total, failed, reason)
    }
}

/**
 * 把逐条构造的建任务请求按「共享选项」分组：`url` / `fileName` / `checksum` 之外完全相同的归为一组
 * （组按首次出现的顺序，组内保持原序）。绝大多数导入只有一组；外部唤起追加的链接各带独立的
 * Cookie / 来源页 / 请求头时才会多组，分块不跨组。
 */
fun groupImportBatches(requests: List<CreateTaskRequest>): List<ImportBatch> {
    val groups = LinkedHashMap<CreateTaskRequest, MutableList<ImportItem>>()
    for (request in requests) {
        val common = request.copy(url = "", fileName = "", checksum = "")
        groups.getOrPut(common) { ArrayList() } += ImportItem(request.url, request.fileName, request.checksum)
    }
    return groups.map { (common, items) -> ImportBatch(common, items) }
}
