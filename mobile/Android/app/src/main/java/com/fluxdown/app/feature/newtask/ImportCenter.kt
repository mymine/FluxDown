package com.fluxdown.app.feature.newtask

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalResources
import android.content.res.Resources
import com.fluxdown.app.R
import com.fluxdown.app.actions.LocalTaskActions
import com.fluxdown.app.i18n.fill
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.host.HostSession
import com.fluxdown.core.host.ImportBatch
import com.fluxdown.core.host.ImportItem
import com.fluxdown.core.host.ImportJob
import com.fluxdown.core.host.ImportNotice
import com.fluxdown.core.host.ImportProgress
import com.fluxdown.core.host.ImportRules
import com.fluxdown.core.host.ImportSummary
import com.fluxdown.core.host.ImportUpdate
import com.fluxdown.core.host.isError
import com.fluxdown.core.host.notice
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxProgressLine
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import java.text.NumberFormat

/**
 * 一次进行中的批量导入。进度 / 取消状态是 Compose 状态：下载页横幅直接读取，
 * 关闭「新建下载」Sheet 不影响它（持有者是进程级的 [ImportCenter]）。
 */
@Stable
class ImportRun internal constructor(
    val id: Long,
    val total: Int,
    /** 以「暂停」创建：完成提示用 [R.string.taskCreatedToast] 而不是「已开始下载」。 */
    val startPaused: Boolean,
    internal val job: ImportJob,
    /** 各组条目按提交顺序拼接（与 Rust 汇总里的序号一致）。 */
    internal val items: List<ImportItem>,
) {
    var progress by mutableStateOf(ImportProgress(total = total, done = 0, failed = 0))
        internal set

    /** 已请求取消，正在等在途分块收尾（至多 3 秒）。 */
    var cancelling by mutableStateOf(false)
        private set

    internal val result = CompletableDeferred<ImportSummary>()

    /** 超过一块才有进度界面；单块导入由新建 Sheet 内联等待。 */
    val showsProgress: Boolean get() = total > ImportRules.CHUNK_SIZE

    /** 请求取消（幂等）：已创建的任务保留，未提交的不再发送。 */
    fun cancel() {
        if (result.isCompleted) return
        cancelling = true
        job.cancel()
    }

    suspend fun await(): ImportSummary = result.await()
}

/** 一次导入结束（给 [ImportEffects] 弹汇总提示）。 */
class ImportFinished(val startPaused: Boolean, val summary: ImportSummary)

/**
 * 批量导入的进程级持有者（挂在 `AppContainer`）：多个导入可并存、各自可取消，且与「新建下载」Sheet 的
 * 生命周期无关。分块 / 重发 / 取消语义都在 Rust 侧（`native/mobile/src/import.rs`），这里只持有句柄、
 * 汇总进度并在结束时发出提示。
 */
class ImportCenter(private val scope: CoroutineScope) {
    private val active = mutableStateListOf<ImportRun>()
    private val notices = Channel<ImportFinished>(capacity = 16, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    private var nextId = 0L

    /** 进行中的导入（Compose 快照状态）。 */
    val runs: List<ImportRun> get() = active

    /**
     * 结束提示流：UI 不在前台时先缓冲，下次有人收集时再给出（每条只投递一次）。
     * 只含 `notify = true` 的导入。
     */
    val finished: Flow<ImportFinished> = notices.receiveAsFlow()

    /**
     * 在 [session] 上启动导入。[notify] = false 时由调用方 [ImportRun.await] 自行处理结果（新建 Sheet 内联等待小导入）。
     */
    fun start(session: HostSession, batches: List<ImportBatch>, startPaused: Boolean, notify: Boolean): ImportRun {
        val total = batches.sumOf { it.items.size }
        val run = ImportRun(nextId++, total, startPaused, session.startImport(batches), batches.flatMap { it.items })
        active += run
        scope.launch {
            val summary = collect(run)
            active.remove(run)
            run.result.complete(summary)
            if (notify) notices.trySend(ImportFinished(startPaused, summary))
        }
        return run
    }

    private suspend fun collect(run: ImportRun): ImportSummary {
        var summary: ImportSummary? = null
        var failure: HostException? = null
        try {
            run.job.updates.collect { update ->
                when (update) {
                    is ImportUpdate.Progress -> run.progress = update.progress
                    is ImportUpdate.Finished -> summary = update.summary
                }
            }
        } catch (e: HostException) {
            failure = e
        }
        return summary ?: interrupted(run, failure)
    }

    /** 更新流没有给出最终结果就结束了（引擎关闭等）：按最后一次进度汇总，其余按未提交计。 */
    private fun interrupted(run: ImportRun, failure: HostException?): ImportSummary {
        val progress = run.progress
        return ImportSummary(
            total = run.total,
            succeeded = (progress.done - progress.failed).coerceAtLeast(0),
            failed = progress.failed,
            notSubmitted = (run.total - progress.done).coerceAtLeast(0),
            cancelled = run.cancelling,
            firstError = failure ?: HostException(HostErrorCode.Internal, message = "import ended unexpectedly"),
            // 结果未知：只能保证还没轮到的条目（已有结论的不重发）
            unfinished = run.items.drop(progress.done),
        )
    }
}

/** 千分位计数（对齐 GPUI 的导入文案）。 */
internal fun formatCount(n: Int): String = NumberFormat.getIntegerInstance().format(n)

/** 全部成功的提示：沿用新建 Sheet 一直以来的「已创建」文案。 */
internal fun createdMessage(resources: Resources, count: Int, startPaused: Boolean): String = when {
    count > 1 -> resources.getString(R.string.mobileDownloadStartedN).fill("n" to formatCount(count))
    startPaused -> resources.getString(R.string.taskCreatedToast)
    else -> resources.getString(R.string.mobileDownloadStarted)
}

/** 汇总提示文案（对齐 GPUI `local_notice`）：失败附首个错误的原因（按错误码本地化）。 */
internal fun importNoticeText(
    resources: Resources,
    notice: ImportNotice,
    count: Int,
    startPaused: Boolean,
    errorText: (HostException) -> String,
): String = when (notice) {
    ImportNotice.Created -> createdMessage(resources, count, startPaused)
    is ImportNotice.Cancelled -> resources.getString(R.string.importCancelled)
        .fill("added" to formatCount(notice.added), "total" to formatCount(notice.total))
        .withReason(notice.reason, errorText)
    is ImportNotice.Failed -> errorText(notice.reason)
    is ImportNotice.Partial -> resources.getString(R.string.importPartial)
        .fill("added" to formatCount(notice.added), "total" to formatCount(notice.total), "failed" to formatCount(notice.failed))
        .withReason(notice.reason, errorText)
}

private fun String.withReason(reason: HostException?, errorText: (HostException) -> String): String =
    if (reason == null) this else "$this: ${errorText(reason)}"

/**
 * 导入结束的汇总提示（壳层根部挂一次）：全部成功 → 已创建提示；取消 → `importCancelled`；有失败 → `importPartial`。
 */
@Composable
internal fun ImportEffects() {
    val container = LocalAppContainer.current
    val overlays = LocalFluxOverlays.current
    val resources = LocalResources.current
    val actions = LocalTaskActions.current
    val haptics = FluxTheme.haptics
    val latestOverlays by rememberUpdatedState(overlays)
    val latestResources by rememberUpdatedState(resources)
    val latestActions by rememberUpdatedState(actions)
    val latestHaptics by rememberUpdatedState(haptics)
    LaunchedEffect(container) {
        container.imports.finished.collect { done ->
            val notice = done.summary.notice()
            val text = importNoticeText(
                latestResources, notice, done.summary.succeeded, done.startPaused,
            ) { latestActions.errorText(it) }
            if (notice.isError) {
                latestHaptics.reject()
                latestOverlays.toast(text, FluxToastKind.Error)
            } else {
                latestHaptics.confirm()
                val icon = if (notice is ImportNotice.Created && done.startPaused) FluxIcons.Clock else FluxIcons.ArrowDown
                latestOverlays.toast(text, if (notice is ImportNotice.Created) FluxToastKind.Accent else FluxToastKind.Info, icon)
            }
        }
    }
}

/**
 * 下载页顶部的导入进度横幅：`importProgress` / `importRetrying` / `importCancelling` + 进度线 + 「取消」。
 */
@Composable
fun ImportProgressBanner(run: ImportRun, modifier: Modifier = Modifier) {
    val progress = run.progress
    val retry = progress.retry
    val retrying = retry != null && !run.cancelling
    val text = when {
        run.cancelling -> str(R.string.importCancelling)
        retry != null -> str(
            R.string.importRetrying,
            "seconds" to formatCount(retry.delaySecs),
            "attempt" to retry.attempt,
            "max" to retry.max,
        )
        else -> str(R.string.importProgress, "done" to formatCount(progress.done), "total" to formatCount(progress.total))
    }
    FluxBanner(
        text = text,
        modifier = modifier,
        kind = if (retrying) FluxBannerKind.Warn else FluxBannerKind.Info,
        icon = if (retrying) FluxIcons.WifiOff else FluxIcons.ArrowDown,
        slim = true,
        actions = {
            FluxProgressLine(progress.fraction, Modifier.weight(1f))
            FluxButton(
                text = str(R.string.cancel),
                onClick = run::cancel,
                variant = ButtonVariant.Secondary,
                size = ButtonSize.Xs,
                enabled = !run.cancelling,
            )
        },
    )
}
