package com.fluxdown.app.feature.settings.general

import android.content.Context
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Row
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.vector.ImageVector
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.SettingRowBox
import com.fluxdown.app.feature.settings.SettingsCtx
import com.fluxdown.app.i18n.fill
import com.fluxdown.app.i18n.str
import com.fluxdown.app.ui.friendlyLocalPath
import com.fluxdown.app.ui.prepareLocalDirectories
import com.fluxdown.core.protocol.CategoryRules
import com.fluxdown.core.protocol.CustomCategoryDto
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxIconButton
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxTag
import com.fluxdown.fluxui.controls.GlassSectionScope
import com.fluxdown.fluxui.controls.IconButtonSize
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxOverlayState
import com.fluxdown.fluxui.overlay.FluxToastAction
import com.fluxdown.fluxui.overlay.FluxToastKind
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** 「自定义分类」行 id（下载页；与 iOS 一致）。 */
internal const val CATEGORIES_ROW_ID = "download.categories"

/** 分类编辑器的呈现目标；每次打开新建一个实例（以实例身份区分「再次打开」，草稿随之重置）。 */
internal class CategoryTarget(val existing: CustomCategoryDto?)

/**
 * 分类列表的读写（唯一写路径：`ConfigEditor.setPreference`，整张重排后的列表，`position` = 下标）。
 * 本机主机的目录在 [scope] 内经 IO 线程创建（分类目录写入偏好后立即可在文件管理器中看到）。
 */
internal class CategoryStore(
    private val ctx: SettingsCtx,
    private val overlays: FluxOverlayState,
    private val context: Context,
    private val scope: CoroutineScope,
) {
    val list: List<CustomCategoryDto>
        get() = CustomCategoryDto.fromPreference(ctx.form.pref(CustomCategoryDto.PREFERENCE_KEY))

    /** 「一键分类目录」的基准目录（默认保存目录）。 */
    val defaultSaveDir: String get() = ctx.form.string("default_save_dir").trim()

    private fun write(next: List<CustomCategoryDto>) {
        ctx.editor.setPreference(CustomCategoryDto.PREFERENCE_KEY, CustomCategoryDto.preferenceValue(next), immediate = true)
    }

    fun save(entry: CustomCategoryDto) = write(CategoryRules.upserting(entry, list))

    /** 与相邻项交换位置（[delta] = -1 上移 / +1 下移）。 */
    fun move(entry: CustomCategoryDto, delta: Int) {
        val current = list
        val index = current.indexOfFirst { it.id == entry.id }
        if (index < 0) return
        val neighbor = current.getOrNull(index + delta) ?: return
        CategoryRules.reorder(current, entry.id, neighbor.id)?.let { write(it) }
    }

    /** 删除（仅非内置）；toast 提供撤销，恢复删除前的整张列表。 */
    fun delete(entry: CustomCategoryDto) {
        if (entry.isBuiltin) return
        val previous = list
        write(previous.filter { it.id != entry.id })
        overlays.toast(
            context.str(R.string.mobileGeneralCategoryDeleted, "name" to categoryDisplayName(context, entry)),
            FluxToastKind.Info,
            FluxIcons.Trash2,
            action = FluxToastAction(context.str(R.string.mobileGeneralUndo)) { write(previous) },
        )
    }

    /**
     * 一键分类目录：改写偏好后，本机主机把各分类目录逐个建好（含此前已写入但从未建出的目录）；
     * 远端主机的路径在服务器上，由引擎在下载时创建。
     */
    fun applyAutoDirs() {
        val current = list
        val base = defaultSaveDir
        val updated = CategoryRules.autoDirs(current, base) ?: return
        val changed = CategoryRules.autoDirsChangeCount(current, base)
        if (changed > 0) write(updated)
        val done = if (changed == 0) {
            context.str(R.string.mobileGeneralAutoDirsUnchanged) to FluxToastKind.Info
        } else {
            context.str(R.string.mobileGeneralAutoDirsDone, "n" to changed) to FluxToastKind.Success
        }
        if (!ctx.isLocalHost) {
            overlays.toast(done.first, done.second)
            return
        }
        scope.launch {
            val failed = withContext(Dispatchers.IO) { prepareLocalDirectories(updated.map { it.saveDir }) }
            val firstFailed = failed.firstOrNull()
            if (firstFailed == null) {
                overlays.toast(done.first, done.second)
            } else {
                val dir = friendlyLocalPath(context, firstFailed)
                overlays.toast(context.str(R.string.mobileSaveDirNotWritable).fill("dir" to dir), FluxToastKind.Error)
            }
        }
    }

    fun resetBuiltin() = write(CustomCategoryDto.builtinDefaults)
}

// ───────────────────────────── 文案 / 图标 ─────────────────────────────

@StringRes
private fun builtinLabelRes(builtinType: String): Int = when (builtinType) {
    "all" -> R.string.categoryAll
    "video" -> R.string.categoryVideo
    "audio" -> R.string.categoryAudio
    "document" -> R.string.categoryDocument
    "image" -> R.string.categoryImage
    "program" -> R.string.categoryProgram
    "archive" -> R.string.categoryArchive
    else -> R.string.categoryOther
}

/** 显示名：内置分类用本地化标签，自定义分类用其名称（同 GPUI `display_name`）。 */
internal fun categoryDisplayName(context: Context, entry: CustomCategoryDto): String {
    val type = entry.builtinType
    return if (entry.isBuiltin && type != null) context.str(builtinLabelRes(type)) else entry.name
}

/** 详情行：`.ext, .ext` / `正则表达式: pattern` / `网址模式: pattern`；无规则（全部 / 其他）为 null。 */
internal fun categoryDetail(context: Context, entry: CustomCategoryDto): String? {
    if (!entry.hasMatchRules) return null
    if (entry.isRegex) {
        return entry.regexPattern.takeIf { it.isNotEmpty() }?.let { "${context.str(R.string.regexLabel)}: $it" }
    }
    if (entry.isUrl) {
        return entry.urlPattern.takeIf { it.isNotEmpty() }?.let { "${context.str(R.string.urlPatternLabel)}: $it" }
    }
    return entry.extensions.takeIf { it.isNotEmpty() }?.joinToString(", ") { ".$it" }
}

/** 分类图标 wire 名 → Lucide 图标（wire 名原样保留，与 PC / Web / iOS 互通）；未知名回退「文件」。 */
internal fun categoryIcon(wire: String): ImageVector = when (wire) {
    "folders" -> FluxIcons.Folders
    "film" -> FluxIcons.Film
    "music" -> FluxIcons.Music
    "fileText" -> FluxIcons.FileText
    "image" -> FluxIcons.Image
    "archive" -> FluxIcons.Archive
    "file" -> FluxIcons.File
    "code" -> FluxIcons.Code
    "database" -> FluxIcons.Database
    "gamepad" -> FluxIcons.Gamepad
    "globe" -> FluxIcons.Globe
    "bookmark" -> FluxIcons.Bookmark
    "box" -> FluxIcons.Box
    "cpu" -> FluxIcons.Cpu
    "disc" -> FluxIcons.Disc
    "font" -> FluxIcons.Font
    "hardDrive" -> FluxIcons.HardDrive
    "library" -> FluxIcons.Library
    "package2" -> FluxIcons.Package
    "pen" -> FluxIcons.Pen
    "printer" -> FluxIcons.Printer
    "smartphone" -> FluxIcons.Smartphone
    "subtitles" -> FluxIcons.Subtitles
    "type" -> FluxIcons.Type
    "zap" -> FluxIcons.Zap
    else -> FluxIcons.File
}

/** 图标格的无障碍名称（`mobileGeneralIconName_<wire>`）。 */
@StringRes
internal fun categoryIconName(wire: String): Int = when (wire) {
    "folders" -> R.string.mobileGeneralIconName_folders
    "film" -> R.string.mobileGeneralIconName_film
    "music" -> R.string.mobileGeneralIconName_music
    "fileText" -> R.string.mobileGeneralIconName_fileText
    "image" -> R.string.mobileGeneralIconName_image
    "archive" -> R.string.mobileGeneralIconName_archive
    "code" -> R.string.mobileGeneralIconName_code
    "database" -> R.string.mobileGeneralIconName_database
    "gamepad" -> R.string.mobileGeneralIconName_gamepad
    "globe" -> R.string.mobileGeneralIconName_globe
    "bookmark" -> R.string.mobileGeneralIconName_bookmark
    "box" -> R.string.mobileGeneralIconName_box
    "cpu" -> R.string.mobileGeneralIconName_cpu
    "disc" -> R.string.mobileGeneralIconName_disc
    "font" -> R.string.mobileGeneralIconName_font
    "hardDrive" -> R.string.mobileGeneralIconName_hardDrive
    "library" -> R.string.mobileGeneralIconName_library
    "package2" -> R.string.mobileGeneralIconName_package2
    "pen" -> R.string.mobileGeneralIconName_pen
    "printer" -> R.string.mobileGeneralIconName_printer
    "smartphone" -> R.string.mobileGeneralIconName_smartphone
    "subtitles" -> R.string.mobileGeneralIconName_subtitles
    "type" -> R.string.mobileGeneralIconName_type
    "zap" -> R.string.mobileGeneralIconName_zap
    else -> R.string.mobileGeneralIconName_file
}

// ───────────────────────────── 列表行 ─────────────────────────────

/**
 * 「自定义分类」分组的行：标题行（☁︎ + 重排开关）→ 各分类（点按编辑 / 长按删除自定义分类 / 重排模式下上下移）。
 * 回调由页面承接：[onEdit] 打开编辑器，[onDeleteRequest] 弹删除确认。
 */
internal fun GlassSectionScope.categoryRows(
    ctx: SettingsCtx,
    store: CategoryStore,
    list: List<CustomCategoryDto>,
    reordering: Boolean,
    onReorderingChange: (Boolean) -> Unit,
    onEdit: (CustomCategoryDto) -> Unit,
    onDeleteRequest: (CustomCategoryDto) -> Unit,
) {
    row {
        SettingRowBox(ctx, CATEGORIES_ROW_ID, CustomCategoryDto.PREFERENCE_KEY) {
            FluxListRow(
                title = str(R.string.customCategories),
                subtitle = str(R.string.categoryPriorityNote),
                cloud = ctx.synced(CustomCategoryDto.PREFERENCE_KEY),
                trailing = {
                    FluxButton(
                        text = str(if (reordering) R.string.mobileViewDone else R.string.mobileGeneralReorder),
                        onClick = { onReorderingChange(!reordering) },
                        variant = ButtonVariant.Ghost,
                        size = ButtonSize.Xs,
                        enabled = !ctx.readOnly,
                    )
                },
            )
        }
    }
    list.forEachIndexed { index, entry ->
        row(hasIcon = true) {
            CategoryRow(
                ctx = ctx,
                entry = entry,
                reordering = reordering,
                canMoveUp = index > 0,
                canMoveDown = index < list.lastIndex,
                onMove = { delta -> store.move(entry, delta) },
                onEdit = { onEdit(entry) },
                onDeleteRequest = { onDeleteRequest(entry) },
            )
        }
    }
}

@Composable
private fun CategoryRow(
    ctx: SettingsCtx,
    entry: CustomCategoryDto,
    reordering: Boolean,
    canMoveUp: Boolean,
    canMoveDown: Boolean,
    onMove: (Int) -> Unit,
    onEdit: () -> Unit,
    onDeleteRequest: () -> Unit,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val detail = categoryDetail(context, entry)
    val saveDir = androidx.compose.runtime.remember(entry.saveDir, ctx.isLocalHost) {
        entry.saveDir.takeIf { it.isNotEmpty() }?.let { if (ctx.isLocalHost) friendlyLocalPath(context, it) else it }
    }
    val subtitle = listOfNotNull(detail, saveDir).joinToString("\n").ifEmpty { null }
    FluxListRow(
        title = categoryDisplayName(context, entry),
        subtitle = subtitle,
        icon = categoryIcon(entry.icon),
        iconTone = if (entry.isBuiltin) Tone.Neutral else Tone.Accent,
        trailing = {
            if (reordering) {
                Row {
                    FluxIconButton(
                        FluxIcons.ArrowUp, str(R.string.mobileGeneralMoveUp), { onMove(-1) },
                        size = IconButtonSize.Sm, enabled = canMoveUp && !ctx.readOnly,
                    )
                    FluxIconButton(
                        FluxIcons.ArrowDown, str(R.string.mobileGeneralMoveDown), { onMove(1) },
                        size = IconButtonSize.Sm, enabled = canMoveDown && !ctx.readOnly,
                    )
                }
            } else {
                FluxTag(
                    text = str(if (entry.isBuiltin) R.string.builtinCategory else R.string.customCategory),
                    tone = if (entry.isBuiltin) Tone.Neutral else Tone.Accent,
                )
            }
        },
        chevron = !reordering,
        enabled = !ctx.readOnly,
        onClick = if (reordering) null else onEdit,
        onLongClick = if (reordering || entry.isBuiltin) null else onDeleteRequest,
    )
}
