package com.fluxdown.app.feature.settings.service

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.os.PersistableBundle
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import com.fluxdown.app.R
import com.fluxdown.app.actions.LocalTaskActions
import com.fluxdown.app.feature.settings.ContainerDaemonConfigPort
import com.fluxdown.app.feature.settings.daemonWriteDetail
import com.fluxdown.app.feature.settings.daemonWriteErrorText
import com.fluxdown.app.i18n.str
import com.fluxdown.app.AppContainer
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostException
import com.fluxdown.core.protocol.HostMethod
import com.fluxdown.core.protocol.WebhookDeliveriesResponse
import com.fluxdown.core.protocol.WebhookEndpoint
import com.fluxdown.core.protocol.WebhookEndpointWriter
import com.fluxdown.core.protocol.WebhookSimulateResponse
import com.fluxdown.core.protocol.WebhookTestResponse
import com.fluxdown.core.protocol.callJson
import com.fluxdown.core.protocol.callUnit
import com.fluxdown.fluxui.overlay.FluxOverlayState
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import java.text.DateFormat
import java.util.Date

/** 「发送测试」的展示结果；行内测试归属发起的端点（[endpointId]）。 */
internal class WebhookTestReport(val endpointId: String, val success: Boolean, val text: String)

/** 毫秒时间戳 → 本地化日期 + 时间。 */
internal fun formatWebhookTime(ms: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(ms))

internal fun webhookHttpStatus(context: Context, code: Int): String = context.str(R.string.mobileWebhookHttpStatus, "code" to code)

/** 推送结果摘要：成功 `状态码 · 延迟`；失败为错误信息或 HTTP 状态码。 */
internal fun webhookDeliverySummary(context: Context, d: com.fluxdown.core.protocol.WebhookDelivery): String =
    if (d.success) "${d.statusCode} · ${d.latencyMs} ms" else d.error.ifEmpty { webhookHttpStatus(context, d.statusCode) }

/** 把文本写入剪贴板；[sensitive] 时标记 `EXTRA_IS_SENSITIVE`（系统剪贴板预览与同步会隐藏内容）。 */
internal fun copyToClipboard(context: Context, label: String, text: String, sensitive: Boolean = false) {
    val cm = context.getSystemService(ClipboardManager::class.java) ?: return
    val clip = ClipData.newPlainText(label, text)
    if (sensitive) {
        clip.description.extras = PersistableBundle().apply { putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true) }
    }
    cm.setPrimaryClip(clip)
}

/**
 * S12 页面状态：端点写入（读-改-写 + 冲突重放）、测试、模拟、清空投递日志。
 * 测试结果只显示在发起行，保存 / 删除后作废。
 */
@Stable
internal class WebhookController(
    private val container: AppContainer,
    private val context: Context,
    private val overlays: FluxOverlayState,
    private val errorText: (HostException) -> String,
    private val scope: CoroutineScope,
) {
    var testingId by mutableStateOf<String?>(null)
        private set
    var testReport by mutableStateOf<WebhookTestReport?>(null)
        private set
    var simulating by mutableStateOf(false)
        private set
    var simulateText by mutableStateOf<String?>(null)
        private set
    var clearing by mutableStateOf(false)
        private set

    private val writer = WebhookEndpointWriter(ContainerDaemonConfigPort(container))

    private fun text(id: Int, vararg args: Pair<String, Any?>) = context.str(id, *args)

    /** 主机给出的细节（没有则空串）。 */
    private fun detail(e: HostException): String = daemonWriteDetail(e)

    /** 测试 / 模拟失败：主机给出的细节优先，其次按码文案。 */
    private fun failureDetail(e: HostException): String = detail(e).ifEmpty { errorText(e) }

    // ── 端点写入 ──

    /** 保存（新增或按 id 覆盖）；成功回调 [onDone]，失败 toast。 */
    fun save(draft: WebhookEndpoint, onDone: (Boolean) -> Unit) {
        scope.launch {
            val ok = write { writer.upsert(draft) }
            if (ok) dropTestReport(draft.id)
            onDone(ok)
        }
    }

    fun setEnabled(endpoint: WebhookEndpoint, enabled: Boolean) {
        scope.launch { write { writer.setEnabled(endpoint.id, enabled) } }
    }

    fun remove(endpoint: WebhookEndpoint) {
        dropTestReport(endpoint.id)
        scope.launch { write { writer.remove(endpoint.id) } }
    }

    private suspend fun write(block: suspend () -> Unit): Boolean = try {
        block()
        true
    } catch (e: CancellationException) {
        throw e
    } catch (e: HostException) {
        overlays.toast(daemonWriteErrorText(context, e), FluxToastKind.Error)
        false
    }

    // ── 测试 ──

    private fun report(endpointId: String, response: WebhookTestResponse): WebhookTestReport {
        if (response.success) {
            val status = if (response.statusCode == 0) text(R.string.mobileWebhookStatusOk) else response.statusCode.toString()
            return WebhookTestReport(endpointId, true, text(R.string.webhookTestOk, "status" to status, "ms" to response.latencyMs))
        }
        val error = response.error.ifEmpty { webhookHttpStatus(context, response.statusCode) }
        return WebhookTestReport(endpointId, false, text(R.string.webhookTestFail, "error" to error))
    }

    /** 把端点（或编辑器草稿）直接交给 `daemon.webhook.test`，无需先保存。 */
    suspend fun runTest(endpoint: WebhookEndpoint): WebhookTestReport = try {
        val response = WebhookTestResponse.fromJson(container.session.callJson(HostMethod.daemonWebhookTest, endpoint.toJson()))
        report(endpoint.id, response)
    } catch (e: CancellationException) {
        throw e
    } catch (e: HostException) {
        WebhookTestReport(endpoint.id, false, text(R.string.webhookTestFail, "error" to failureDetail(e)))
    }

    /** 行内测试：结果挂在发起行。 */
    fun test(endpoint: WebhookEndpoint) {
        if (testingId != null) return
        testingId = endpoint.id
        testReport = null
        scope.launch {
            try {
                testReport = runTest(endpoint)
            } finally {
                testingId = null
            }
        }
    }

    fun dropTestReport(endpointId: String) {
        if (testReport?.endpointId == endpointId) testReport = null
    }

    // ── 投递日志 ──

    fun simulate() {
        if (simulating) return
        simulating = true
        simulateText = text(R.string.webhookLogPending)
        scope.launch {
            try {
                val response = WebhookSimulateResponse.fromJson(container.session.callJson(HostMethod.daemonWebhookSimulate))
                simulateText = if (response.dispatched == 0) {
                    text(R.string.webhookSimulateNoTarget)
                } else {
                    text(R.string.webhookSimulateDispatched, "n" to response.dispatched)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                simulateText = text(R.string.webhookTestFail, "error" to failureDetail(e))
            } finally {
                simulating = false
            }
        }
    }

    fun clearDeliveries() {
        if (clearing) return
        clearing = true
        scope.launch {
            try {
                container.session.callUnit(HostMethod.daemonWebhookClearDeliveries)
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                overlays.toast(daemonWriteErrorText(context, e), FluxToastKind.Error)
            } finally {
                clearing = false
            }
        }
    }

    /** 预设目录 + 模板变量清单（`daemon.webhook.get`；前端不复制模板内容）。失败 toast 并返回 null。 */
    suspend fun loadCatalog(): WebhookDeliveriesResponse? = try {
        WebhookDeliveriesResponse.fromJson(container.session.callJson(HostMethod.daemonWebhookGet))
    } catch (e: CancellationException) {
        throw e
    } catch (e: HostException) {
        overlays.toast(text(R.string.mobileWebhookCatalogFailed, "message" to failureDetail(e)), FluxToastKind.Warn)
        null
    }
}

@Composable
internal fun rememberWebhookController(): WebhookController {
    val container = LocalAppContainer.current
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val actions = LocalTaskActions.current
    val scope = rememberCoroutineScope()
    return remember(container, context, overlays, actions, scope) {
        WebhookController(container, context, overlays, actions::errorText, scope)
    }
}
