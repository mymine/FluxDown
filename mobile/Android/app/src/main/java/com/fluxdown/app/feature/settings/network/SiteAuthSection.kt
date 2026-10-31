package com.fluxdown.app.feature.settings.network

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.actions.LocalTaskActions
import com.fluxdown.app.feature.settings.SettingRowBox
import com.fluxdown.app.feature.settings.SettingsCtx
import com.fluxdown.app.feature.settings.rememberLastNonNull
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostException
import com.fluxdown.core.protocol.NetworkRow
import com.fluxdown.core.protocol.SiteAuthEntryDto
import com.fluxdown.core.protocol.SiteAuthFilter
import com.fluxdown.core.protocol.SiteAuthSaveRequest
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxActionRow
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxFieldAction
import com.fluxdown.fluxui.controls.FluxIconButton
import com.fluxdown.fluxui.controls.FluxKeyValue
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxPasswordField
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.IconButtonSize
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.feedback.FluxSpinner
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxPortal
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetFooter
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.launch

/*
 * 已保存的网站凭据：`daemon.siteAuth.*`。列表只含站点与用户名；明文密码只在编辑表单里经
 * `daemon.siteAuth.get` 取回，不进入列表状态、不写日志。
 */

/** 编辑表单的目标：新建 / 编辑已有站点。 */
internal sealed interface SiteAuthTarget {
    data object Add : SiteAuthTarget
    data class Edit(val entry: SiteAuthEntryDto) : SiteAuthTarget
}

@Composable
internal fun SiteAuthSection(
    ctx: SettingsCtx,
    model: SiteAuthModel,
    remoteHostName: String?,
    onEdit: (SiteAuthTarget) -> Unit,
    onDelete: (SiteAuthEntryDto) -> Unit,
    onClear: () -> Unit,
    onReload: () -> Unit,
) {
    var query by rememberSaveable { mutableStateOf("") }
    val entries = model.entries
    val showsSearch = SiteAuthFilter.showsSearch(entries.size)
    // 搜索框消失（条数降到阈值以下）时查询随之失效。
    val activeQuery = if (showsSearch) query else ""
    val visible = SiteAuthFilter.filter(entries, activeQuery)
    val phase = model.phase
    val footer = remoteHostName?.let { str(R.string.mobileSiteAuthRemoteNote, "host" to it) }

    GlassSection(footer = footer) {
        row {
            SettingRowBox(ctx, NetworkRow.SiteAuth.rowId, null) {
                val value = if (phase == SiteAuthModel.Phase.Loaded && entries.isNotEmpty()) {
                    if (activeQuery.isBlank()) {
                        str(R.string.settingsSiteAuthCount, "n" to entries.size)
                    } else {
                        str(R.string.settingsSiteAuthCountFiltered, "m" to visible.size, "n" to entries.size)
                    }
                } else {
                    null
                }
                val loadingText = str(R.string.mobileLoading)
                FluxListRow(
                    title = str(R.string.settingsSiteAuthTitle),
                    subtitle = str(R.string.settingsSiteAuthDesc),
                    value = value,
                    trailing = if (phase == SiteAuthModel.Phase.Loading) {
                        { FluxSpinner(size = 18.dp, label = loadingText) }
                    } else {
                        null
                    },
                )
            }
        }
        when (phase) {
            SiteAuthModel.Phase.Loading -> Unit
            is SiteAuthModel.Phase.Failed -> {
                row(hasIcon = true) {
                    FluxListRow(title = phase.message, icon = FluxIcons.CircleAlert, danger = true)
                }
                row(hasIcon = true) {
                    FluxActionRow(str(R.string.mobileRetry), onClick = onReload, icon = FluxIcons.RefreshCw)
                }
            }
            SiteAuthModel.Phase.Loaded -> {
                if (entries.isEmpty()) {
                    custom(padded = true) { EmptyNote(str(R.string.settingsSiteAuthEmpty)) }
                } else {
                    if (showsSearch) {
                        custom(padded = true) {
                            val clear = str(R.string.mobileSiteAuthSearchClear)
                            FluxField(
                                value = query,
                                onValueChange = { query = it },
                                placeholder = str(R.string.settingsSiteAuthSearchHint),
                                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                                trailing = if (query.isNotEmpty()) {
                                    { FluxFieldAction(FluxIcons.X, clear, onClick = { query = "" }) }
                                } else {
                                    null
                                },
                            )
                        }
                    }
                    if (visible.isEmpty()) {
                        custom(padded = true) { EmptyNote(str(R.string.settingsSiteAuthNoMatch)) }
                    }
                    for (entry in visible) {
                        row {
                            val delete = str(R.string.settingsSiteAuthDelete)
                            FluxListRow(
                                title = entry.site,
                                subtitle = entry.user.ifEmpty { null },
                                enabled = !ctx.readOnly && entry.site !in model.deleting,
                                onClick = { onEdit(SiteAuthTarget.Edit(entry)) },
                                trailing = {
                                    FluxIconButton(
                                        FluxIcons.Trash2,
                                        delete,
                                        onClick = { onDelete(entry) },
                                        size = IconButtonSize.Sm,
                                        enabled = !ctx.readOnly && entry.site !in model.deleting,
                                    )
                                },
                            )
                        }
                    }
                }
            }
        }
        row(hasIcon = true) {
            SettingRowBox(ctx, NetworkRow.SiteAuthAdd.rowId, null) {
                FluxActionRow(
                    str(R.string.settingsSiteAuthAdd),
                    onClick = { onEdit(SiteAuthTarget.Add) },
                    icon = FluxIcons.Plus,
                    enabled = !ctx.readOnly,
                )
            }
        }
        row(hasIcon = true) {
            SettingRowBox(ctx, NetworkRow.SiteAuthClear.rowId, null) {
                FluxActionRow(
                    str(R.string.settingsSiteAuthClearAll),
                    onClick = onClear,
                    tone = Tone.Coral,
                    icon = FluxIcons.Trash,
                    loading = model.isClearing,
                    enabled = !ctx.readOnly && entries.isNotEmpty(),
                )
            }
        }
    }
}

@Composable
internal fun EmptyNote(text: String) {
    Box(Modifier.fillMaxWidth().padding(vertical = 16.dp), contentAlignment = Alignment.Center) {
        FluxText(
            text,
            style = FluxTheme.type.sm.copy(textAlign = TextAlign.Center),
            color = FluxTheme.colors.inkMuted,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

private sealed interface FormPhase {
    data object Loading : FormPhase
    data object Ready : FormPhase
    data class Failed(val message: String) : FormPhase
}

/**
 * 添加 / 编辑凭据：站点、用户名、密码（带显隐）。编辑时站点是主键只读；
 * 明文密码经 `daemon.siteAuth.get` 取回，仅存活于本表单。[target] = null 时收起。
 */
@Composable
internal fun SiteAuthEditSheet(
    target: SiteAuthTarget?,
    model: SiteAuthModel,
    onDismiss: () -> Unit,
    onSaved: () -> Unit,
) {
    val container = LocalAppContainer.current
    val actions = LocalTaskActions.current
    val haptics = FluxTheme.haptics
    val scope = rememberCoroutineScope()
    val spec = rememberLastNonNull(target)
    val editingSite = (spec as? SiteAuthTarget.Edit)?.entry?.site

    var site by remember { mutableStateOf("") }
    var user by remember { mutableStateOf("") }
    var pass by remember { mutableStateOf("") }
    var phase by remember { mutableStateOf<FormPhase>(FormPhase.Ready) }
    var saving by remember { mutableStateOf(false) }
    var saveError by remember { mutableStateOf<String?>(null) }
    var reloadTick by remember { mutableStateOf(0) }

    // 每次打开重置；编辑时回填（站点已被别处删除 → 保留列表里的用户名、密码留空）。
    LaunchedEffect(target, reloadTick) {
        val current = target ?: return@LaunchedEffect
        saveError = null
        if (current is SiteAuthTarget.Edit) {
            site = current.entry.site
            if (reloadTick == 0) {
                user = current.entry.user
                pass = ""
            }
            phase = FormPhase.Loading
            try {
                val credential = model.credential(current.entry.site, container.session)
                if (credential != null) {
                    user = credential.user
                    pass = credential.pass
                }
                phase = FormPhase.Ready
            } catch (e: HostException) {
                phase = FormPhase.Failed(actions.errorText(e))
            }
        } else {
            site = ""
            user = ""
            pass = ""
            phase = FormPhase.Ready
        }
    }
    LaunchedEffect(target) { if (target == null) reloadTick = 0 }

    val canSave = phase == FormPhase.Ready && !saving && SiteAuthFilter.canSave(editingSite ?: site, user)
    val title = str(if (editingSite == null) R.string.settingsSiteAuthAdd else R.string.settingsSiteAuthEdit)
    val close = str(R.string.close)

    fun save() {
        if (!canSave) return
        val request = SiteAuthSaveRequest(
            site = (editingSite ?: site).trim(),
            user = user.trim(),
            pass = pass,
        )
        val session = container.session
        saving = true
        saveError = null
        scope.launch {
            try {
                model.save(request, session) { actions.errorText(it) }
                onSaved()
                onDismiss()
            } catch (e: HostException) {
                saveError = actions.errorText(e)
                haptics.reject()
            } finally {
                saving = false
            }
        }
    }

    FluxPortal {
        FluxSheet(
            visible = target != null,
            onDismissRequest = { if (!saving) onDismiss() },
            dismissible = !saving,
            title = title,
            header = {
                FluxSheetHeader(
                    title = title,
                    actions = { FluxIconButton(FluxIcons.X, close, onClick = { if (!saving) onDismiss() }, size = IconButtonSize.Sm) },
                )
            },
            footer = {
                FluxSheetFooter {
                    FluxButton(
                        str(R.string.cancel),
                        onClick = { if (!saving) onDismiss() },
                        modifier = Modifier.weight(1f),
                        variant = ButtonVariant.Secondary,
                    )
                    FluxButton(
                        str(R.string.settingsSiteAuthSave),
                        onClick = ::save,
                        modifier = Modifier.weight(1f),
                        variant = ButtonVariant.Primary,
                        enabled = canSave,
                        loading = saving,
                    )
                }
            },
        ) {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                if (editingSite != null) {
                    GlassSection {
                        row { FluxKeyValue(str(R.string.settingsSiteAuthSite), editingSite, mono = true) }
                    }
                } else {
                    FluxField(
                        value = site,
                        onValueChange = { site = it },
                        label = str(R.string.settingsSiteAuthSite),
                        placeholder = str(R.string.settingsSiteAuthSitePlaceholder),
                        mono = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next),
                    )
                }
                FluxField(
                    value = user,
                    onValueChange = { user = it },
                    label = str(R.string.taskHttpAuthUser),
                    enabled = phase != FormPhase.Loading,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next),
                )
                FluxPasswordField(
                    value = pass,
                    onValueChange = { pass = it },
                    label = str(R.string.taskHttpAuthPassword),
                    enabled = phase != FormPhase.Loading,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { save() }),
                )
                when (val p = phase) {
                    FormPhase.Loading -> FluxSpinner(size = 20.dp, label = str(R.string.mobileLoading))
                    is FormPhase.Failed -> {
                        FluxText(p.message, style = FluxTheme.type.sm, color = FluxTheme.colors.coralText)
                        FluxButton(
                            str(R.string.mobileRetry),
                            onClick = { reloadTick++ },
                            variant = ButtonVariant.Secondary,
                            icon = FluxIcons.RefreshCw,
                        )
                    }
                    FormPhase.Ready -> Unit
                }
                saveError?.let {
                    FluxText(it, style = FluxTheme.type.sm, color = FluxTheme.colors.coralText)
                }
            }
        }
    }
}
