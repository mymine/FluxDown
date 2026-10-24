package com.fluxdown.app.feature.devices

import android.util.Log
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.fluxdown.core.host.HostException
import com.fluxdown.core.protocol.LinkLan
import com.fluxdown.core.protocol.LinkPairingCode
import com.fluxdown.core.protocol.LinkRules
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.joinAll
import java.util.concurrent.ConcurrentHashMap

/** 表单 / 核对页的内联错误：本地输入不合法，或主机拒绝（文案由视图按 `AccountText.error(.., Pairing)` 本地化）。 */
internal sealed interface PairingIssue {
    data class Input(val error: LinkRules.InputError) : PairingIssue

    data class Host(val error: HostException) : PairingIssue
}

/**
 * 直连配对页签的状态机（本机配对码 / 发现 / 手动配对 / SAS 核对），同 iOS `PairingModel`。
 *
 * 生命周期：页签出现 [start]；本机局域网实际可达（[LinkLan.Active]）时才申请本机配对码，发现与发起配对不依赖它。
 * 页签消失 / 对话框关闭（[stop] / [dismissed]）= 关闭发现 + 撤回配对码广播 + 放弃进行中的发起流程；
 * 发起流程在 begin 进行中 / SAS 核对 / 等待对方确认时被放弃，一律 `pairFinish(accept=false)` 释放会话。
 * 所有失败都落到可见错误；只有拆除阶段（无界面可显示）才写日志。
 *
 * 调用都落在创建时刻的会话上（[api]），不随后续主机切换改道；[scope] 是应用级作用域，关闭后的拆除仍会执行。
 */
@Stable
internal class PairingModel(
    private val scope: CoroutineScope,
    private val api: LinkApi,
    private val nowMs: () -> Long = System::currentTimeMillis,
    /** 发起配对时同时请求完全控制对端（添加主机的局域网配对）；false = 普通设备配对。 */
    private val requestControl: Boolean = false,
    /** 配对完成。 */
    private val onPaired: (Paired) -> Unit,
    /** 发起流程失败（触感 / 提示）。 */
    private val onFailure: () -> Unit,
) {
    data class Verify(val token: String, val sas: String, val peerName: String, val peerFingerprint: String)

    /** 已配对的对端：设备名 + 指纹（添加主机据此取控制凭据）。 */
    data class Paired(val name: String, val fingerprint: String)

    sealed interface Step {
        /** 表单（本机配对码 + 发现列表 + 手动地址）。 */
        data object Form : Step

        /** SAS 核对：等待用户确认或拒绝。 */
        data class Check(val verify: Verify) : Step

        /** 已确认，等待对端确认（对端有 60 秒窗口）。 */
        data class Finishing(val verify: Verify) : Step

        /** 已配对（对端设备名）。 */
        data class Done(val name: String) : Step

        /** 对端拒绝。 */
        data object Rejected : Step
    }

    var address by mutableStateOf("")
    var code by mutableStateOf("")
        private set
    var step by mutableStateOf<Step>(Step.Form)
        private set
    var connecting by mutableStateOf(false)
        private set

    /** 核对页的「拒绝 / 确认」已提交、等待结果（防重复点击）。 */
    var deciding by mutableStateOf(false)
        private set
    var message by mutableStateOf<PairingIssue?>(null)
        private set

    var own by mutableStateOf<LinkPairingCode?>(null)
        private set

    /** 本机配对码的有效总时长（秒，取自拿到码时的剩余时间），供倒计时环使用。 */
    var ownTotal by mutableStateOf(DEFAULT_OWN_TOTAL)
        private set
    var ownError by mutableStateOf<HostException?>(null)
        private set
    var ownLoading by mutableStateOf(false)
        private set
    var discoveryError by mutableStateOf<HostException?>(null)
        private set
    var scanning by mutableStateOf(false)
        private set

    /** 发现开启 / 重新搜索后的 8 秒搜索窗口内（列表为空时显示「正在搜索…」）。 */
    var searching by mutableStateOf(false)
        private set

    /** 本机局域网三态（由视图按 `agent.gateway` 分区同步；同步前按未就绪处理，不提前申请配对码）。 */
    var lan by mutableStateOf(LinkLan.Disabled)
        private set

    private val ops = Channel<suspend () -> Unit>(Channel.UNLIMITED)

    private val worker: Job = scope.launch { for (op in ops) op() }

    /** 在途的发起 / 确认 / 释放调用（拆除时等它们落定，再关会话）。 */
    private val inflight: MutableSet<Job> = ConcurrentHashMap.newKeySet()

    private fun track(block: suspend () -> Unit) {
        val job = scope.launch(start = CoroutineStart.LAZY) { block() }
        inflight += job
        job.invokeOnCompletion { inflight -= job }
        job.start()
    }

    /**
     * 已 [dismissed] 后等待所有排队的拆除调用（关发现 / 作废配对码）与在途的发起 / 释放调用落定。
     * 之后才能安全关闭这个模型所用的会话，否则这些调用会打在已关闭的会话上而丢失。
     */
    suspend fun awaitTeardown() {
        worker.join()
        inflight.toList().joinAll()
    }

    /** 配对码 / 发现是否正在运行（= 页签可见 && 未关闭）。 */
    private var active = false
    private var visible = false
    private var closed = false
    private var pairingStarted = false

    /** 本机当前展示的配对码（`stopPairing` 带上它，只作废自己的码）。 */
    private var ownCode: String? = null

    /** 发起流程代次：放弃流程（离开页签 / 关闭 / 取消等待）时自增，迟到的响应据此丢弃。 */
    private var flowGeneration = 0
    private var searchJob: Job? = null
    private var expiryJob: Job? = null
    private var seenRequests: Set<String> = emptySet()

    // ── 生命周期 ──

    /** 直连页签出现：开启发现；局域网可达时申请本机配对码。 */
    fun start() {
        visible = true
        closed = false
        reconcile()
    }

    /** 视图同步本机局域网三态；变为 [LinkLan.Active] 时补申请配对码，离开时撤回。 */
    fun syncLan(access: LinkLan) {
        if (lan == access) return
        lan = access
        reconcile()
    }

    private fun reconcile() {
        val shouldRun = visible && !closed
        if (shouldRun && !active) {
            active = true
            enqueue { setDiscovery(true, report = true) }
            beginSearching()
        } else if (!shouldRun && active) {
            active = false
            searchJob?.cancel()
            searchJob = null
            searching = false
            enqueue { setDiscovery(false, report = false) }
        }
        // 本机配对码（及其 mDNS 广播）只在局域网实际可达时申请；发现 / 发起配对不依赖它。
        if (active && lan == LinkLan.Active) {
            refreshOwnCode()
        } else if (pairingStarted || own != null || ownError != null) {
            expiryJob?.cancel()
            expiryJob = null
            val code = ownCode
            ownCode = null
            own = null
            ownError = null
            pairingStarted = false
            // 只作废自己拿到的那个码（多客户端互不干扰）；码还在途中的由 runRefreshOwn 返回时自行作废。
            if (code != null) enqueue { stopPairing(code) }
        }
    }

    /** 直连页签消失：关闭发现与配对码广播（顺序在已排队的操作之后），并放弃进行中的发起流程。 */
    fun stop() {
        visible = false
        reconcile()
        abandonFlow()
    }

    /** 对话框关闭：停止一切，并放弃仍在等待的会话；之后不再复用本实例。 */
    fun dismissed() {
        closed = true
        stop()
        ops.close()
    }

    /** 放弃进行中的发起流程：核对 / 等待阶段立即释放会话，begin 在途的迟到响应由代次丢弃并释放。 */
    private fun abandonFlow() {
        flowGeneration++
        connecting = false
        deciding = false
        val pending = when (val s = step) {
            is Step.Check -> s.verify
            is Step.Finishing -> s.verify
            else -> null
        }
        if (pending != null) {
            step = Step.Form
            track { rejectSilently(pending.token) }
        }
    }

    /** 等待对方确认阶段点「取消」：回到表单并放弃等待。 */
    fun cancelFinishing() {
        val verify = (step as? Step.Finishing)?.verify ?: return
        flowGeneration++
        step = Step.Form
        message = null
        deciding = false
        // 在途的确认调用由主机中止；token 保留到此刻，显式 accept=false 让对端立即收到取消。
        track { rejectSilently(verify.token) }
    }

    private fun beginSearching() {
        searchJob?.cancel()
        searching = true
        searchJob = scope.launch {
            delay(LinkRules.SEARCH_WINDOW_MS)
            searching = false
        }
    }

    // ── 本机配对码 ──

    /** 申请（或换一个）本机配对码；局域网不可达 / 已有申请在途时忽略。 */
    fun refreshOwnCode() {
        if (!active || lan != LinkLan.Active || ownLoading) return
        ownLoading = true
        pairingStarted = true
        enqueue { runRefreshOwn() }
    }

    private suspend fun runRefreshOwn() {
        try {
            if (!active || lan != LinkLan.Active) return
            val result = api.pairingCode()
            if (!active || lan != LinkLan.Active) {
                // 等待期间已离开：这个码没人展示，立刻作废。
                stopPairing(result.code)
                return
            }
            ownCode = result.code
            own = result
            ownError = null
            ownTotal = maxOf(1L, LinkRules.secondsUntil(result.expiresAtUnixMs, nowMs()))
            scheduleExpiry(result.expiresAtUnixMs)
        } catch (e: HostException) {
            ownError = e
        } finally {
            ownLoading = false
        }
    }

    /** 配对码到期自动换新。 */
    private fun scheduleExpiry(expiresAtUnixMs: Long) {
        expiryJob?.cancel()
        val wait = maxOf(expiresAtUnixMs - nowMs(), MIN_REFRESH_DELAY_MS)
        expiryJob = scope.launch {
            delay(wait)
            refreshOwnCode()
        }
    }

    /** 分区里出现新的入站请求：本机配对码已被对端使用的可能性高，换一个。 */
    fun onIncomingRequests(ids: Set<String>) {
        val fresh = ids.any { it !in seenRequests }
        seenRequests = ids
        if (fresh) refreshOwnCode()
    }

    // ── 发现 ──

    private suspend fun setDiscovery(enabled: Boolean, report: Boolean) {
        try {
            api.discoverySet(enabled)
            if (enabled) discoveryError = null
        } catch (e: HostException) {
            if (report) discoveryError = e else Log.w(TAG, "link discovery off failed: ${e.message}")
        }
    }

    /** 重新搜索：发现关 → 开（串行）。 */
    fun rescan() {
        if (!active || scanning) return
        scanning = true
        beginSearching()
        enqueue {
            try {
                setDiscovery(false, report = true)
                setDiscovery(true, report = true)
            } finally {
                scanning = false
            }
        }
    }

    private suspend fun stopPairing(code: String) {
        try {
            api.stopPairing(code)
        } catch (e: HostException) {
            Log.w(TAG, "link stopPairing failed: ${e.message}")
        }
    }

    private fun enqueue(op: suspend () -> Unit) {
        // 通道已关闭（对话框已被 dismissed）：拆除操作由关闭前入队的那批完成，之后不再有新操作。
        ops.trySend(op)
    }

    // ── 配对 ──

    fun updateCode(raw: String) {
        code = LinkRules.normalizePairingCode(raw)
    }

    /** 校验输入并发起配对（`agent.link.pairBegin`），成功进入 SAS 核对。 */
    fun begin() {
        if (connecting || step != Step.Form) return
        when (val v = LinkRules.validate(address, code, requestControl)) {
            is LinkRules.Validation.Failed -> message = PairingIssue.Input(v.error)
            is LinkRules.Validation.Ok -> {
                connecting = true
                message = null
                val generation = flowGeneration
                track {
                    try {
                        val r = api.pairBegin(v.params)
                        if (closed || generation != flowGeneration) {
                            // 对话框 / 页签已在等待期间离开：不留悬挂会话。
                            rejectSilently(r.token)
                        } else {
                            step = Step.Check(Verify(r.token, r.sas, r.peerName, r.peerFingerprint))
                        }
                    } catch (e: HostException) {
                        if (generation == flowGeneration) {
                            message = PairingIssue.Host(e)
                            onFailure()
                        }
                    } finally {
                        if (generation == flowGeneration) connecting = false
                    }
                }
            }
        }
    }

    /** SAS 核对后的决定：确认 → 等待对端；拒绝 → 回到表单。 */
    fun finish(accept: Boolean) {
        val verify = (step as? Step.Check)?.verify ?: return
        if (deciding) return
        deciding = true
        message = null
        val generation = flowGeneration
        if (accept) step = Step.Finishing(verify)
        track {
            try {
                val r = api.pairFinish(verify.token, accept)
                // 等待期间被取消 / 离开：结果作废（配对若已成立，由已配对列表的推送体现）。
                if (generation != flowGeneration) return@track
                when {
                    !accept -> {
                        step = Step.Form
                        code = ""
                    }
                    r.paired -> {
                        val name = r.deviceName ?: verify.peerName
                        step = Step.Done(name)
                        onPaired(Paired(name, verify.peerFingerprint))
                    }
                    else -> step = Step.Rejected
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                if (generation != flowGeneration) return@track
                step = Step.Form
                message = PairingIssue.Host(e)
                onFailure()
            } finally {
                if (generation == flowGeneration) deciding = false
            }
        }
    }

    /** 拒绝页 / 完成后返回表单。 */
    fun backToForm() {
        step = Step.Form
        message = null
        code = ""
        deciding = false
    }

    private suspend fun rejectSilently(token: String) {
        try {
            api.pairFinish(token, accept = false)
        } catch (e: HostException) {
            Log.w(TAG, "link pairFinish(reject) failed: ${e.message}")
        }
    }

    private companion object {
        const val TAG = "Pairing"

        /** 配对码有效期缺省值（秒），拿到码后按真实剩余时间覆盖。 */
        const val DEFAULT_OWN_TOTAL = 120L

        /** 配对码到期后重新申请前的最短等待（防止主机时钟偏差造成刷新风暴）。 */
        const val MIN_REFRESH_DELAY_MS = 3000L
    }
}
