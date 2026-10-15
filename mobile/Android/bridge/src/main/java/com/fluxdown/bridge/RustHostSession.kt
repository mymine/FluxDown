package com.fluxdown.bridge

import com.fluxdown.core.host.CreateTaskRequest
import com.fluxdown.core.host.HostSignal
import com.fluxdown.core.host.ImportBatch
import com.fluxdown.core.host.ImportItem
import com.fluxdown.core.host.ImportJob
import com.fluxdown.core.host.ImportUpdate
import com.fluxdown.core.model.SelectionOutcome
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import com.fluxdown.core.host.HostSession as HostPort

/**
 * `:core` 的 [HostPort] 端口在 UniFFI 绑定上的实现：一个生成的 `HostSession`（Rust 侧已做握手、游标、重同步、
 * 重连与离线宽限）对应一台主机。信号流是“拉”模型，[signals] 只在被收集时逐个拉取；收集被取消即停止拉取，
 * 会话本身由 [close] 结束。
 */
internal class RustHostSession(private val session: HostSession) : HostPort {
    override val signals: Flow<HostSignal> = flow {
        while (true) {
            val dto = guarded { session.nextSignal() } ?: break
            emit(dto.toCore())
        }
    }

    override fun close() {
        session.disconnect()
        session.destroy()
    }

    override suspend fun createTask(request: CreateTaskRequest): String =
        guarded { session.createTask(request.toDto()) }

    override suspend fun pause(taskId: String) = guarded { session.pause(taskId) }
    override suspend fun resume(taskId: String) = guarded { session.resume(taskId) }
    override suspend fun delete(taskId: String, deleteFiles: Boolean) = guarded { session.delete(taskId, deleteFiles) }
    override suspend fun pauseMany(taskIds: List<String>) = guarded { session.pauseMany(taskIds) }
    override suspend fun resumeMany(taskIds: List<String>) = guarded { session.resumeMany(taskIds) }
    override suspend fun deleteMany(taskIds: List<String>, deleteFiles: Boolean) =
        guarded { session.deleteMany(taskIds, deleteFiles) }

    override suspend fun pauseAll() = guarded { session.pauseAll() }
    override suspend fun resumeAll() = guarded { session.resumeAll() }
    override suspend fun rename(taskId: String, fileName: String) = guarded { session.rename(taskId, fileName) }
    override suspend fun changeUrl(taskId: String, url: String) = guarded { session.changeUrl(taskId, url) }
    override suspend fun rescan() = guarded { session.rescan() }

    override fun startImport(batches: List<ImportBatch>): ImportJob =
        RustImportJob(session.startImport(batches.map { it.toDto() }), batches.flatMap { it.items })

    override suspend fun moveToQueue(taskId: String, queueId: String) =
        guarded { session.moveToQueue(taskId, queueId) }

    override suspend fun boost(taskId: String) = guarded { session.boost(taskId) }
    override suspend fun resolveSelection(requestId: String, outcome: SelectionOutcome) =
        guarded { session.resolveSelection(requestId, outcome.toDto()) }

    override suspend fun patchConfig(expectedRevision: Long, values: Map<String, String>) =
        guarded { session.patchConfig(expectedRevision.toULong(), values) }

    override suspend fun refreshRssSource(sourceId: String) = guarded { session.refreshRssSource(sourceId) }
    override suspend fun setRssSourceEnabled(sourceId: String, enabled: Boolean) =
        guarded { session.setRssSourceEnabled(sourceId, enabled) }

    override suspend fun call(method: String, paramsJson: String?): String =
        guarded { session.call(method, paramsJson) }
}

/**
 * 一次导入：生成的 `ImportHandle`（拉取模型）适配成 [ImportJob]。[updates] 被收集时逐个拉取，`Finished` 后结束并释放句柄；
 * 收集被取消只停止拉取，不取消导入（导入由 [cancel] 取消）。[items] = 各组条目按提交顺序拼接，
 * 汇总里的 `unfinished` 序号据此还原成条目。
 */
private class RustImportJob(private val handle: ImportHandle, private val items: List<ImportItem>) : ImportJob {
    override val updates: Flow<ImportUpdate> = flow {
        try {
            while (true) {
                val update = guarded { handle.nextUpdate() }?.toCore(items) ?: break
                emit(update)
                if (update is ImportUpdate.Finished) break
            }
        } finally {
            handle.destroy()
        }
    }

    /** 导入已结束（句柄已释放）后的取消没有意义：生成的对象此时抛 [IllegalStateException]，直接忽略。 */
    override fun cancel() {
        try {
            handle.cancel()
        } catch (ignored: IllegalStateException) {
            // 已结束：无事可取消
        }
    }
}
