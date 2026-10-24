package com.fluxdown.app.feature.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.account.AccountErrorLabel
import com.fluxdown.app.feature.settings.account.AccountHint
import com.fluxdown.app.feature.settings.account.AccountText
import com.fluxdown.app.i18n.str
import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.protocol.AccountErrorContext
import com.fluxdown.core.protocol.HostSection
import com.fluxdown.core.protocol.Json
import com.fluxdown.core.protocol.LinkDiscoveredPeer
import com.fluxdown.core.protocol.LinkRules
import com.fluxdown.core.store.HostState
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxSectionTitle
import com.fluxdown.fluxui.controls.FluxTag
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.feedback.FluxSpinner
import com.fluxdown.fluxui.icons.FluxIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.flow.MutableStateFlow

private val NoLocalState = MutableStateFlow(HostState())

/**
 * 添加主机「局域网配对」模式的内容：发现列表 / 手动地址 + 配对码 → SAS 核对 → 等待主机批准 → 取凭据并连接。
 * 状态机在 [HostPairingModel]（配对部分复用 [PairingModel]）；SAS 核对页与 [VerifyFooter] 与「添加设备」共用。
 */
@Composable
internal fun HostPairContent(model: HostPairingModel) {
    val pairing = model.pairing
    val connect = model.connect
    val openError = model.openError
    when {
        connect is HostConnect.Connecting -> Progress(str(R.string.mobileHostConnConnecting))
        connect is HostConnect.Failed -> ConnectFailed(model, connect.error)
        pairing == null && openError != null -> {
            AccountErrorLabel(pairingErrorText(openError))
            FluxButton(str(R.string.localPairingRetryScan), model::retryOpen, variant = ButtonVariant.Secondary, fullWidth = true)
        }
        pairing == null -> Progress(str(R.string.mobileAddHostSearching))
        else -> PairSteps(model, pairing)
    }
}

@Composable
private fun PairSteps(model: HostPairingModel, pairing: PairingModel) {
    when (val step = pairing.step) {
        PairingModel.Step.Form -> HostPairForm(model, pairing)
        is PairingModel.Step.Check -> HostVerify(step.verify, finishing = false, message = pairing.message)
        is PairingModel.Step.Finishing -> HostVerify(step.verify, finishing = true, message = pairing.message)
        // 配对已成立：HostPairingModel 随即进入「取凭据并连接」
        is PairingModel.Step.Done -> Progress(str(R.string.mobileHostConnConnecting))
        PairingModel.Step.Rejected -> {
            FluxBanner(str(R.string.errReasonPairingRejected), kind = FluxBannerKind.Warn, slim = true)
            FluxButton(str(R.string.back), pairing::backToForm, variant = ButtonVariant.Secondary, fullWidth = true)
        }
    }
}

@Composable
private fun HostVerify(verify: PairingModel.Verify, finishing: Boolean, message: PairingIssue?) {
    VerifyContent(
        verify = verify,
        finishing = finishing,
        message = message,
        hintRes = R.string.mobileAddHostVerifyHint,
        waitingRes = R.string.mobileAddHostWaitingApproval,
        issueText = { hostPairIssueText(it) },
    )
}

@Composable
private fun Progress(text: String) {
    Row(
        Modifier.fillMaxWidth().padding(vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        FluxSpinner(size = 20.dp)
        FluxText(text, style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
    }
}

/** 取控制凭据 / 保存 / 连接失败：配对已成立，可重试同一目标，或回到配对表单重新选主机。 */
@Composable
private fun ConnectFailed(model: HostPairingModel, error: HostException) {
    FluxBanner(hostConnectErrorText(error), kind = FluxBannerKind.Error, slim = true)
    // 被主机拒绝（凭据无效）重试没有意义：只给「返回」重新配对
    if (error.code != HostErrorCode.Unauthorized) {
        FluxButton(str(R.string.mobileHostConnect), model::retryConnect, variant = ButtonVariant.Primary, fullWidth = true)
    }
    FluxButton(str(R.string.back), model::backToPairing, variant = ButtonVariant.Secondary, fullWidth = true)
}

@Composable
private fun HostPairForm(model: HostPairingModel, pairing: PairingModel) {
    val localFlow = remember(model.channel) { model.channel?.state ?: NoLocalState }
    val local by localFlow.collectAsStateWithLifecycle()
    val discoveredRaw = local.sections[HostSection.agentLinkDiscovered]
    val entries = remember(discoveredRaw, local.linkDevices) {
        LinkRules.hostEntries(LinkDiscoveredPeer.listFromJson(Json.parseOrNull(discoveredRaw)), local.linkDevices)
    }

    if (model.repair != null) {
        FluxBanner(str(R.string.mobileHostRepairNeeded), kind = FluxBannerKind.Warn, slim = true)
    }
    AccountHint(str(R.string.mobileAddHostPairIntro))
    DiscoveredHosts(model, pairing, entries)

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        FluxSectionTitle(str(R.string.mobileAddHostManualAddress))
        FluxField(
            value = pairing.address,
            onValueChange = { pairing.address = it },
            label = str(R.string.mobileHostAddress),
            placeholder = "192.168.1.20:17800",
            hint = str(R.string.mobileHostAddressHint),
            mono = true,
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Uri,
                imeAction = ImeAction.Next,
            ),
        )
        FluxField(
            value = pairing.code,
            onValueChange = pairing::updateCode,
            label = str(R.string.localPairingCodeLabel),
            placeholder = str(R.string.localPairingCodePlaceholder),
            hint = str(R.string.localPairingCodeHint),
            mono = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { pairing.begin() }),
        )
        pairing.message?.let { AccountErrorLabel(hostPairIssueText(it)) }
        FluxButton(
            str(R.string.mobileHostConnect),
            onClick = pairing::begin,
            variant = ButtonVariant.Primary,
            fullWidth = true,
            loading = pairing.connecting,
        )
    }
}

@Composable
private fun DiscoveredHosts(model: HostPairingModel, pairing: PairingModel, entries: List<LinkRules.DiscoveredEntry>) {
    val searching = entries.isEmpty() && pairing.searching
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        FluxSectionTitle(str(R.string.mobileAddHostDiscovered))
        GlassSection {
            if (searching) {
                custom(padded = true) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        FluxSpinner(size = 20.dp)
                        FluxText(str(R.string.mobileAddHostSearching), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
                    }
                }
            } else if (entries.isEmpty()) {
                custom(padded = true) {
                    FluxText(str(R.string.mobileAddHostNoneFound), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
                }
            }
            entries.forEach { entry ->
                row(hasIcon = true) {
                    val selected = pairing.address == entry.address
                    FluxListRow(
                        title = entry.name,
                        subtitle = listOfNotNull(platformLabel(entry.platform), entry.address).joinToString(" · "),
                        leading = { GlyphTile(platformIcon(entry.platform), tint = FluxTheme.colors.inkMuted) },
                        trailing = when {
                            // 已配对且可控制：点按直接连接（跳过配对）
                            entry.controllable -> ({ FluxTag(str(R.string.localDevicePairedTag), tone = Tone.Mint) })
                            selected -> ({ FluxIcon(FluxIcons.Check, null, size = 20.dp, tint = FluxTheme.colors.accentHi) })
                            else -> null
                        },
                        selected = selected,
                        onClick = { if (entry.controllable) model.connectPaired(entry) else pairing.address = entry.address },
                    )
                }
            }
            row(hasIcon = true) {
                FluxListRow(
                    title = str(R.string.localPairingRetryScan),
                    icon = FluxIcons.RefreshCw,
                    trailing = if (pairing.scanning) ({ FluxSpinner(size = 18.dp) }) else null,
                    enabled = !pairing.scanning,
                    onClick = pairing::rescan,
                )
            }
        }
        pairing.discoveryError?.let { AccountErrorLabel(pairingErrorText(it)) }
    }
}

/** 配对 / 核对阶段的错误文案：对端不支持「请求控制」给专门提示，其余沿用配对错误文案。 */
@Composable
internal fun hostPairIssueText(issue: PairingIssue): String =
    if (issue is PairingIssue.Host && LinkRules.isControlUnsupported(issue.error.reason)) {
        str(R.string.mobileAddHostControlUnsupported)
    } else {
        pairingIssueText(issue)
    }

/** 取凭据 / 保存 / 连接失败的文案：凭据被主机拒绝 = 需要重新配对；带 reason 的按配对错误；其余按连接错误。 */
@Composable
private fun hostConnectErrorText(error: HostException): String {
    val context = LocalContext.current
    return remember(error, context) {
        when {
            LinkRules.isControlUnsupported(error.reason) -> context.str(R.string.mobileAddHostControlUnsupported)
            error.code == HostErrorCode.Unauthorized -> context.str(R.string.mobileHostRepairNeeded)
            error.reason != null -> AccountText.error(context, error, AccountErrorContext.Pairing)
            else -> context.hostErrorText(error)
        }
    }
}
