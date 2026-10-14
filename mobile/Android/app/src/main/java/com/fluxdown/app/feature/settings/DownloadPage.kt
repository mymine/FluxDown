package com.fluxdown.app.feature.settings

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.StringRes
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.fluxdown.app.R
import com.fluxdown.app.actions.LocalTaskActions
import com.fluxdown.app.i18n.fill
import com.fluxdown.app.i18n.str
import com.fluxdown.app.nav.LocalNavigator
import com.fluxdown.app.nav.SettingsPage
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.app.shell.hostState
import com.fluxdown.app.ui.defaultLocalSaveDir
import com.fluxdown.app.ui.isLocalDirWritable
import com.fluxdown.app.ui.label
import com.fluxdown.app.ui.canRequestStorageAccess
import com.fluxdown.app.ui.publicDownloadSaveDir
import com.fluxdown.app.ui.rememberStorageAccessPrompt
import com.fluxdown.app.ui.treeUriToPath
import com.fluxdown.core.host.HostException
import com.fluxdown.core.protocol.ConnPolicySummary
import com.fluxdown.core.protocol.HostMethod
import com.fluxdown.core.protocol.SettingsForm
import com.fluxdown.core.protocol.SettingsSaveDirectory
import com.fluxdown.core.protocol.callJson
import com.fluxdown.core.store.Connection
import com.fluxdown.fluxui.chrome.FluxPill
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxActionRow
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxFieldAction
import com.fluxdown.fluxui.controls.FluxFieldRow
import com.fluxdown.fluxui.controls.FluxIconButton
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.GlassSectionScope
import com.fluxdown.fluxui.controls.IconButtonSize
import com.fluxdown.fluxui.controls.SelectOption
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.feedback.FluxSpinner
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.rememberFlowInGate
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.FluxPortal
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** 全局 User-Agent 预设（与 PC 端同一份取值）。 */
private class UaPreset(val id: String, @StringRes val label: Int, val ua: String)

private val UaPresets = listOf(
    UaPreset(
        "chrome", R.string.userAgentPresetChrome,
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36",
    ),
    UaPreset(
        "firefox", R.string.userAgentPresetFirefox,
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:147.0) Gecko/20100101 Firefox/147.0",
    ),
    UaPreset(
        "edge", R.string.userAgentPresetEdge,
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 Edg/145.0.3800.70",
    ),
    UaPreset(
        "safari", R.string.userAgentPresetSafari,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3.1 Safari/605.1.15",
    ),
)

private const val UA_DEFAULT = "default"
private const val UA_CUSTOM = "custom"

// ───────────────────────────── 行目录（页面渲染与搜索索引共用） ─────────────────────────────

/** 下载页分组（顺序即页面顺序；[key] = LazyColumn 项 key = 搜索条目 itemKey）。 */
private enum class DlSection(val key: String, @StringRes val title: Int) {
    Save("save", R.string.settingsGroupSaveLocation),
    Behavior("behavior", R.string.settingsGroupBehavior),
    Connection("connection", R.string.settingsGroupConnection),
    Retry("retry", R.string.settingsGroupRetry),
    Advanced("advanced", R.string.settingsGroupAdvanced),
}

/** 一行：iOS `SettingsDownloadRow` 的逐项对应（id / 配置键 / 文案 / 可见性）。 */
private class DlRow(
    val id: String,
    val key: String,
    val section: DlSection,
    @StringRes val title: Int,
    @StringRes val desc: Int,
    val visible: (form: SettingsForm, isLocalHost: Boolean) -> Boolean = { f, _ -> f.has(key) },
)

private val DlRows: List<DlRow> = listOf(
    DlRow("download.saveDir", "default_save_dir", DlSection.Save, R.string.defaultSaveDir, R.string.defaultSaveDirDesc),
    DlRow(
        "download.rememberLastSaveDir", "download.remember_last_save_dir", DlSection.Save,
        R.string.rememberLastSaveDir, R.string.rememberLastSaveDirDesc,
    ),
    DlRow(
        "download.silentDownload", "download.silent_download", DlSection.Behavior,
        R.string.silentDownload, R.string.silentDownloadDesc,
    ),
    DlRow(
        "download.silentSkipSelection", "download.silent_skip_selection", DlSection.Behavior,
        R.string.silentSkipSelection, R.string.silentSkipSelectionDesc,
        visible = { f, _ -> f.has("download.silent_skip_selection") && f.bool("download.silent_download") },
    ),
    DlRow("download.useServerTime", "use_server_time", DlSection.Behavior, R.string.useServerTime, R.string.useServerTimeDesc),
    DlRow(
        "download.fileExistsBehavior", "file_exists_behavior", DlSection.Behavior,
        R.string.fileExistsBehavior, R.string.fileExistsBehaviorDesc,
    ),
    DlRow(
        "download.fileMissingAction", "file_missing_action", DlSection.Behavior,
        R.string.fileMissingAction, R.string.fileMissingActionDesc,
    ),
    DlRow(
        "download.idleFileScan", "idle_file_scan", DlSection.Behavior, R.string.idleFileScan, R.string.idleFileScanDesc,
        // 本机不为刷新 UI 周期轮询（回前台节流重扫代替）；连接 NAS 时保留。
        visible = { f, local -> f.has("idle_file_scan") && !local },
    ),
    DlRow(
        "download.defaultQueue", "default_queue_id", DlSection.Behavior,
        R.string.defaultQueueSetting, R.string.defaultQueueSettingDesc,
    ),
    DlRow("download.dedupSameUrl", "dedup_same_url", DlSection.Behavior, R.string.mobileDedupSameUrl, R.string.mobileDedupSameUrlDesc),
    DlRow("download.defaultSegments", "default_segments", DlSection.Connection, R.string.defaultThreads, R.string.defaultThreadsDesc),
    DlRow(
        "download.autoMaxConnections", "auto_max_connections", DlSection.Connection,
        R.string.autoMaxConnections, R.string.autoMaxConnectionsDesc,
        visible = { f, _ -> f.has("auto_max_connections") && f.has("default_segments") && f.int("default_segments", 0) == 0 },
    ),
    DlRow("download.cdnMulti", "cdn_multi_enabled", DlSection.Connection, R.string.cdnMultiEnabled, R.string.cdnMultiEnabledDesc),
    DlRow(
        "download.cdnMaxNodes", "cdn_max_nodes", DlSection.Connection, R.string.cdnMaxNodes, R.string.cdnMaxNodesDesc,
        visible = { f, _ -> f.has("cdn_max_nodes") && f.has("cdn_multi_enabled") && f.bool("cdn_multi_enabled") },
    ),
    DlRow("download.multiNic", "multi_nic_enabled", DlSection.Connection, R.string.multiNicEnabled, R.string.multiNicEnabledDesc),
    DlRow("download.maxConcurrent", "max_concurrent_tasks", DlSection.Connection, R.string.maxConcurrent, R.string.maxConcurrentDesc),
    DlRow("download.speedLimit", "speed_limit_bytes", DlSection.Connection, R.string.speedLimit, R.string.speedLimitDesc),
    DlRow("download.uploadLimit", "upload_limit_bytes", DlSection.Connection, R.string.uploadLimit, R.string.uploadLimitDesc),
    DlRow("download.autoRetryCount", "max_auto_retries", DlSection.Retry, R.string.autoRetryCount, R.string.autoRetryCountDesc),
    DlRow("download.autoRetryDelay", "auto_retry_delay_secs", DlSection.Retry, R.string.autoRetryDelay, R.string.autoRetryDelayDesc),
    DlRow("download.autoResumeOnStart", "auto_resume_on_start", DlSection.Retry, R.string.autoResumeOnStart, R.string.autoResumeOnStartDesc),
    DlRow("download.userAgent", "global_user_agent", DlSection.Advanced, R.string.userAgent, R.string.userAgentDesc),
)

private fun visibleDlRows(form: SettingsForm, isLocalHost: Boolean): List<DlRow> =
    DlRows.filter { it.visible(form, isLocalHost) }

/** 设置搜索条目：与页面渲染共用 [DlRow.visible]。 */
internal fun downloadSearchEntries(ctx: SettingsSearchContext): List<SettingsEntry> =
    visibleDlRows(ctx.form, ctx.isLocalHost).map { row ->
        ctx.entry(
            row.id, SettingsPage.Download, row.section.key, row.title, row.desc,
            ctx.crumb(R.string.settingsCatDownload, row.section.title), FluxIcons.Download,
        )
    }

// ───────────────────────────── 页面 ─────────────────────────────

/** 「已学习的服务器策略」：条数（null = 加载中 / 失败）。 */
private class ConnPolicyUi(val count: Long?, val failed: Boolean, val clearing: Boolean)

/**
 * S5 · 下载设置。字段经 [ConfigEditor] 读写（本地乐观 → 250ms 防抖 → 按目录路由；冲突重放、失败回滚 + 行内说明）。
 * 只读（断线）时全部控件置灰，点按给出 REJECT 触感。
 */
@Composable
internal fun DownloadPage() {
    val nav = LocalNavigator.current
    val container = LocalAppContainer.current
    val overlays = LocalFluxOverlays.current
    val actions = LocalTaskActions.current
    val haptics = FluxTheme.haptics
    val motion = FluxTheme.motion
    val scope = rememberCoroutineScope()
    val hostState = hostState()
    val queues by remember { derivedStateOf { hostState.value.queues } }
    val live by remember { derivedStateOf { hostState.value.connection == Connection.Live } }
    val hostRef by container.host.collectAsStateWithLifecycle()

    var picker by remember { mutableStateOf<PickerSpec?>(null) }
    var showNicHelp by remember { mutableStateOf(false) }
    var showRemotePicker by remember { mutableStateOf(false) }
    var uaForceCustom by rememberSaveable { mutableStateOf(false) }
    var connCount by remember { mutableStateOf<Long?>(null) }
    var connFailed by remember { mutableStateOf(false) }
    var connClearing by remember { mutableStateOf(false) }

    val ctx = rememberSettingsCtx { picker = it }
    val gate = rememberFlowInGate()
    val animateSize = Modifier.animateContentSize(motion.of(motion.fluid))
    val readOnly = ctx.readOnly
    val loaded = ctx.form.isLoaded

    // 已学习的服务器策略：主机切换 / 重连后重新读取
    LaunchedEffect(hostRef, live, loaded) {
        if (!loaded || !live) return@LaunchedEffect
        try {
            val summary = ConnPolicySummary.fromJson(container.session.callJson(HostMethod.daemonConfigConnPolicy))
            connCount = summary.domainCount
            connFailed = false
        } catch (e: CancellationException) {
            throw e
        } catch (e: HostException) {
            connFailed = true
        }
    }

    val clearTitle = str(R.string.connPolicyCacheClear)
    val clearMessage = str(R.string.connPolicyCacheDesc)
    val clearedText = str(R.string.connPolicyCacheCleared)
    val cancel = str(R.string.cancel)
    val onClearConnPolicy: () -> Unit = {
        overlays.showDialog(
            FluxDialogSpec(
                title = clearTitle,
                message = clearMessage,
                icon = FluxIcons.Trash2,
                buttons = listOf(
                    FluxDialogButton(cancel, FluxDialogButtonStyle.Secondary),
                    FluxDialogButton(clearTitle, FluxDialogButtonStyle.Destructive) {
                        connClearing = true
                        scope.launch {
                            try {
                                val summary = ConnPolicySummary.fromJson(
                                    container.session.callJson(HostMethod.daemonConfigClearConnPolicy),
                                )
                                connCount = summary.domainCount
                                connFailed = false
                                haptics.confirm()
                                overlays.toast(clearedText, FluxToastKind.Success)
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: HostException) {
                                overlays.toast(actions.errorText(e), FluxToastKind.Error)
                            } finally {
                                connClearing = false
                            }
                        }
                    },
                ),
            ),
        )
    }

    // 多 CDN：开启时若代理启用，先确认「关闭代理并开启」（代理下该功能不生效）
    val cdnTitle = str(R.string.cdnMultiProxyConfirmTitle)
    val cdnSystem = str(R.string.cdnMultiProxyConfirmDescSystem)
    val cdnManual = str(R.string.cdnMultiProxyConfirmDescManual)
    val cdnDisable = str(R.string.cdnMultiProxyConfirmDisable)
    val onCdnToggle: (Boolean) -> Unit = { on ->
        val proxy = ctx.form.value("proxy_mode")
        if (on && (proxy == "system" || proxy == "manual")) {
            overlays.showDialog(
                FluxDialogSpec(
                    title = cdnTitle,
                    message = if (proxy == "system") cdnSystem else cdnManual,
                    icon = FluxIcons.TriangleAlert,
                    buttons = listOf(
                        FluxDialogButton(cancel, FluxDialogButtonStyle.Secondary),
                        FluxDialogButton(cdnDisable, FluxDialogButtonStyle.Primary) {
                            ctx.editor.setNow(mapOf("cdn_multi_enabled" to "true", "proxy_mode" to "none"))
                        },
                    ),
                ),
            )
        } else {
            ctx.editor.set("cdn_multi_enabled", SettingsForm.wire(on))
        }
    }

    val shown = visibleDlRows(ctx.form, ctx.isLocalHost).mapTo(HashSet()) { it.id }
    val sections = DlSection.entries.filter { s -> DlRows.any { it.section == s && it.id in shown } }
    val legend = str(R.string.settingsSyncLegend)

    Box(Modifier.fillMaxSize()) {
        SettingsPageFrame(
            title = str(R.string.settingsCatDownload),
            onBack = { nav.pop() },
            modifier = Modifier.pointerInput(readOnly) {
                if (readOnly) detectTapGestures(onTap = { haptics.reject() })
            },
        ) {
            var index = 0
            if (readOnly) {
                flowItem(index++, gate, "banner") {
                    FluxBanner(str(R.string.localServiceDisconnected), kind = FluxBannerKind.Warn, slim = true)
                }
            }
            for (section in sections) {
                val footer = if (section == sections.last()) legend else null
                flowItem(index++, gate, section.key) {
                    GlassSection(animateSize, title = str(section.title), footer = footer) {
                        when (section) {
                            DlSection.Save -> saveRows(ctx, shown, onBrowse = { showRemotePicker = true })
                            DlSection.Behavior -> behaviorRows(ctx, shown, queues)
                            DlSection.Connection -> connectionRows(
                                ctx, shown, onCdnToggle, onNicHelp = { showNicHelp = true },
                                policy = ConnPolicyUi(connCount, connFailed, connClearing),
                                onClearPolicy = onClearConnPolicy,
                            )
                            DlSection.Retry -> retryRows(ctx, shown)
                            DlSection.Advanced -> advancedRows(ctx, shown, uaForceCustom) { uaForceCustom = it }
                        }
                    }
                }
            }
        }
        PickerSheetHost(picker = picker, onDismiss = { picker = null })
        NicHelpSheet(visible = showNicHelp, onDismiss = { showNicHelp = false })
        RemoteDirectoryPickerSheet(
            visible = showRemotePicker,
            startPath = ctx.form.string("default_save_dir"),
            onPick = { ctx.editor.set("default_save_dir", it, immediate = true) },
            onDismiss = { showRemotePicker = false },
        )
    }
}

// ───────────────────────────── 分组 ─────────────────────────────

private fun GlassSectionScope.saveRows(ctx: SettingsCtx, shown: Set<String>, onBrowse: () -> Unit) {
    if ("download.saveDir" in shown) {
        row {
            SettingRowBox(ctx, "download.saveDir", "default_save_dir") {
                FluxFieldRow(
                    title = str(R.string.defaultSaveDir),
                    subtitle = str(R.string.defaultSaveDirDesc),
                    cloud = ctx.synced("default_save_dir"),
                ) {
                    if (ctx.isLocalHost) {
                        LocalSaveDirPicker(ctx)
                    } else {
                        RemoteSaveDirField(ctx, onBrowse)
                    }
                }
            }
        }
    }
    if ("download.rememberLastSaveDir" in shown) {
        settingSwitch(
            ctx, "download.remember_last_save_dir", R.string.rememberLastSaveDir, R.string.rememberLastSaveDirDesc,
            id = "download.rememberLastSaveDir",
        )
    }
}

/** 远端主机默认保存目录：手填（绝对路径校验）+「浏览…」打开服务器目录选择器。 */
@Composable
private fun RemoteSaveDirField(ctx: SettingsCtx, onBrowse: () -> Unit) {
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val invalid = str(R.string.mobileSaveDirInvalid)
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        CommitTextField(
            value = ctx.form.string("default_save_dir"),
            onCommit = { raw ->
                val path = raw.trim()
                if (SettingsSaveDirectory.isValid(path)) {
                    ctx.editor.set("default_save_dir", path)
                } else {
                    haptics.reject()
                    overlays.toast(invalid, FluxToastKind.Warn)
                }
            },
            enabled = !ctx.readOnly,
            placeholder = str(R.string.mobileSaveDirUnset),
        )
        FluxButton(
            text = str(R.string.mobileBrowseServerFolders),
            onClick = onBrowse,
            variant = ButtonVariant.Ghost,
            size = ButtonSize.Sm,
            icon = FluxIcons.FolderOpen,
            enabled = !ctx.readOnly,
        )
    }
}

/**
 * 本机主机的保存目录：只读展示 + 系统目录选择器（SAF，映射为绝对路径）+ 快捷“系统 Download/FluxDown”
 * + 偏离默认时一键恢复。不提供手填：手填路径极易写成不可写目录（引擎下载时才报 EPERM）。
 *
 * 能否写入只看系统真实结果（[isLocalDirWritable]）；共享存储里需要“所有文件访问”的目录，
 * 不可写时引导授权，而不是在应用层另设白名单。系统选择器本身禁止选 Download 根 / Android/data
 * 等目录（Android 11+ 隐私限制），可在其下新建子文件夹选择，或直接用快捷项。
 */
@Composable
private fun LocalSaveDirPicker(ctx: SettingsCtx) {
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val current = ctx.form.string("default_save_dir")
    val defaultDir = remember(context) { defaultLocalSaveDir(context).path }
    val publicDir = remember { publicDownloadSaveDir().path }
    val notWritable = str(R.string.mobileSaveDirNotWritable)
    val unmappable = str(R.string.mobilePickDirUnmappable)
    // 授权后以同一目录重试：已授权时 canRequestStorageAccess 为 false，仍不可写只提示，不会循环弹授权
    var retry: (String) -> Unit = {}
    val promptAccess = rememberStorageAccessPrompt { retry(it) }

    fun apply(dir: String) {
        when {
            isLocalDirWritable(dir) -> if (dir != current) {
                haptics.tick()
                ctx.editor.set("default_save_dir", dir)
            }
            canRequestStorageAccess(context, dir) -> {
                haptics.reject()
                promptAccess(dir)
            }
            else -> {
                haptics.reject()
                overlays.toast(notWritable.fill("dir" to dir), FluxToastKind.Error)
            }
        }
    }
    retry = ::apply

    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri: Uri? ->
        if (uri == null) return@rememberLauncherForActivityResult
        val path = treeUriToPath(uri)
        if (path == null) {
            haptics.reject()
            overlays.toast(unmappable, FluxToastKind.Warn)
        } else {
            apply(path)
        }
    }
    val shown = current.ifEmpty { defaultDir }.trimEnd('/')
    val modified = shown != defaultDir.trimEnd('/')

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        FluxField(
            value = shown,
            onValueChange = {},
            readOnly = true,
            mono = true,
            singleLine = false,
            enabled = !ctx.readOnly,
            trailing = {
                Row {
                    if (modified) {
                        FluxFieldAction(FluxIcons.RotateCcw, str(R.string.restoreDefaultPath), onClick = { apply(defaultDir) })
                    }
                    FluxFieldAction(FluxIcons.FolderOpen, str(R.string.browse), onClick = { launcher.launch(null) })
                }
            },
        )
        if (shown != publicDir && !ctx.readOnly) {
            FluxPill(
                text = str(R.string.mobileUsePublicDownloadDir),
                selected = false,
                onClick = { apply(publicDir) },
                icon = FluxIcons.Download,
                small = true,
                toggleable = false,
            )
        }
    }
}

private fun GlassSectionScope.behaviorRows(ctx: SettingsCtx, shown: Set<String>, queues: List<com.fluxdown.core.model.Queue>) {
    if ("download.silentDownload" in shown) {
        settingSwitch(
            ctx, "download.silent_download", R.string.silentDownload, R.string.silentDownloadDesc,
            id = "download.silentDownload",
        )
    }
    if ("download.silentSkipSelection" in shown) {
        settingSwitch(
            ctx, "download.silent_skip_selection", R.string.silentSkipSelection, R.string.silentSkipSelectionDesc,
            id = "download.silentSkipSelection",
        )
    }
    if ("download.useServerTime" in shown) {
        settingSwitch(ctx, "use_server_time", R.string.useServerTime, R.string.useServerTimeDesc, id = "download.useServerTime")
    }
    if ("download.fileExistsBehavior" in shown) {
        settingChoice(
            ctx, "file_exists_behavior", R.string.fileExistsBehavior, R.string.fileExistsBehaviorDesc,
            id = "download.fileExistsBehavior",
        ) {
            listOf(
                SelectOption("rename", str(R.string.fileExistsRename)),
                SelectOption("overwrite", str(R.string.fileExistsOverwrite)),
                SelectOption("skip", str(R.string.fileExistsSkip)),
                SelectOption("ask", str(R.string.fileExistsAsk)),
            )
        }
    }
    if ("download.fileMissingAction" in shown) {
        settingChoice(
            ctx, "file_missing_action", R.string.fileMissingAction, R.string.fileMissingActionDesc,
            id = "download.fileMissingAction",
        ) {
            listOf(
                SelectOption("keep", str(R.string.fileMissingKeep)),
                SelectOption("delete", str(R.string.fileMissingDelete)),
            )
        }
    }
    if ("download.idleFileScan" in shown) {
        settingSwitch(ctx, "idle_file_scan", R.string.idleFileScan, R.string.idleFileScanDesc, id = "download.idleFileScan")
    }
    if ("download.defaultQueue" in shown) {
        settingChoice(
            ctx, "default_queue_id", R.string.defaultQueueSetting, R.string.defaultQueueSettingDesc,
            id = "download.defaultQueue",
        ) {
            listOf(SelectOption("", str(R.string.defaultQueue))) + queues.map { SelectOption(it.queueId, it.label()) }
        }
    }
    if ("download.dedupSameUrl" in shown) {
        settingSwitch(ctx, "dedup_same_url", R.string.mobileDedupSameUrl, R.string.mobileDedupSameUrlDesc, id = "download.dedupSameUrl")
    }
}

private fun GlassSectionScope.connectionRows(
    ctx: SettingsCtx,
    shown: Set<String>,
    onCdnToggle: (Boolean) -> Unit,
    onNicHelp: () -> Unit,
    policy: ConnPolicyUi,
    onClearPolicy: () -> Unit,
) {
    if ("download.defaultSegments" in shown) {
        settingStepper(
            ctx, "default_segments", R.string.defaultThreads, R.string.defaultThreadsDesc, 0..64, 0,
            zeroLabel = R.string.auto, id = "download.defaultSegments",
        )
    }
    if ("download.autoMaxConnections" in shown) {
        settingStepper(
            ctx, "auto_max_connections", R.string.autoMaxConnections, R.string.autoMaxConnectionsDesc, 0..128, 16,
            id = "download.autoMaxConnections",
        )
    }
    if ("download.cdnMulti" in shown) {
        settingSwitch(
            ctx, "cdn_multi_enabled", R.string.cdnMultiEnabled, R.string.cdnMultiEnabledDesc,
            id = "download.cdnMulti", onChange = onCdnToggle,
        )
    }
    if ("download.cdnMaxNodes" in shown) {
        settingStepper(
            ctx, "cdn_max_nodes", R.string.cdnMaxNodes, R.string.cdnMaxNodesDesc, 0..8, 0,
            zeroLabel = R.string.auto, id = "download.cdnMaxNodes",
        )
    }
    if ("download.multiNic" in shown) {
        settingSwitch(ctx, "multi_nic_enabled", R.string.multiNicEnabled, R.string.multiNicEnabledDesc, id = "download.multiNic")
        row(hasIcon = true) {
            FluxListRow(
                title = str(R.string.multiNicHelpHint),
                icon = FluxIcons.CircleHelp,
                chevron = true,
                onClick = onNicHelp,
            )
        }
    }
    if (ctx.form.isLoaded) {
        row {
            SettingRowBox(ctx, "download.connPolicy", null) {
                FluxListRow(
                    title = str(R.string.connPolicyCache),
                    subtitle = str(R.string.connPolicyCacheDesc),
                    value = when {
                        policy.count != null -> if (policy.count == 0L) str(R.string.connPolicyCacheEmpty) else policy.count.toString()
                        policy.failed -> "—"
                        else -> null
                    },
                    trailing = if (policy.count == null && !policy.failed) {
                        { FluxSpinner(size = 18.dp, label = str(R.string.mobileLoading)) }
                    } else {
                        null
                    },
                )
            }
        }
        if ((policy.count ?: 0L) > 0L) {
            row(hasIcon = true) {
                FluxActionRow(
                    title = str(R.string.connPolicyCacheClear),
                    onClick = onClearPolicy,
                    tone = Tone.Coral,
                    icon = FluxIcons.Trash2,
                    loading = policy.clearing,
                    enabled = !ctx.readOnly,
                )
            }
        }
    }
    if ("download.maxConcurrent" in shown) {
        settingNumber(
            ctx, "max_concurrent_tasks", R.string.maxConcurrent, R.string.maxConcurrentDesc, 1L..1024L, 5L,
            id = "download.maxConcurrent",
        )
    }
    if ("download.speedLimit" in shown) {
        settingSpeed(ctx, "speed_limit_bytes", R.string.speedLimit, R.string.speedLimitDesc, id = "download.speedLimit")
    }
    if ("download.uploadLimit" in shown) {
        settingSpeed(ctx, "upload_limit_bytes", R.string.uploadLimit, R.string.uploadLimitDesc, id = "download.uploadLimit")
    }
}

private fun GlassSectionScope.retryRows(ctx: SettingsCtx, shown: Set<String>) {
    if ("download.autoRetryCount" in shown) {
        settingStepper(
            ctx, "max_auto_retries", R.string.autoRetryCount, R.string.autoRetryCountDesc, -1..20, 3,
            zeroLabel = R.string.autoRetryOff, minusOneLabel = R.string.autoRetryUnlimited,
            id = "download.autoRetryCount",
        )
    }
    if ("download.autoRetryDelay" in shown) {
        settingNumber(
            ctx, "auto_retry_delay_secs", R.string.autoRetryDelay, R.string.autoRetryDelayDesc, 0L..86_400L, 5L,
            unit = R.string.autoRetryDelayUnit, id = "download.autoRetryDelay",
        )
    }
    if ("download.autoResumeOnStart" in shown) {
        settingSwitch(
            ctx, "auto_resume_on_start", R.string.autoResumeOnStart, R.string.autoResumeOnStartDesc,
            id = "download.autoResumeOnStart",
        )
    }
}

private fun GlassSectionScope.advancedRows(
    ctx: SettingsCtx,
    shown: Set<String>,
    uaForceCustom: Boolean,
    setUaForceCustom: (Boolean) -> Unit,
) {
    if ("download.userAgent" !in shown) return
    val key = "global_user_agent"
    val current = ctx.form.string(key)
    val matched = UaPresets.firstOrNull { it.ua == current }?.id ?: if (current.isEmpty()) UA_DEFAULT else UA_CUSTOM
    val effective = if (uaForceCustom) UA_CUSTOM else matched
    row {
        SettingRowBox(ctx, "download.userAgent", key) {
            val title = str(R.string.userAgent)
            val options = buildList {
                add(SelectOption(UA_DEFAULT, str(R.string.userAgentPresetDefault)))
                UaPresets.forEach { add(SelectOption(it.id, str(it.label), hint = it.ua)) }
                add(SelectOption(UA_CUSTOM, str(R.string.userAgentPresetCustom)))
            }
            FluxListRow(
                title = title,
                subtitle = str(R.string.userAgentDesc),
                value = options.firstOrNull { it.value == effective }?.label,
                chevron = true,
                cloud = ctx.synced(key),
                enabled = !ctx.readOnly,
                onClick = {
                    ctx.openPicker(
                        PickerSpec(title, options, effective) { id ->
                            when (id) {
                                UA_DEFAULT -> {
                                    setUaForceCustom(false)
                                    ctx.editor.set(key, "")
                                }
                                UA_CUSTOM -> setUaForceCustom(true)
                                else -> {
                                    setUaForceCustom(false)
                                    UaPresets.firstOrNull { it.id == id }?.let { ctx.editor.set(key, it.ua) }
                                }
                            }
                        },
                    )
                },
            )
        }
    }
    if (effective == UA_CUSTOM) {
        row {
            SettingRowBox(ctx, "download.userAgent.custom", key) {
                FluxFieldRow(title = str(R.string.userAgentPresetCustom), cloud = ctx.synced(key)) {
                    CommitTextField(
                        value = current,
                        onCommit = { ctx.editor.set(key, it.trim()) },
                        enabled = !ctx.readOnly,
                        placeholder = str(R.string.userAgentPlaceholder),
                    )
                }
            }
        }
    }
}

/** 多网卡聚合说明（沿用 PC 文案）。 */
@Composable
private fun NicHelpSheet(visible: Boolean, onDismiss: () -> Unit) {
    val title = str(R.string.multiNicHelpTitle)
    val close = str(R.string.close)
    FluxPortal {
        FluxSheet(
            visible = visible,
            onDismissRequest = onDismiss,
            title = title,
            header = {
                FluxSheetHeader(
                    title = title,
                    actions = { FluxIconButton(FluxIcons.X, close, onDismiss, size = IconButtonSize.Sm) },
                )
            },
        ) {
            FluxText(
                text = str(R.string.multiNicHelp),
                style = FluxTheme.type.body,
                color = FluxTheme.colors.inkMuted,
                modifier = Modifier.padding(horizontal = 6.dp),
            )
        }
    }
}
