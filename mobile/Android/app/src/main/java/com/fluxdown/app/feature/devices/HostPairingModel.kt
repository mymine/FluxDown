package com.fluxdown.app.feature.devices

import android.util.Log
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.fluxdown.app.AppContainer
import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.HostRef
import com.fluxdown.core.protocol.LinkRules
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout

/** 配对成功（或选中已配对主机）后「取控制凭据 → 连接 → 保存」的进度。 */
internal sealed interface HostConnect {
    data object Idle : HostConnect

    data class Connecting(val name: String) : HostConnect

    data class Failed(val error: HostException) : HostConnect
}

/**
 * 添加主机「局域网配对」模式的状态机：在 [PairingModel]（发现 / 手动地址 / 配对码 / SAS / 等待批准，`requestControl = true`）
 * 之上接「取控制凭据 → [AppContainer.addRemoteHost]（或 [AppContainer.repairRemoteHost]）→ 切换」。
 *
 * 配对与发现**始终对本机主机**：打开面板后另开一个本机独立会话（[LocalLinkChannel]），与当前选中的主机无关；
 * 离开配对模式 = 停止发现并放弃未完成的配对，面板关闭 = 以上 + 等拆除调用落定后再关会话。
 *
 * [repair] 非空 = 重新配对一台已保存的、被拒的主机：成功后更新同一主机的凭据而不是新增一条，地址预填它的地址。
 * 所有调用落在 [scope]（应用级作用域）上，面板关闭后的拆除仍会执行。
 */
@Stable
internal class HostPairingModel(
    private val container: AppContainer,
    private val scope: CoroutineScope,
    val repair: HostRef.Remote?,
    /** 主机已保存（并已用新凭据连通）。 */
    private val onConnected: (HostRef.Remote) -> Unit,
    /** 配对 / 连接失败（触感）。 */
    private val onFailure: () -> Unit,
) {
    var channel by mutableStateOf<LocalLinkChannel?>(null)
        private set

    /** 配对状态机；本机会话打开前为 null。 */
    var pairing by mutableStateOf<PairingModel?>(null)
        private set

    /** 打开本机会话失败（本机引擎不可用）。 */
    var openError by mutableStateOf<HostException?>(null)
        private set

    var connect by mutableStateOf<HostConnect>(HostConnect.Idle)
        private set

    private class Target(val name: String, val fingerprint: String, val endpoint: String)

    private var wanted = false
    private var opening = false
    private var closed = false
    private var lastTarget: Target? = null
    private var connectJob: Job? = null

    /** 配对模式出现：打开本机会话（首次）并开始发现。 */
    fun activate() {
        if (closed) return
        wanted = true
        val ready = pairing
        if (ready != null) {
            ready.start()
        } else {
            open()
        }
    }

    /** 离开配对模式：停止发现并放弃未完成的配对（会话保留到面板关闭）。 */
    fun deactivate() {
        wanted = false
        pairing?.stop()
    }

    private fun open() {
        if (opening || channel != null) return
        opening = true
        openError = null
        scope.launch {
            try {
                val opened = LocalLinkChannel.open(container)
                if (closed) {
                    opened.close()
                    return@launch
                }
                val model = PairingModel(
                    scope = scope,
                    api = opened.api,
                    requestControl = true,
                    onPaired = { paired -> onPaired(paired) },
                    onFailure = onFailure,
                )
                repair?.let { model.address = it.endpoint }
                channel = opened
                pairing = model
                if (wanted) model.start()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "open local link session failed: ${e.message}")
                openError = e as? HostException ?: HostException(HostErrorCode.Internal, message = e.message ?: "open local session failed", cause = e)
            } finally {
                opening = false
            }
        }
    }

    /** 重试打开本机会话。 */
    fun retryOpen() = open()

    private fun onPaired(paired: PairingModel.Paired) {
        val address = pairing?.address.orEmpty()
        val parsed = parseHostEndpoint(address) as? EndpointParse.Ok
        if (parsed == null) {
            connect = HostConnect.Failed(HostException(HostErrorCode.InvalidArgument, message = "invalid host address"))
            onFailure()
            return
        }
        startConnect(Target(paired.name, paired.fingerprint, parsed.endpoint))
    }

    /** 发现列表里已配对且可控制的主机：跳过配对，直接取控制凭据连接。 */
    fun connectPaired(entry: LinkRules.DiscoveredEntry) {
        val fingerprint = entry.fingerprint ?: return
        val parsed = parseHostEndpoint(entry.address) as? EndpointParse.Ok
        if (parsed == null) {
            connect = HostConnect.Failed(HostException(HostErrorCode.InvalidArgument, message = "invalid host address"))
            onFailure()
            return
        }
        startConnect(Target(entry.name, fingerprint, parsed.endpoint))
    }

    private fun startConnect(target: Target) {
        if (closed || connect is HostConnect.Connecting) return
        lastTarget = target
        connect = HostConnect.Connecting(target.name)
        connectJob = scope.launch {
            try {
                val api = channel?.api
                    ?: throw HostException(HostErrorCode.Unavailable, message = "local link session unavailable")
                val credential = api.controlCredential(target.fingerprint)
                val result = if (repair != null) {
                    container.repairRemoteHost(repair.id, target.endpoint, credential, target.fingerprint)
                } else {
                    container.addRemoteHost(target.name, target.endpoint, credential, target.fingerprint)
                }
                result.fold(
                    onSuccess = { ref -> onConnected(ref) },
                    onFailure = { e -> failConnect(e as? HostException ?: HostException(HostErrorCode.Internal, message = e.message ?: "connect failed", cause = e)) },
                )
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                failConnect(e)
            }
        }
    }

    private fun failConnect(error: HostException) {
        connect = HostConnect.Failed(error)
        onFailure()
    }

    /** 连接失败后用同一目标再试一次（配对已经成立，不重新输入配对码）。 */
    fun retryConnect() {
        val target = lastTarget ?: return
        if (connect is HostConnect.Failed) {
            connect = HostConnect.Idle
            startConnect(target)
        }
    }

    /** 连接失败后回到配对表单（重新选主机 / 输入配对码）。 */
    fun backToPairing() {
        if (connect is HostConnect.Failed) connect = HostConnect.Idle
        pairing?.backToForm()
    }

    /** 面板关闭 / 离开组合：放弃一切进行中的流程，等拆除调用落定后关闭本机会话。只能调用一次有效。 */
    fun close() {
        if (closed) return
        closed = true
        wanted = false
        connectJob?.cancel()
        val model = pairing
        val opened = channel
        model?.dismissed()
        scope.launch {
            try {
                withTimeout(TEARDOWN_TIMEOUT_MS) { model?.awaitTeardown() }
            } catch (_: TimeoutCancellationException) {
                Log.w(TAG, "pairing teardown timed out")
            } finally {
                opened?.close()
            }
        }
    }

    private companion object {
        const val TAG = "HostPairing"
        const val TEARDOWN_TIMEOUT_MS = 5_000L
    }
}
