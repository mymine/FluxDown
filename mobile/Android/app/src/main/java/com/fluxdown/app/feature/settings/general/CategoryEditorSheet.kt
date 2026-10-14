package com.fluxdown.app.feature.settings.general

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.RemoteDirectoryPickerSheet
import com.fluxdown.app.feature.settings.rememberLastNonNull
import com.fluxdown.app.i18n.fill
import com.fluxdown.app.i18n.str
import com.fluxdown.app.ui.canRequestStorageAccess
import com.fluxdown.app.ui.rememberStorageAccessPrompt
import com.fluxdown.app.ui.isLocalDirWritable
import com.fluxdown.app.ui.treeUriToPath
import com.fluxdown.core.protocol.CategoryBuild
import com.fluxdown.core.protocol.CategoryDraft
import com.fluxdown.core.protocol.CategoryRules
import com.fluxdown.core.protocol.CategoryValidationError
import com.fluxdown.core.protocol.CustomCategoryDto
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxFieldAction
import com.fluxdown.fluxui.controls.FluxSegmented
import com.fluxdown.fluxui.controls.FluxTag
import com.fluxdown.fluxui.controls.SegOption
import com.fluxdown.fluxui.icons.FluxIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.FluxPortal
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetDetent
import com.fluxdown.fluxui.overlay.FluxSheetFooter
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import com.fluxdown.fluxui.theme.fluxPressable

@StringRes
private fun CategoryValidationError.message(): Int = when (this) {
    CategoryValidationError.NameRequired -> R.string.categoryNameRequired
    CategoryValidationError.ExtensionsRequired -> R.string.extensionsRequired
    CategoryValidationError.RegexInvalid -> R.string.regexInvalid
}

/**
 * 分类编辑器（新建 / 编辑）。自包含：只依赖分类条目与回调，保存 / 删除由调用方写回 `custom_categories`。
 * 校验与条目生成全部走 [CategoryRules.build]。[target] = null 时收起；本机主机的保存目录用系统目录选择器
 * （SAF → 绝对路径 + 可写检查），远端主机可手填或浏览服务器目录。有未保存修改时只能经按钮关闭（先确认丢弃）。
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun CategoryEditorSheet(
    target: CategoryTarget?,
    isLocalHost: Boolean,
    onSave: (CustomCategoryDto) -> Unit,
    onDelete: (CustomCategoryDto) -> Unit,
    onDismiss: () -> Unit,
) {
    val shown = rememberLastNonNull(target)
    val existing = shown?.existing
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val initial = remember(shown) { CategoryDraft(existing) }
    var draft by remember(shown) { mutableStateOf(initial) }
    var error by remember(shown) { mutableStateOf<CategoryValidationError?>(null) }
    var showRemotePicker by remember(shown) { mutableStateOf(false) }
    val dirty = draft != initial
    val canDelete = existing != null && !existing.isBuiltin

    val title = str(if (existing == null) R.string.addCategory else R.string.editCategory)
    val notWritable = str(R.string.mobileSaveDirNotWritable)
    val unmappable = str(R.string.mobilePickDirUnmappable)
    val cancel = str(R.string.cancel)
    // 授权后以同一目录重试（已授权时不会再弹授权）
    var retryDir: (String) -> Unit = {}
    val promptAccess = rememberStorageAccessPrompt { retryDir(it) }

    fun change(next: CategoryDraft) {
        draft = next
        error = null
    }

    fun requestClose() {
        if (!dirty) {
            onDismiss()
            return
        }
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.mobileGeneralDiscardTitle),
                icon = FluxIcons.TriangleAlert,
                buttons = listOf(
                    FluxDialogButton(context.str(R.string.mobileGeneralKeepEditing)),
                    FluxDialogButton(context.str(R.string.mobileGeneralDiscard), FluxDialogButtonStyle.Destructive) { onDismiss() },
                ),
            ),
        )
    }

    fun confirmDelete() {
        val entry = existing ?: return
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.deleteCategory),
                message = context.str(R.string.deleteCategoryConfirm),
                icon = FluxIcons.Trash2,
                buttons = listOf(
                    FluxDialogButton(cancel),
                    FluxDialogButton(context.str(R.string.delete), FluxDialogButtonStyle.Destructive) {
                        onDelete(entry)
                        onDismiss()
                    },
                ),
            ),
        )
    }

    fun save() {
        when (val result = CategoryRules.build(draft, System.currentTimeMillis())) {
            is CategoryBuild.Failure -> {
                error = result.error
                haptics.reject()
            }
            is CategoryBuild.Success -> {
                val dir = result.entry.saveDir
                if (isLocalHost && !isLocalDirWritable(dir)) {
                    haptics.reject()
                    overlays.toast(notWritable.fill("dir" to dir), FluxToastKind.Error)
                    return
                }
                onSave(result.entry)
                onDismiss()
            }
        }
    }

    fun applyLocalDir(dir: String) {
        when {
            isLocalDirWritable(dir) -> {
                haptics.tick()
                change(draft.copy(saveDir = dir))
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
    retryDir = ::applyLocalDir

    val treePicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri: Uri? ->
        if (uri == null) return@rememberLauncherForActivityResult
        val path = treeUriToPath(uri)
        if (path == null) {
            haptics.reject()
            overlays.toast(unmappable, FluxToastKind.Warn)
        } else {
            applyLocalDir(path)
        }
    }

    FluxPortal {
        FluxSheet(
            visible = target != null,
            onDismissRequest = ::requestClose,
            detent = FluxSheetDetent.Full,
            dismissible = !dirty,
            title = title,
            header = { FluxSheetHeader(title = title, onClose = ::requestClose) },
            footer = {
                FluxSheetFooter {
                    FluxButton(
                        text = cancel,
                        onClick = ::requestClose,
                        modifier = Modifier.weight(1f),
                        variant = ButtonVariant.Secondary,
                    )
                    FluxButton(
                        text = str(R.string.confirm),
                        onClick = ::save,
                        modifier = Modifier.weight(1f),
                        variant = ButtonVariant.Primary,
                    )
                }
            },
        ) {
            if (shown != null) {
                EditorBody(
                    existing = existing,
                    draft = draft,
                    error = error,
                    isLocalHost = isLocalHost,
                    canDelete = canDelete,
                    onChange = ::change,
                    onBrowseLocal = { treePicker.launch(null) },
                    onBrowseRemote = { showRemotePicker = true },
                    onDelete = ::confirmDelete,
                )
            }
        }
    }
    if (!isLocalHost) {
        RemoteDirectoryPickerSheet(
            visible = showRemotePicker,
            startPath = draft.saveDir,
            onPick = {
                showRemotePicker = false
                change(draft.copy(saveDir = it))
            },
            onDismiss = { showRemotePicker = false },
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ColumnScope.EditorBody(
    existing: CustomCategoryDto?,
    draft: CategoryDraft,
    error: CategoryValidationError?,
    isLocalHost: Boolean,
    canDelete: Boolean,
    onChange: (CategoryDraft) -> Unit,
    onBrowseLocal: () -> Unit,
    onBrowseRemote: () -> Unit,
    onDelete: () -> Unit,
) {
    val colors = FluxTheme.colors
    val type = FluxTheme.type
    val context = LocalContext.current
    val showsRules = existing?.hasMatchRules ?: true
    val showsSaveDir = existing?.isAll != true
    val isRegex = draft.matchMode == "regex"
    val namePrompt = if (existing != null && existing.isBuiltin) categoryDisplayName(context, existing) else str(R.string.categoryNameHint)

    androidx.compose.foundation.layout.Column(
        Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        FluxField(
            value = draft.name,
            onValueChange = { onChange(draft.copy(name = it)) },
            label = str(R.string.categoryName),
            placeholder = namePrompt,
            error = error?.takeIf { it == CategoryValidationError.NameRequired }?.let { str(it.message()) },
        )

        // 图标：5 列方格，选中 = 强调色描边（形状 + 描边，不只靠颜色）
        androidx.compose.foundation.layout.Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            FluxText(str(R.string.categoryIcon), style = type.micro, color = colors.inkMuted, modifier = Modifier.padding(start = 4.dp))
            for (rowNames in CategoryRules.iconNames.chunked(ICON_COLUMNS)) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (name in rowNames) {
                        IconCell(
                            name = name,
                            selected = draft.icon == name,
                            onClick = { onChange(draft.copy(icon = name)) },
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            }
        }

        if (showsRules) {
            androidx.compose.foundation.layout.Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                FluxText(str(R.string.matchMode), style = type.micro, color = colors.inkMuted, modifier = Modifier.padding(start = 4.dp))
                FluxSegmented(
                    options = listOf(
                        SegOption("extension", str(R.string.matchByExtension)),
                        SegOption("regex", str(R.string.matchByRegex)),
                    ),
                    selected = if (isRegex) "regex" else "extension",
                    onSelect = { onChange(draft.copy(matchMode = it)) },
                )
                if (isRegex) {
                    FluxField(
                        value = draft.regexText,
                        onValueChange = { onChange(draft.copy(regexText = it)) },
                        label = str(R.string.regexLabel),
                        placeholder = str(R.string.regexHint),
                        mono = true,
                        error = error?.takeIf { it == CategoryValidationError.RegexInvalid }?.let { str(it.message()) },
                    )
                } else {
                    FluxField(
                        value = draft.extensionsText,
                        onValueChange = { onChange(draft.copy(extensionsText = it)) },
                        label = str(R.string.extensionsLabel),
                        placeholder = str(R.string.extensionsHint),
                        error = error?.takeIf { it == CategoryValidationError.ExtensionsRequired }?.let { str(it.message()) },
                    )
                    val tokens = CategoryRules.parseExtensions(draft.extensionsText)
                    if (tokens.isNotEmpty()) {
                        FlowRow(
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            for (token in tokens) FluxTag(".$token")
                        }
                    }
                }
            }
        }

        if (showsSaveDir) {
            SaveDirField(
                value = draft.saveDir,
                isLocalHost = isLocalHost,
                onChange = { onChange(draft.copy(saveDir = it)) },
                onBrowse = if (isLocalHost) onBrowseLocal else onBrowseRemote,
            )
        }

        if (canDelete) {
            FluxButton(
                text = str(R.string.deleteCategory),
                onClick = onDelete,
                variant = ButtonVariant.Danger,
                icon = FluxIcons.Trash2,
                fullWidth = true,
            )
        }
    }
}

/**
 * 保存目录：本机 = 只读展示 + 系统目录选择器（手填路径极易写成不可写目录）；远端 = 可手填，也可浏览服务器目录。
 */
@Composable
private fun SaveDirField(value: String, isLocalHost: Boolean, onChange: (String) -> Unit, onBrowse: () -> Unit) {
    FluxField(
        value = value,
        onValueChange = onChange,
        label = str(R.string.categorySaveDir),
        placeholder = str(R.string.mobileSaveDirUnset),
        hint = str(R.string.categorySaveDirDesc),
        mono = true,
        singleLine = false,
        readOnly = isLocalHost,
        trailing = {
            Row {
                if (value.isNotEmpty()) {
                    FluxFieldAction(FluxIcons.RotateCcw, str(R.string.restoreDefaultPath), onClick = { onChange("") })
                }
                FluxFieldAction(
                    FluxIcons.FolderOpen,
                    str(if (isLocalHost) R.string.browse else R.string.mobileBrowseServerFolders),
                    onClick = onBrowse,
                )
            }
        },
    )
}

private const val ICON_COLUMNS = 5

@Composable
private fun IconCell(name: String, selected: Boolean, onClick: () -> Unit, modifier: Modifier) {
    val c = FluxTheme.colors
    val shape = RoundedCornerShape(12.dp)
    val label = str(categoryIconName(name))
    Box(
        modifier
            .aspectRatio(1f)
            .clip(shape)
            .background(if (selected) c.accentLo else c.glass2)
            .border(if (selected) 2.dp else 0.5.dp, if (selected) c.accent else c.hairline, shape)
            .fluxPressable(onClick = onClick, role = Role.RadioButton)
            .semantics {
                contentDescription = label
                this.selected = selected
            },
        contentAlignment = Alignment.Center,
    ) {
        FluxIcon(categoryIcon(name), null, size = 20.dp, tint = if (selected) c.accentHi else c.ink)
    }
}
