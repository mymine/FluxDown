package com.fluxdown.app.feature.devices

import android.content.ClipData
import android.content.ClipboardManager
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.account.AccountErrorLabel
import com.fluxdown.app.feature.settings.account.AccountHint
import com.fluxdown.app.feature.settings.account.AccountText
import com.fluxdown.app.feature.settings.account.rememberNowMs
import com.fluxdown.app.i18n.str
import com.fluxdown.app.nav.LocalNavigator
import com.fluxdown.app.nav.Route
import com.fluxdown.app.nav.SettingsPage
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.app.shell.hostState
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.HostRef
import com.fluxdown.core.protocol.AccountErrorContext
import com.fluxdown.core.protocol.AgentSessionDto
import com.fluxdown.core.protocol.GatewayStatusDto
import com.fluxdown.core.protocol.HostCapability
import com.fluxdown.core.protocol.HostSection
import com.fluxdown.core.protocol.Json
import com.fluxdown.core.protocol.LinkDiscoveredPeer
import com.fluxdown.core.protocol.LinkLan
import com.fluxdown.core.protocol.LinkPairingCode
import com.fluxdown.core.protocol.LinkPairingRequest
import com.fluxdown.core.protocol.LinkRules
import com.fluxdown.core.protocol.has
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxSectionFoot
import com.fluxdown.fluxui.controls.FluxSectionTitle
import com.fluxdown.fluxui.controls.FluxSegmented
import com.fluxdown.fluxui.controls.FluxTag
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.SegOption
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.data.CountdownRing
import com.fluxdown.fluxui.feedback.FluxSpinner
import com.fluxdown.fluxui.icons.FluxIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetDetent
import com.fluxdown.fluxui.overlay.FluxSheetFooter
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme

private enum class AddTab { Account, Local }

/**
 * V4 添加设备：账户自动（同账号登录即出现）/ 直连配对（免账号，局域网发现 + 配对码 + SAS 核对）。
 * 只有账号能力的主机只显示账户页；只有直连能力的主机只显示直连页；两者都有时未登录默认直连。
 * 状态机 [PairingModel] 每次打开新建；离开直连页签 / 关闭 = 停止发现、撤回配对码、放弃进行中的发起流程。
 */
@Composable
fun AddDeviceSheet(visible: Boolean, onDismiss: () -> Unit) {
    val tracker = remember { OpenTracker() }
    if (visible && !tracker.wasOpen) tracker.generation++
    tracker.wasOpen = visible
    if (tracker.generation == 0) return
    key(tracker.generation) { AddDeviceSheetImpl(visible, onDismiss) }
}

@Composable
private fun AddDeviceSheetImpl(visible: Boolean, onDismiss: () -> Unit) {
    val container = LocalAppContainer.current
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val host = hostState()
    val hostRef by container.host.collectAsStateWithLifecycle()

    val accountCap by remember { derivedStateOf { host.value.has(HostCapability.agentAuth) } }
    val linkCap by remember { derivedStateOf { host.value.has(HostCapability.agentDeviceLink) } }
    val gatewayCap by remember { derivedStateOf { host.value.has(HostCapability.agentGateway) } }
    val readOnly by remember { derivedStateOf { host.value.isReadOnly } }
    val sessionRaw by remember { derivedStateOf { host.value.sections[HostSection.agentSession] } }
    val gatewayRaw by remember { derivedStateOf { host.value.sections[HostSection.agentGateway] } }
    val requestsRaw by remember { derivedStateOf { host.value.sections[HostSection.agentLinkPairingRequests] } }
    val discoveredRaw by remember { derivedStateOf { host.value.sections[HostSection.agentLinkDiscovered] } }
    val linkDevices by remember { derivedStateOf { host.value.linkDevices } }

    val session = remember(sessionRaw, accountCap) {
        if (accountCap) AgentSessionDto.fromJson(Json.parseOrNull(sessionRaw)) else null
    }
    // 本机嵌入主机不监听任何端口（虽然它的 hello 仍带 `agent.gateway`）：别人连不上本机，不存在「本机配对码」，
    // 也没有可前往的「API 服务」设置——按无网关处理，视图只给发起方向的提示。远端主机按 `lanActive` 三态。
    val hasGateway = gatewayCap && hostRef !is HostRef.Local
    val lan = remember(gatewayRaw, hasGateway) {
        if (!hasGateway) LinkLan.Disabled else LinkLan.of(Json.parseOrNull(gatewayRaw)?.let { GatewayStatusDto.fromJson(it) })
    }
    val requestIds = remember(requestsRaw) {
        LinkPairingRequest.listFromJson(Json.parseOrNull(requestsRaw)).mapTo(HashSet()) { it.sessionId }
    }
    val entries = remember(discoveredRaw, linkDevices) {
        LinkRules.discoveredEntries(LinkDiscoveredPeer.listFromJson(Json.parseOrNull(discoveredRaw)), linkDevices)
    }

    val model = remember {
        PairingModel(
            scope = container.appScope,
            api = LinkApi(container.session),
            onPaired = { paired ->
                haptics.confirm()
                overlays.toast(context.str(R.string.localPairingPaired, "device" to paired.name), FluxToastKind.Success, FluxIcons.CircleCheck)
            },
            onFailure = { haptics.reject() },
        )
    }

    var tab by remember { mutableStateOf<AddTab?>(null) }
    val defaultTab = if (session != null || !linkCap) AddTab.Account else AddTab.Local
    val current = if (linkCap) (if (accountCap) tab ?: defaultTab else AddTab.Local) else AddTab.Account
    val local = current == AddTab.Local

    LaunchedEffect(lan) { model.syncLan(lan) }
    LaunchedEffect(visible, local) { if (visible && local) model.start() else model.stop() }
    LaunchedEffect(visible) { if (!visible) model.dismissed() }
    LaunchedEffect(requestIds) { if (local) model.onIncomingRequests(requestIds) }
    DisposableEffect(model) { onDispose { model.dismissed() } }
    // 对本机主机的发现跑在本机进程里：期间持有 MulticastLock（远端主机在它自己的网络上发现，与本机 Wi-Fi 无关）
    HoldMulticastLock(active = visible && local && hostRef is HostRef.Local)

    val step = model.step
    val title = str(R.string.addDeviceEntry)

    fun login() {
        onDismiss()
        nav.push(Route.Settings(SettingsPage.Account))
    }

    fun openLanSettings() {
        onDismiss()
        nav.push(Route.Settings(SettingsPage.Api))
    }

    FluxSheet(
        visible = visible,
        onDismissRequest = onDismiss,
        detent = FluxSheetDetent.Full,
        title = title,
        header = { FluxSheetHeader(title = title, subtitle = (hostRef as? HostRef.Remote)?.displayName, onClose = onDismiss) },
        footer = if (local && (step is PairingModel.Step.Check || step is PairingModel.Step.Finishing)) {
            { VerifyFooter(model, step) }
        } else {
            null
        },
    ) {
        Column(Modifier.padding(top = 2.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            if (accountCap && linkCap && step == PairingModel.Step.Form) {
                FluxSegmented(
                    options = listOf(
                        SegOption(AddTab.Account, str(R.string.addDeviceTabAccount)),
                        SegOption(AddTab.Local, str(R.string.addDeviceTabLocal)),
                    ),
                    selected = current,
                    onSelect = { tab = it },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            when (current) {
                AddTab.Account -> AccountContent(session, ::login)
                AddTab.Local -> LocalContent(
                    model = model,
                    entries = entries,
                    lan = lan,
                    hasGateway = hasGateway,
                    readOnly = readOnly,
                    canOpenLanSettings = hostRef is HostRef.Remote && gatewayCap,
                    onOpenLanSettings = ::openLanSettings,
                    onClose = onDismiss,
                )
            }
        }
    }
}

// ── 账户页签 ────────────────────────────────────────────────────────────

@Composable
private fun AccountContent(session: AgentSessionDto?, onLogin: () -> Unit) {
    GlassSection(footer = str(R.string.addDeviceAccountFooter)) {
        row(hasIcon = true) {
            if (session != null) {
                FluxListRow(
                    title = str(R.string.addDeviceAccountSynced, "account" to session.user.email),
                    icon = FluxIcons.CircleCheck,
                    iconTone = Tone.Mint,
                )
            } else {
                FluxListRow(
                    title = str(R.string.addDeviceLoginRequired),
                    icon = FluxIcons.UserPlus,
                    iconTone = Tone.Amber,
                )
            }
        }
    }
    if (session == null) {
        FluxButton(str(R.string.accountLogin), onLogin, variant = ButtonVariant.Primary, fullWidth = true)
    }
    AccountHint(str(R.string.addDeviceHint))
}

// ── 直连页签 ────────────────────────────────────────────────────────────

@Composable
private fun LocalContent(
    model: PairingModel,
    entries: List<LinkRules.DiscoveredEntry>,
    lan: LinkLan,
    hasGateway: Boolean,
    readOnly: Boolean,
    canOpenLanSettings: Boolean,
    onOpenLanSettings: () -> Unit,
    onClose: () -> Unit,
) {
    when (val step = model.step) {
        PairingModel.Step.Form -> FormContent(model, entries, lan, hasGateway, readOnly, canOpenLanSettings, onOpenLanSettings)
        is PairingModel.Step.Check -> VerifyContent(step.verify, finishing = false, message = model.message)
        is PairingModel.Step.Finishing -> VerifyContent(step.verify, finishing = true, message = model.message)
        is PairingModel.Step.Done -> {
            Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                FluxIcon(FluxIcons.CircleCheck, null, size = 56.dp, tint = FluxTheme.colors.mintText)
                FluxText(
                    str(R.string.localPairingPaired, "device" to step.name),
                    style = FluxTheme.type.h1,
                    color = FluxTheme.colors.ink,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            FluxButton(str(R.string.close), onClose, variant = ButtonVariant.Secondary, fullWidth = true)
        }
        PairingModel.Step.Rejected -> {
            FluxBanner(str(R.string.errReasonPairingRejected), kind = FluxBannerKind.Warn, slim = true)
            FluxButton(str(R.string.back), model::backToForm, variant = ButtonVariant.Secondary, fullWidth = true)
        }
    }
}

@Composable
private fun FormContent(
    model: PairingModel,
    entries: List<LinkRules.DiscoveredEntry>,
    lan: LinkLan,
    hasGateway: Boolean,
    readOnly: Boolean,
    canOpenLanSettings: Boolean,
    onOpenLanSettings: () -> Unit,
) {
    AccountHint(str(R.string.localPairingHint))
    if (readOnly) FluxBanner(str(R.string.localServiceDisconnected), kind = FluxBannerKind.Warn, slim = true)
    if (hasGateway) {
        // 本机局域网未生效：仍允许发起，但只能单向（对端无法回连本机）
        if (lan != LinkLan.Active) FluxBanner(str(R.string.localPairingOneWayHint), kind = FluxBannerKind.Warn, slim = true)
        OwnCodeSection(model, lan, canOpenLanSettings, onOpenLanSettings)
    } else {
        AccountHint(str(R.string.mobileAddDeviceNoGatewayHint))
    }
    DiscoveredSection(model, entries)
    ManualSection(model, readOnly)
    FluxSectionFoot(str(R.string.mobileAddDeviceForeground))
}

// ── 本机配对码 ──────────────────────────────────────────────────────────

/** 本机配对码区三态：未开启 / 未生效 → 只给原因（隐藏码 / 复制 / 换一个）；实际可达 → 码 + 倒计时 + 地址。 */
@Composable
private fun OwnCodeSection(model: PairingModel, lan: LinkLan, canOpenLanSettings: Boolean, onOpenLanSettings: () -> Unit) {
    val own = model.own
    val error = model.ownError
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        FluxSectionTitle(str(R.string.localPairingMyCodeTitle))
        GlassSection {
            custom(padded = true) {
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    when {
                        lan != LinkLan.Active -> LanHint(lan, canOpenLanSettings, onOpenLanSettings)
                        own != null -> CodeBlock(model, own)
                        model.ownLoading -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            FluxSpinner(size = 20.dp)
                            FluxText(str(R.string.mobileAddDeviceCodeLoading), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
                        }
                    }
                    if (lan == LinkLan.Active && error != null) {
                        AccountErrorLabel(pairingErrorText(error))
                        if (own == null) {
                            FluxButton(
                                str(R.string.localPairingMyCodeShow),
                                model::refreshOwnCode,
                                variant = ButtonVariant.Secondary,
                                size = ButtonSize.Sm,
                                enabled = !model.ownLoading,
                            )
                        }
                    }
                }
            }
        }
        if (lan == LinkLan.Active) FluxSectionFoot(str(R.string.localPairingMyCodeHint))
    }
}

/** 未开启 / 未生效局域网：不显示码，只给出原因；未开启且设置页可达时附「前往设置」。 */
@Composable
private fun LanHint(lan: LinkLan, canOpenLanSettings: Boolean, onOpenLanSettings: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
            FluxIcon(FluxIcons.WifiOff, null, size = 18.dp, tint = FluxTheme.colors.amberText, modifier = Modifier.padding(top = 2.dp))
            FluxText(
                str(if (lan == LinkLan.Disabled) R.string.localPairingLanDisabledHint else R.string.localPairingLanRestartHint),
                style = FluxTheme.type.sm,
                color = FluxTheme.colors.inkMuted,
                modifier = Modifier.weight(1f),
            )
        }
        if (lan == LinkLan.Disabled && canOpenLanSettings) {
            FluxButton(
                str(R.string.localPairingOpenLanSettings),
                onOpenLanSettings,
                variant = ButtonVariant.Secondary,
                size = ButtonSize.Sm,
                icon = FluxIcons.Settings,
            )
        }
    }
}

@Composable
private fun CodeBlock(model: PairingModel, own: LinkPairingCode) {
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val copied = str(R.string.localDeviceCodeCopied)
    val now = rememberNowMs(active = true)
    val seconds = LinkRules.secondsUntil(own.expiresAtUnixMs, now)
    val totalMs = (model.ownTotal * 1000).coerceAtLeast(1_000L)
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        GroupedDigits(own.code, sizeSp = 40f, label = str(R.string.localPairingMyCodeTitle))
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            CountdownRing(
                remainingFraction = { ((own.expiresAtUnixMs - now).toFloat() / totalMs).coerceIn(0f, 1f) },
                seconds = seconds.toInt(),
                // 本机配对码到期会自动换新，不是需要用户抢时间的倒计时：不做 warn 变色 / 逐秒触感
                warn = false,
                size = 36.dp,
            )
            FluxText(
                if (seconds > 0) str(R.string.localDeviceCodeRemaining, "seconds" to seconds) else str(R.string.localPairingMyCodeExpired),
                style = FluxTheme.type.sm,
                color = FluxTheme.colors.inkMuted,
            )
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally)) {
            FluxButton(
                str(R.string.localDeviceCodeCopy),
                onClick = {
                    context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText("FluxDown pairing code", own.code))
                    haptics.confirm()
                    overlays.toast(copied, FluxToastKind.Success, FluxIcons.Copy)
                },
                variant = ButtonVariant.Secondary,
                size = ButtonSize.Sm,
                icon = FluxIcons.Copy,
            )
            FluxButton(
                str(R.string.localPairingMyCodeRefresh),
                onClick = model::refreshOwnCode,
                variant = ButtonVariant.Secondary,
                size = ButtonSize.Sm,
                icon = FluxIcons.RefreshCw,
                enabled = !model.ownLoading,
            )
        }
        if (own.addresses.isEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
                FluxIcon(FluxIcons.WifiOff, null, size = 16.dp, tint = FluxTheme.colors.amberText, modifier = Modifier.padding(top = 2.dp))
                FluxText(str(R.string.localPairingNoAddress), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted, modifier = Modifier.weight(1f))
            }
        } else {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                FluxText(str(R.string.localPairingMyAddresses), style = FluxTheme.type.sm, color = FluxTheme.colors.inkFaint)
                own.addresses.forEach { address ->
                    FluxText(address, style = FluxTheme.type.monoS, color = FluxTheme.colors.ink)
                }
            }
        }
    }
}

// ── 发现 ────────────────────────────────────────────────────────────────

@Composable
private fun DiscoveredSection(model: PairingModel, entries: List<LinkRules.DiscoveredEntry>) {
    val searching = entries.isEmpty() && model.searching
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (entries.isNotEmpty()) FluxSectionTitle(str(R.string.localPairingNearbyTitle))
        GlassSection {
            if (searching) {
                custom(padded = true) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        FluxSpinner(size = 20.dp)
                        FluxText(str(R.string.localPairingDiscovering), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
                    }
                }
            } else if (entries.isEmpty()) {
                custom(padded = true) {
                    FluxText(str(R.string.localPairingNoDevices), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
                }
            }
            entries.forEach { entry ->
                row(hasIcon = true) {
                    val selected = model.address == entry.address
                    FluxListRow(
                        title = entry.name,
                        subtitle = listOfNotNull(platformLabel(entry.platform), entry.address).joinToString(" · "),
                        leading = { GlyphTile(platformIcon(entry.platform), tint = FluxTheme.colors.inkMuted) },
                        trailing = when {
                            entry.paired -> ({ FluxTag(str(R.string.localDevicePairedTag)) })
                            selected -> ({ FluxIcon(FluxIcons.Check, null, size = 20.dp, tint = FluxTheme.colors.accentHi) })
                            else -> null
                        },
                        enabled = !entry.paired,
                        selected = selected,
                        onClick = { model.address = entry.address },
                    )
                }
            }
            row(hasIcon = true) {
                FluxListRow(
                    title = str(R.string.localPairingRetryScan),
                    icon = FluxIcons.RefreshCw,
                    trailing = if (model.scanning) ({ FluxSpinner(size = 18.dp) }) else null,
                    enabled = !model.scanning,
                    onClick = model::rescan,
                )
            }
        }
        model.discoveryError?.let { AccountErrorLabel(pairingErrorText(it)) }
    }
}

// ── 手动地址 + 配对码 ───────────────────────────────────────────────────

@Composable
private fun ManualSection(model: PairingModel, readOnly: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        FluxSectionTitle(str(R.string.localPairingManualAddress))
        FluxField(
            value = model.address,
            onValueChange = { model.address = it },
            label = str(R.string.localPairingAddressLabel),
            placeholder = "192.168.1.20:17800",
            hint = str(R.string.localPairingAddressHint),
            mono = true,
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Uri,
                imeAction = ImeAction.Next,
            ),
        )
        FluxField(
            value = model.code,
            onValueChange = model::updateCode,
            label = str(R.string.localPairingCodeLabel),
            placeholder = str(R.string.localPairingCodePlaceholder),
            hint = str(R.string.localPairingCodeHint),
            mono = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { if (!readOnly) model.begin() }),
        )
        model.message?.let { AccountErrorLabel(pairingIssueText(it)) }
        FluxButton(
            str(R.string.localPairingConnect),
            onClick = model::begin,
            variant = ButtonVariant.Primary,
            fullWidth = true,
            loading = model.connecting,
            enabled = !readOnly,
        )
    }
}

// ── SAS 核对 ────────────────────────────────────────────────────────────

@Composable
internal fun VerifyContent(
    verify: PairingModel.Verify,
    finishing: Boolean,
    message: PairingIssue?,
    @StringRes hintRes: Int = R.string.localPairingSasHint,
    @StringRes waitingRes: Int = R.string.localPairingWaitingPeer,
    issueText: @Composable (PairingIssue) -> String = { pairingIssueText(it) },
) {
    Column(Modifier.fillMaxWidth().padding(vertical = 8.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
        FluxText(str(R.string.localPairingSasTitle), style = FluxTheme.type.h1, color = FluxTheme.colors.ink, modifier = Modifier.fillMaxWidth(), maxLines = 2)
        SasDigits(verify.sas)
        FluxText(
            verify.peerName,
            style = FluxTheme.type.body.copy(textAlign = TextAlign.Center),
            color = FluxTheme.colors.inkMuted,
            modifier = Modifier.fillMaxWidth(),
            maxLines = 2,
        )
        if (finishing) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FluxSpinner(size = 18.dp)
                FluxText(str(waitingRes), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
            }
        }
    }
    AccountHint(str(hintRes))
    message?.let { FluxBanner(issueText(it), kind = FluxBannerKind.Error, slim = true) }
}

/** 核对页底部操作：核对中 = 拒绝 / 确认配对；等待对方确认 = 取消（回到表单并放弃等待）。 */
@Composable
internal fun VerifyFooter(model: PairingModel, step: PairingModel.Step) {
    FluxSheetFooter {
        if (step is PairingModel.Step.Finishing) {
            FluxButton(str(R.string.cancel), model::cancelFinishing, variant = ButtonVariant.Secondary, modifier = Modifier.weight(1f))
        } else {
            FluxButton(
                str(R.string.localPairingReject),
                onClick = { model.finish(accept = false) },
                variant = ButtonVariant.Danger,
                enabled = !model.deciding,
                modifier = Modifier.weight(1f),
            )
            FluxButton(
                str(R.string.localPairingConfirm),
                onClick = { model.finish(accept = true) },
                variant = ButtonVariant.Primary,
                enabled = !model.deciding,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

// ── 文案 ────────────────────────────────────────────────────────────────

@Composable
internal fun pairingErrorText(error: HostException): String {
    val context = LocalContext.current
    return remember(error, context) { AccountText.error(context, error, AccountErrorContext.Pairing) }
}

@Composable
internal fun pairingIssueText(issue: PairingIssue): String = when (issue) {
    is PairingIssue.Input -> str(
        if (issue.error == LinkRules.InputError.AddressMissing) R.string.localPairingHostRequired else R.string.localPairingCodeIncomplete,
    )
    is PairingIssue.Host -> pairingErrorText(issue.error)
}
