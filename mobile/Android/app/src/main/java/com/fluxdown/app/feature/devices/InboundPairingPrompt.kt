package com.fluxdown.app.feature.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.feature.selection.CountdownLine
import com.fluxdown.app.feature.selection.rememberCountdown
import com.fluxdown.app.feature.settings.account.AccountText
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.app.shell.hostState
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.LinkDevice
import com.fluxdown.core.protocol.AccountErrorContext
import com.fluxdown.core.protocol.HostSection
import com.fluxdown.core.protocol.Json
import com.fluxdown.core.protocol.LinkPairingRequest
import com.fluxdown.core.protocol.LinkRules
import com.fluxdown.fluxui.controls.FluxTag
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetDetent
import com.fluxdown.fluxui.overlay.FluxSheetFooter
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/** 已批准、等待已配对列表出现该指纹才提示「已配对」的有效窗口。 */
private const val AWAITING_WINDOW_MS = 120_000L

/** 已批准的入站请求：指纹 → 对端名、批准时刻、批准时该指纹已有记录的配对时间（重新配对时以其变化为成功信号）。 */
private class Awaiting(val name: String, val at: Long, val pairedAt: Long?)

/** 退场动画期间保留最后一次的请求与已提示过的请求 id。 */
private class PromptMemory {
    var last: LinkPairingRequest? = null
    var shownId: String? = null
}

/**
 * V5 入站配对请求（全局弹层）：对端输入了本机配对码，本机必须核对 SAS 后用显式按钮确认 / 拒绝。
 * 事实来源 = 分区 `agent.linkPairingRequests`（按到期先后排序）；同一时刻只呈现一个，其余以计数提示。
 *
 * - 不可划走 / 点遮罩 / 返回键均不处理（不会误拒）；倒计时到 0 自动拒绝恰好一次；
 * - 点按钮先乐观收起，请求失败则恢复并 toast；自动拒绝失败不重试（请求本身已过期，由 agent 清理）；
 * - 批准后不提前报成功：以 `LinkedDevicesChanged` 里该指纹新出现（或已存在但 `pairedAt` 变化）为准再提示。
 */
@Composable
fun InboundPairingPrompt() {
    val container = LocalAppContainer.current
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val host = hostState()
    val requestsRaw by remember { derivedStateOf { host.value.sections[HostSection.agentLinkPairingRequests] } }
    val requests = remember(requestsRaw) { LinkPairingRequest.listFromJson(Json.parseOrNull(requestsRaw)) }

    /** 已由用户 / 到期处理过、等待分区移除的请求（乐观关闭，避免等主机回写前重复呈现）。 */
    val handled = remember { mutableStateListOf<String>() }
    val announced = remember { HashSet<String>() }
    val awaiting = remember { HashMap<String, Awaiting>() }
    val memory = remember { PromptMemory() }

    val liveIds = remember(requests) { requests.mapTo(HashSet()) { it.sessionId } }
    val queue = remember(requests, handled.toList()) { LinkRules.queue(requests).filter { it.sessionId !in handled } }
    val request = queue.firstOrNull()

    // 请求从分区消失：清掉对应的记录
    LaunchedEffect(liveIds) {
        handled.removeAll { it !in liveIds }
        announced.retainAll(liveIds)
    }

    // 请求在用户处理之前消失：对端取消 / 已过期 / 在别处处理
    LaunchedEffect(request?.sessionId) {
        val prev = memory.shownId
        val now = request?.sessionId
        if (prev != null && prev != now && prev !in handled && prev !in liveIds) {
            overlays.toast(context.str(R.string.mobileIncomingPairingCancelled), FluxToastKind.Info, FluxIcons.Link)
        }
        memory.shownId = now
    }

    // 批准后以已配对列表为准提示成功
    val pairedKey by remember { derivedStateOf { host.value.linkDevices.map { "${it.fingerprint}#${it.pairedAt}" } } }
    LaunchedEffect(pairedKey) {
        if (awaiting.isEmpty()) return@LaunchedEffect
        val now = System.currentTimeMillis()
        awaiting.entries.removeAll { now - it.value.at >= AWAITING_WINDOW_MS }
        val current: Map<String, LinkDevice> = host.value.linkDevices.associateBy { it.fingerprint }
        val hits = awaiting.filter { (fingerprint, entry) ->
            val device = current[fingerprint] ?: return@filter false
            entry.pairedAt?.let { it != device.pairedAt } ?: true
        }
        for ((fingerprint, entry) in hits) {
            awaiting.remove(fingerprint)
            haptics.confirm()
            overlays.toast(context.str(R.string.localPairingPaired, "device" to entry.name), FluxToastKind.Success, FluxIcons.CircleCheck)
        }
    }

    fun respond(target: LinkPairingRequest, accept: Boolean, automatic: Boolean) {
        if (target.sessionId in handled) return
        handled.add(target.sessionId)
        if (accept) {
            val existing = host.value.linkDevices.firstOrNull { it.fingerprint == target.peerFingerprint }
            awaiting[target.peerFingerprint] = Awaiting(target.peerName, System.currentTimeMillis(), existing?.pairedAt)
        }
        val api = LinkApi(container.session)
        container.appScope.launch {
            try {
                api.approve(target.sessionId, accept)
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                awaiting.remove(target.peerFingerprint)
                if (!automatic) handled.remove(target.sessionId)
                haptics.reject()
                overlays.toast(AccountText.error(context, e, AccountErrorContext.Pairing), FluxToastKind.Error, FluxIcons.CircleAlert)
            }
        }
    }

    if (request != null) {
        memory.last = request
        // 首次出现提示一次（触感）：弹层本身是显式决策，不静默
        LaunchedEffect(request.sessionId) {
            if (announced.add(request.sessionId)) haptics.longPress()
        }
    }
    val shown = request ?: memory.last ?: return
    key(shown.sessionId) {
        PromptSheet(
            request = shown,
            visible = request != null,
            queued = (queue.size - 1).coerceAtLeast(0),
            onRespond = { accept, automatic -> respond(shown, accept, automatic) },
        )
    }
}

@Composable
private fun PromptSheet(request: LinkPairingRequest, visible: Boolean, queued: Int, onRespond: (accept: Boolean, automatic: Boolean) -> Unit) {
    val c = FluxTheme.colors
    val type = FluxTheme.type
    val countdown = rememberCountdown(request.sessionId, request.expiresAtUnixMs, active = visible)

    // 到 0：自动拒绝恰好一次（key = 请求 id，随请求消失被取消，不会误拒别的请求）
    LaunchedEffect(request.sessionId) {
        snapshotFlow { countdown.seconds }.first { it <= 0 }
        onRespond(false, true)
    }

    val title = str(R.string.incomingPairingTitle)
    FluxSheet(
        visible = visible,
        onDismissRequest = {},
        detent = FluxSheetDetent.Wrap,
        dismissible = false,
        title = title,
        header = { FluxSheetHeader(title = title, subtitle = str(R.string.incomingPairingFrom, "device" to request.peerName)) },
        footer = {
            FluxSheetFooter {
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    CountdownLine(countdown, R.string.incomingPairingCountdown)
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        FluxButton(
                            str(R.string.incomingPairingReject),
                            onClick = { onRespond(false, false) },
                            variant = ButtonVariant.Danger,
                            modifier = Modifier.weight(1f),
                        )
                        FluxButton(
                            str(if (request.requestsControl) R.string.linkControlApprove else R.string.incomingPairingAccept),
                            onClick = { onRespond(true, false) },
                            variant = ButtonVariant.Primary,
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            }
        },
    ) {
        Column(Modifier.fillMaxWidth().padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(18.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            if (queued > 0) {
                FluxText(
                    str(R.string.mobileIncomingPairingMore, "n" to queued),
                    style = type.sm.copy(textAlign = TextAlign.Center),
                    color = c.inkMuted,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            GlyphTile(platformIcon(request.peerPlatform), size = 56.dp, iconSize = 26.dp)
            FluxText(
                str(R.string.incomingPairingHint),
                style = type.sm.copy(textAlign = TextAlign.Center),
                color = c.inkMuted,
                modifier = Modifier.fillMaxWidth(),
            )
            SasDigits(request.sas)
            if (request.requestsControl) {
                FluxTag(str(R.string.linkControlRequestTag), tone = Tone.Coral)
                FluxBanner(str(R.string.linkControlRequestWarning), kind = FluxBannerKind.Warn)
            }
        }
    }
}
