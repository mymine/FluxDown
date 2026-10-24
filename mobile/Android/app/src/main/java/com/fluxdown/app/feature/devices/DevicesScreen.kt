package com.fluxdown.app.feature.devices

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.fluxdown.app.R
import com.fluxdown.app.actions.LocalTaskActions
import com.fluxdown.app.i18n.str
import com.fluxdown.app.nav.LocalNavigator
import com.fluxdown.app.nav.SheetRoute
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.app.shell.hostState
import com.fluxdown.core.format.Format
import com.fluxdown.core.model.CloudDevice
import com.fluxdown.core.model.HostRef
import com.fluxdown.core.model.LinkDevice
import com.fluxdown.core.model.TaskStatus
import com.fluxdown.core.protocol.CloudConnectionDto
import com.fluxdown.core.protocol.CloudPresence
import com.fluxdown.core.protocol.DeviceRules
import com.fluxdown.core.protocol.HostCapability
import com.fluxdown.core.protocol.HostSection
import com.fluxdown.core.protocol.Json
import com.fluxdown.core.protocol.has
import com.fluxdown.core.store.Connection
import com.fluxdown.fluxui.chrome.FluxGlassIconButton
import com.fluxdown.fluxui.chrome.FluxHeader
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxIconButton
import com.fluxdown.fluxui.controls.IconButtonSize
import com.fluxdown.fluxui.controls.FluxDivider
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxPresenceDot
import com.fluxdown.fluxui.controls.FluxSectionFoot
import com.fluxdown.fluxui.controls.FluxStatRow
import com.fluxdown.fluxui.controls.FluxTag
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.StatItem
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.feedback.FluxEmpty
import com.fluxdown.fluxui.feedback.FluxGlyph
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.FluxGlass
import com.fluxdown.fluxui.material.FluxGlassKind
import com.fluxdown.fluxui.material.fluxFlowIn
import com.fluxdown.fluxui.material.fluxGlass
import com.fluxdown.fluxui.material.rememberFlowInGate
import com.fluxdown.fluxui.overlay.LocalSwipeRevealCoordinator
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme

/**
 * V1 设备页（Dock 目的地）：当前主机卡 + 云端已信任设备（presence 未知时显示「状态未知」）+ 已配对设备
 * （直连：在线点 / 刷新状态 / 下发 / 重命名 / 解除配对）。
 * 添加设备（账户 / 直连配对）经页头「+」打开 [AddDeviceSheet]；入站配对确认是全局弹层（[InboundPairingPrompt]）。
 */
@Composable
fun DevicesScreen() {
    val container = LocalAppContainer.current
    val nav = LocalNavigator.current
    val space = FluxTheme.space
    val host = hostState()
    val hostRef by container.host.collectAsStateWithLifecycle()
    val cloud by remember { derivedStateOf { host.value.cloudDevices } }
    val link by remember { derivedStateOf { host.value.linkDevices } }
    val facts = remember { derivedStateOf { host.value.toFacts() } }
    val live by remember { derivedStateOf { host.value.connection == Connection.Live } }
    val connectionRaw by remember { derivedStateOf { host.value.sections[HostSection.agentCloudConnection] } }
    // 云端在线状态是否可信（与「下载到」候选同一判定）；不可信时云设备显示「状态未知」，不当离线
    val presenceKnown = remember(connectionRaw, live) {
        CloudPresence.isKnown(CloudConnectionDto.fromJson(Json.parseOrNull(connectionRaw)), live)
    }

    val cloudSorted = remember(cloud, presenceKnown) { DeviceRules.sorted(cloud, presenceKnown) }
    val linkSorted = remember(link) {
        link.sortedWith(
            compareBy<LinkDevice> { it.revoked }
                .thenByDescending { it.online }
                .thenBy(String.CASE_INSENSITIVE_ORDER) { it.name },
        )
    }
    val sameAccount = remember(cloud, link) { DeviceRules.attachedLinkFingerprints(cloud, link) }
    // 与「下载到」同一份候选；已挂接的直连记录经 resolveTarget 解析为云条目键，revoked / 不在候选里的没有下发入口
    val targets = remember(cloud, link, presenceKnown, live) { DeviceRules.dispatchTargets(cloud, link, presenceKnown, live) }
    fun dispatchTargetId(d: LinkDevice): String? =
        if (d.revoked) null else DeviceRules.resolveTarget("link:${d.fingerprint}", targets)?.id
    val accountCap by remember { derivedStateOf { host.value.has(HostCapability.agentAuth) } }
    val linkCap by remember { derivedStateOf { host.value.has(HostCapability.agentDeviceLink) } }
    val readOnly by remember { derivedStateOf { host.value.isReadOnly } }
    val links = rememberLinkedDeviceActions()
    var renaming by remember { mutableStateOf<LinkDevice?>(null) }

    val listState = rememberLazyListState()
    val swipe = LocalSwipeRevealCoordinator.current
    val gate = rememberFlowInGate()
    val margin = space.screenMargin
    val topInset = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()

    LazyColumn(
        state = listState,
        modifier = Modifier
            .fillMaxSize()
            .then(if (swipe != null) Modifier.nestedScroll(swipe.nestedScrollConnection) else Modifier),
        contentPadding = PaddingValues(
            top = topInset,
            bottom = space.dockClearance + WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding(),
        ),
    ) {
        item(key = "header") {
            FluxHeader(
                title = str(R.string.mobileNavDevices),
                modifier = Modifier.padding(horizontal = margin).fluxFlowIn(0, gate, "header"),
                actions = {
                    if (accountCap || linkCap) {
                        FluxGlassIconButton(
                            icon = FluxIcons.Plus,
                            contentDescription = str(R.string.addDeviceEntry),
                            onClick = { nav.openSheet(SheetRoute.AddDevice) },
                        )
                    }
                },
            )
        }
        item(key = "host") {
            HostCard(
                hostRef = hostRef,
                facts = facts.value,
                onSwitch = { nav.openSheet(SheetRoute.HostSwitch) },
                onRepair = { (hostRef as? HostRef.Remote)?.let { nav.openSheet(SheetRoute.AddHost(repair = it)) } },
                modifier = Modifier.padding(horizontal = margin, vertical = 6.dp).fluxFlowIn(1, gate, "host"),
            )
        }
        if (cloudSorted.isNotEmpty()) {
            item(key = "cloud") {
                GlassSection(
                    modifier = Modifier.padding(horizontal = margin).fluxFlowIn(2, gate, "cloud"),
                    title = str(R.string.accountDevicesTitle),
                    footer = str(R.string.accountDevicesDesc),
                ) {
                    cloudSorted.forEach { d ->
                        row(hasIcon = true) {
                            DeviceRow(d.name, d.platform, if (presenceKnown) d.isOnline else null, d.appVersion, d.isCurrent)
                        }
                    }
                }
            }
        }
        if (linkCap) {
            item(key = "link") {
                GlassSection(
                    modifier = Modifier.padding(horizontal = margin).fluxFlowIn(3, gate, "link"),
                    title = str(R.string.linkedDevicesTitle),
                    footer = str(R.string.linkedDevicesDesc),
                    action = {
                        FluxButton(
                            text = str(R.string.linkedDeviceRefresh),
                            onClick = links::refresh,
                            variant = ButtonVariant.Ghost,
                            size = ButtonSize.Xs,
                            icon = FluxIcons.RefreshCw,
                            loading = links.refreshing,
                            enabled = !readOnly,
                        )
                    },
                ) {
                    if (linkSorted.isEmpty()) {
                        custom(padded = true) {
                            FluxText(str(R.string.linkedDevicesEmpty), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
                        }
                    }
                    linkSorted.forEach { d ->
                        row(hasIcon = true) {
                            LinkedDeviceRow(
                                device = d,
                                sameAccount = d.fingerprint in sameAccount,
                                actionsEnabled = !readOnly,
                                onDispatch = dispatchTargetId(d)?.let { id -> { nav.openSheet(SheetRoute.NewDownload(targetId = id)) } },
                                onRename = { renaming = d },
                                onRemove = { links.confirmRemove(d) },
                            )
                        }
                    }
                }
            }
        }
        if (cloudSorted.isEmpty() && !linkCap) {
            item(key = "empty") {
                Box(Modifier.fillMaxWidth().padding(top = 24.dp), contentAlignment = Alignment.Center) {
                    FluxEmpty(glyph = FluxGlyph.Device, title = str(R.string.mobileDevicesEmptyTitle))
                }
            }
        }
        item(key = "footnote") {
            FluxSectionFoot(
                str(R.string.mobileDevicesFootnote),
                Modifier.padding(horizontal = margin + 8.dp, vertical = 8.dp),
            )
        }
    }

    LinkRenameSheet(
        target = renaming,
        readOnly = readOnly,
        onDismiss = { renaming = null },
    )
}

// ── 主机事实（派生，结构相等才触发重组）──────────────────────────────────

@Immutable
private data class HostFacts(
    val connection: Connection,
    val version: String?,
    val diskFree: Long?,
    val active: Int,
    val waiting: Int,
    val completed: Int,
    val paused: Int,
    val failed: Int,
)

private fun com.fluxdown.core.store.HostState.toFacts(): HostFacts {
    var completed = 0
    var paused = 0
    var failed = 0
    for (t in tasks) {
        when (t.status) {
            TaskStatus.Completed -> completed++
            TaskStatus.Paused -> paused++
            TaskStatus.Failed -> failed++
            else -> Unit
        }
    }
    return HostFacts(
        connection = connection,
        version = info?.serviceVersion,
        diskFree = stats.diskFreeBytes,
        active = stats.activeTasks,
        waiting = stats.pendingTasks,
        completed = completed,
        paused = paused,
        failed = failed,
    )
}

// ── 当前主机卡 ──────────────────────────────────────────────────────────

@Composable
private fun HostCard(hostRef: HostRef, facts: HostFacts, onSwitch: () -> Unit, onRepair: () -> Unit, modifier: Modifier = Modifier) {
    val c = FluxTheme.colors
    val t = FluxTheme.type
    val actions = LocalTaskActions.current

    val (connText, connTone) = when (facts.connection) {
        Connection.Live -> str(R.string.mobileHostConnLive) to Tone.Mint
        Connection.Connecting -> str(R.string.mobileHostConnConnecting) to Tone.Amber
        Connection.Stale -> str(R.string.mobileHostConnStale) to Tone.Amber
        is Connection.Failed -> str(R.string.mobileHostConnFailed) to Tone.Coral
    }
    val failure = (facts.connection as? Connection.Failed)?.error
    val needsRepair = failure != null && hostRef.needsRepair(failure)
    val connError = when {
        needsRepair -> str(R.string.mobileHostRepairNeeded)
        failure != null -> actions.errorText(failure)
        else -> null
    }

    Column(
        modifier
            .fillMaxWidth()
            .fluxGlass(FluxGlass.G3, FluxTheme.shapes.card, kind = FluxGlassKind.Flat),
    ) {
        Column(Modifier.padding(start = 20.dp, end = 12.dp, top = 18.dp, bottom = 14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                FluxPresenceDot(tone = connTone, size = 9.dp)
                FluxText(hostRef.localizedName(), Modifier.weight(1f), style = t.h1, color = c.ink, maxLines = 2)
                FluxButton(
                    text = str(R.string.mobileDevicesSwitchHost),
                    onClick = onSwitch,
                    variant = ButtonVariant.Ghost,
                    size = ButtonSize.Xs,
                    trailingIcon = FluxIcons.ChevronDown,
                )
            }
            FluxText(hostRef.localizedSubtitle(), style = t.sm, color = c.inkMuted, maxLines = 2)
            FluxText(
                connText,
                style = t.weight(t.sm, 500),
                color = when (connTone) {
                    Tone.Mint -> c.mintText
                    Tone.Amber -> c.amberText
                    else -> c.coralText
                },
            )
            if (connError != null) FluxText(connError, style = t.sm, color = c.coralText)
            if (needsRepair) {
                FluxButton(
                    text = str(R.string.mobileHostRepair),
                    onClick = onRepair,
                    variant = ButtonVariant.Secondary,
                    size = ButtonSize.Sm,
                    icon = FluxIcons.Link,
                )
            }
        }
        FluxDivider()
        val disk = facts.diskFree?.let { Format.bytes(it) }
        FluxStatRow(
            listOf(
                StatItem(disk?.value ?: "—", str(R.string.mobileDevicesStatDisk), disk?.unit),
                StatItem(facts.version ?: "—", str(R.string.mobileDevicesStatVersion)),
                StatItem(facts.active.toString(), str(R.string.statusDownloading), tone = if (facts.active > 0) Tone.Accent else Tone.Neutral),
            ),
        )
        if (facts.waiting + facts.completed + facts.paused + facts.failed > 0) {
            FluxDivider()
            Row(
                Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (facts.waiting > 0) FluxTag("${str(R.string.statusPending)} ${facts.waiting}")
                if (facts.paused > 0) FluxTag("${str(R.string.statusPaused)} ${facts.paused}")
                if (facts.completed > 0) FluxTag("${str(R.string.statusCompleted)} ${facts.completed}", tone = Tone.Mint)
                if (facts.failed > 0) FluxTag("${str(R.string.tabError)} ${facts.failed}", tone = Tone.Coral)
            }
        }
    }
}

// ── 设备行 ──────────────────────────────────────────────────────────────

@Composable
private fun DeviceRow(
    name: String,
    platform: String?,
    /** null = 云端 presence 不可信（状态未知，不当离线）。 */
    online: Boolean?,
    version: String?,
    current: Boolean,
    subtitleOverride: String? = null,
    /** 附在状态行末尾的说明（如「可控制本机」）。 */
    note: String? = null,
    trailing: (@Composable () -> Unit)? = null,
) {
    val c = FluxTheme.colors
    val subtitle = subtitleOverride ?: listOfNotNull(
        str(
            when (online) {
                true -> R.string.deviceOnline
                false -> R.string.deviceOffline
                null -> R.string.devicePresenceUnknown
            },
        ),
        platformLabel(platform),
        version?.takeIf { it.isNotBlank() }?.let { "v$it" },
        note,
    ).joinToString(" · ")
    FluxListRow(
        title = name,
        subtitle = subtitle,
        leading = {
            Box(Modifier.size(44.dp)) {
                GlyphTile(
                    icon = platformIcon(platform),
                    tint = if (online == true) c.ink else c.inkFaint,
                    modifier = Modifier.align(Alignment.TopStart),
                )
                PresenceMark(online == true, Modifier.align(Alignment.BottomEnd))
            }
        },
        trailing = trailing ?: if (current) ({ FluxTag(str(R.string.thisDevice), tone = Tone.Accent) }) else null,
    )
}

/**
 * 已配对设备行（直连）：在线点、生效名（备注名优先）、平台 / 版本；对方已解除 = revoked 标签 + 只给「解除配对」；
 * 挂接到同账号云设备 = 「同一账号」标签；操作：下发（[onDispatch] 非空才显示）、重命名、解除配对。
 */
@Composable
private fun LinkedDeviceRow(
    device: LinkDevice,
    sameAccount: Boolean,
    actionsEnabled: Boolean,
    onDispatch: (() -> Unit)?,
    onRename: () -> Unit,
    onRemove: () -> Unit,
) {
    DeviceRow(
        name = device.name,
        platform = device.platform,
        online = device.online && !device.revoked,
        version = device.appVersion,
        current = false,
        subtitleOverride = if (device.revoked) str(R.string.linkedDeviceRevokedDesc) else null,
        note = if (device.grantsControl && !device.revoked) str(R.string.linkControlGrantedTag) else null,
        trailing = {
            Row(horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically) {
                when {
                    device.revoked -> FluxTag(str(R.string.linkedDeviceRevokedTag), tone = Tone.Coral)
                    sameAccount -> FluxTag(str(R.string.linkedDeviceSameAccountTag), tone = Tone.Accent)
                }
                if (!device.revoked) {
                    if (onDispatch != null) {
                        FluxIconButton(
                            FluxIcons.Send,
                            str(R.string.deviceDownloadToThis),
                            onClick = onDispatch,
                            size = IconButtonSize.Sm,
                            glass = false,
                            enabled = actionsEnabled,
                        )
                    }
                    FluxIconButton(
                        FluxIcons.Pen,
                        str(R.string.linkedDeviceRename),
                        onClick = onRename,
                        size = IconButtonSize.Sm,
                        glass = false,
                        enabled = actionsEnabled,
                    )
                }
                FluxIconButton(
                    FluxIcons.Trash2,
                    str(R.string.linkedDeviceRemove),
                    onClick = onRemove,
                    size = IconButtonSize.Sm,
                    glass = false,
                    danger = true,
                    enabled = actionsEnabled,
                )
            }
        },
    )
}

/** 在线 = mint 实心 + 辉光；离线 = 1.5dp 空心环（永远伴随文字状态）。 */
@Composable
private fun PresenceMark(online: Boolean, modifier: Modifier = Modifier) {
    val c = FluxTheme.colors
    Box(modifier.size(14.dp).background(c.canvas, CircleShape), contentAlignment = Alignment.Center) {
        if (online) {
            FluxPresenceDot(size = 8.dp)
        } else {
            Box(Modifier.size(8.dp).border(1.5.dp, c.inkFaint, CircleShape))
        }
    }
}
