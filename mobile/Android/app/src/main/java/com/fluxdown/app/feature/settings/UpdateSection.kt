package com.fluxdown.app.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.fluxdown.app.R
import com.fluxdown.app.data.DeviceSettings
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.app.update.UpdateCheckJobService
import com.fluxdown.app.update.render
import com.fluxdown.core.format.Format
import com.fluxdown.core.update.AppUpdateStatus
import com.fluxdown.core.update.UpdatePhase
import com.fluxdown.core.update.UpdatePolicy
import com.fluxdown.core.update.UpdateView
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxProgressLine
import com.fluxdown.fluxui.controls.FluxSwitchRow
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.SelectOption
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.markdown.FluxMarkdown
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme

/** 更新说明折叠时显示 1 条、展开最多显示的条数。 */
private const val NOTES_EXPANDED_MAX = 5

/**
 * 关于页「软件更新」分组：状态 / 下载进度 / 主操作 / 手动说明 / 更新说明 / 自动检查 · 仅非计量网络自动下载 · 渠道。
 * 文案与可用操作由 `:core` 的 [UpdateView] 判定（镜像桌面 `update_view.rs`）；状态机在 Rust，这里只呈现与转发点按。
 * 与主机无关：远端主机模式下同样更新本机 App。
 */
@Composable
internal fun UpdateSection(ctx: SettingsCtx, openPicker: (PickerSpec) -> Unit) {
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val controller = LocalAppContainer.current.updates
    val settings = remember(context) { DeviceSettings.of(context) }
    val status by controller.status.collectAsStateWithLifecycle()
    val installerClosed by controller.installerClosed.collectAsStateWithLifecycle()
    // 安装程序界面已返回而应用未被替换：Rust 仍在 Installing，这里换成可重试的说明与操作
    val awaitingRetry = installerClosed && status.phase == UpdatePhase.Installing
    val c = FluxTheme.colors
    val t = FluxTheme.type
    var notesExpanded by rememberSaveable { mutableStateOf(false) }

    val statusText = if (awaitingRetry) str(R.string.mobileUpdateInstallerClosed) else UpdateView.statusLine(status)?.render()
    val manualText = UpdateView.manualLine(status)?.render()
    val manualUrl = UpdateView.manualUrl(status)
    val noBrowser = str(R.string.mobileNoBrowser)
    val channelName = str(R.string.updateChannel)
    val channelOptions = listOf(
        SelectOption(UpdatePolicy.CHANNEL_STABLE, str(R.string.updateChannelStable)),
        SelectOption(UpdatePolicy.CHANNEL_FRONTIER, str(R.string.updateChannelFrontier)),
    )
    val channel = controller.channel
    val sizeText = Format.bytes(status.assetSize).toString()
    val meteredTitle = str(R.string.mobileUpdateMeteredTitle)
    val meteredMessage = str(R.string.mobileUpdateMeteredMessage, "size" to sizeText)
    val downloadLabel = str(R.string.downloadUpdate, "size" to sizeText)
    val cancelLabel = str(R.string.cancel)
    // 更新说明按界面语言取区块（同 GPUI 关于页；Android 没有应用内语言开关，界面语言即当前配置的
    // locale：系统语言或系统设置里的应用语言），并去掉正文开头与「v版本 · 发布日期」标题重复的版本标题。
    val locale = LocalConfiguration.current.locales[0].toLanguageTag()
    var notesOverflow by remember(status.notes, locale) { mutableStateOf(false) }
    val notes = remember(status.notes, notesExpanded, locale) {
        status.notes.take(if (notesExpanded) NOTES_EXPANDED_MAX else 1)
            .map { it.copy(body = UpdateView.stripVersionHeading(UpdateView.localizedReleaseBody(it.body, locale), it.version)) }
    }

    GlassSection(title = str(R.string.softwareUpdate)) {
        row {
            SettingRowBox(ctx, "about.update", null) {
                FluxListRow(
                    title = statusText ?: str(R.string.checkUpdate),
                    subtitle = if (statusText == null) str(R.string.checkUpdateDesc) else null,
                )
            }
        }
        if (status.phase == UpdatePhase.Downloading) {
            custom(padded = true) { FluxProgressLine(UpdateView.downloadPercent(status) / 100f) }
        }
        if (manualText != null) {
            custom(padded = true) { FluxText(manualText, style = t.sm, color = c.inkMuted) }
        }
        custom(padded = true) {
            UpdateActions(
                status = status,
                awaitingRetry = awaitingRetry,
                manualUrl = manualUrl,
                onCheck = controller::check,
                onInstall = {
                    // 计量网络下手动下载前确认流量（包已就绪则直接进入安装）。
                    if (status.phase != UpdatePhase.Ready && status.assetSize > 0 && !controller.isUnmetered()) {
                        overlays.showDialog(
                            FluxDialogSpec(
                                title = meteredTitle,
                                message = meteredMessage,
                                icon = FluxIcons.TriangleAlert,
                                buttons = listOf(
                                    FluxDialogButton(cancelLabel, FluxDialogButtonStyle.Secondary),
                                    FluxDialogButton(downloadLabel, FluxDialogButtonStyle.Primary) { controller.install() },
                                ),
                            ),
                        )
                    } else {
                        controller.install()
                    }
                },
                onReinstall = controller::reinstall,
                onCancel = controller::cancel,
                onOpenSite = { url -> openLink(context, overlays, url, noBrowser) },
            )
        }
        if (status.hasUpdate && notes.isNotEmpty()) {
            custom(padded = true) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    FluxText(str(R.string.mobileUpdateNotesTitle), style = t.sm, color = c.inkMuted)
                    for (note in notes) {
                        FluxText(
                            listOfNotNull("v${note.version}", UpdateView.releaseDate(note.publishedAt)).joinToString(" · "),
                            style = t.mono,
                            color = c.ink,
                        )
                        if (note.body.isNotBlank()) {
                            FluxMarkdown(
                                markdown = note.body.trim(),
                                maxLines = if (notesExpanded) Int.MAX_VALUE else 6,
                                onOpenLink = { url -> openLink(context, overlays, url, noBrowser) },
                                onTextLayout = { layout ->
                                    if (!notesExpanded) notesOverflow = layout.hasVisualOverflow
                                },
                            )
                        }
                    }
                    if (status.notes.size > 1 || notesOverflow || notesExpanded) {
                        FluxButton(
                            text = str(if (notesExpanded) R.string.mobileUpdateNotesCollapse else R.string.mobileUpdateNotesExpand),
                            onClick = { notesExpanded = !notesExpanded },
                            variant = ButtonVariant.Ghost,
                            size = ButtonSize.Xs,
                        )
                    }
                }
            }
        }
        row {
            SettingRowBox(ctx, "about.update.auto", null) {
                FluxSwitchRow(
                    title = str(R.string.autoCheckUpdate),
                    subtitle = str(R.string.autoCheckUpdateDesc),
                    checked = settings.updateAutoCheck,
                    onCheckedChange = { on ->
                        settings.updateUpdateAutoCheck(on)
                        UpdateCheckJobService.sync(context, on)
                    },
                )
            }
        }
        row {
            SettingRowBox(ctx, "about.update.wifiOnly", null) {
                FluxSwitchRow(
                    title = str(R.string.mobileUpdateWifiOnly),
                    subtitle = str(R.string.mobileUpdateWifiOnlyDesc),
                    checked = settings.updateWifiOnly,
                    enabled = settings.updateAutoCheck,
                    onCheckedChange = settings::updateUpdateWifiOnly,
                )
            }
        }
        row {
            SettingRowBox(ctx, "about.update.channel", null) {
                FluxListRow(
                    title = channelName,
                    subtitle = str(R.string.updateChannelDesc),
                    value = channelOptions.firstOrNull { it.value == channel }?.label ?: channel,
                    chevron = true,
                    onClick = { openPicker(PickerSpec(channelName, channelOptions, channel) { controller.setChannel(it) }) },
                )
            }
        }
    }
}

/** 主操作区：按 [UpdateView] 的判定显示 更新 / 安装 / 取消 / 前往官网 / 检查；安装程序返回未完成时显示 重新安装 / 换用其他安装程序。 */
@Composable
private fun UpdateActions(
    status: AppUpdateStatus,
    awaitingRetry: Boolean,
    manualUrl: String?,
    onCheck: () -> Unit,
    onInstall: () -> Unit,
    onReinstall: (chooser: Boolean) -> Unit,
    onCancel: () -> Unit,
    onOpenSite: (String) -> Unit,
) {
    val canInstall = UpdateView.canInstall(status)
    val checking = status.phase == UpdatePhase.Checking
    val canCheck = status.phase in setOf(UpdatePhase.Idle, UpdatePhase.UpToDate, UpdatePhase.Available, UpdatePhase.Failed)
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (awaitingRetry) {
            FluxButton(
                text = str(R.string.mobileUpdateInstallButton),
                onClick = { onReinstall(false) },
                variant = ButtonVariant.Primary,
                icon = FluxIcons.Download,
                fullWidth = true,
            )
            FluxButton(text = str(R.string.mobileUpdateChooseInstaller), onClick = { onReinstall(true) }, fullWidth = true)
        }
        if (canInstall) {
            FluxButton(
                text = str(if (status.phase == UpdatePhase.Ready) R.string.mobileUpdateInstallButton else R.string.updateNow),
                onClick = onInstall,
                variant = ButtonVariant.Primary,
                icon = FluxIcons.Download,
                fullWidth = true,
            )
        }
        if (manualUrl != null) {
            FluxButton(
                text = str(R.string.updateFailedOpenSite),
                onClick = { onOpenSite(manualUrl) },
                variant = ButtonVariant.Primary,
                icon = FluxIcons.ExternalLink,
                fullWidth = true,
            )
        }
        if (UpdateView.canCancel(status)) {
            FluxButton(text = str(R.string.cancel), onClick = onCancel, fullWidth = true)
        }
        if (checking || canCheck) {
            val label = if (status.phase == UpdatePhase.Idle || status.phase == UpdatePhase.UpToDate) R.string.checkUpdate else R.string.recheck
            FluxButton(text = str(label), onClick = onCheck, loading = checking, enabled = canCheck, fullWidth = true)
        }
    }
}
