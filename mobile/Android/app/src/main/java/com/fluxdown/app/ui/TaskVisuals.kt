package com.fluxdown.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import com.fluxdown.app.R
import com.fluxdown.core.model.Category
import com.fluxdown.core.model.Queue
import com.fluxdown.core.model.Task
import com.fluxdown.core.model.TaskProtocol
import com.fluxdown.core.model.TaskStatus
import com.fluxdown.fluxui.data.RingKind
import com.fluxdown.fluxui.data.fileTileIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.theme.FileCategory

/**
 * 跨特性共享的任务视觉映射（列表行、详情英雄头、搜索结果、选择请求里的任务行同一套规则）。
 */

/** 分类 → 色点槽位。自定义分类统一用 Ebook 色（青），其余按内置类型。 */
fun Category?.fileCategory(): FileCategory = when (this?.builtinType) {
    "video" -> FileCategory.Video
    "audio" -> FileCategory.Audio
    "document" -> FileCategory.Document
    "image" -> FileCategory.Image
    "program" -> FileCategory.Program
    "archive" -> FileCategory.Archive
    null -> if (this == null) FileCategory.Other else FileCategory.Ebook
    else -> FileCategory.Other
}

/** 图块图标：分类图标；BT 且分类为 other 时用 magnet。自定义分类按其 icon 名映射。 */
fun Category?.tileIcon(task: Task): ImageVector = tileIcon(bt = task.protocol == TaskProtocol.Bt)

fun Category?.tileIcon(bt: Boolean): ImageVector {
    if (this != null && builtinType == null) {
        return when (icon) {
            "library", "book" -> FluxIcons.Library
            "film" -> FluxIcons.Film
            "music" -> FluxIcons.Music
            "image" -> FluxIcons.Image
            "archive" -> FluxIcons.Archive
            "cpu" -> FluxIcons.Cpu
            "fileText" -> FluxIcons.FileText
            else -> FluxIcons.File
        }
    }
    return fileTileIcon(fileCategory(), bt)
}

/** 分类显示名：内置走 i18n（App / GPUI / Web 共用键），自定义用其名称。 */
@Composable
fun Category.label(): String = when (builtinType) {
    "all" -> stringResource(R.string.categoryAll)
    "video" -> stringResource(R.string.categoryVideo)
    "audio" -> stringResource(R.string.categoryAudio)
    "document" -> stringResource(R.string.categoryDocument)
    "image" -> stringResource(R.string.categoryImage)
    "program" -> stringResource(R.string.categoryProgram)
    "archive" -> stringResource(R.string.categoryArchive)
    "other" -> stringResource(R.string.categoryOther)
    else -> name
}

/** 内置队列名走 i18n；用户队列用其名称。 */
@Composable
fun Queue.label(): String = when (queueId) {
    "", Queue.MAIN -> stringResource(R.string.mainQueue)
    Queue.LATER -> stringResource(R.string.downloadLater)
    else -> name
}

/** 派生显示状态（§2.6：颜色 + 图形冗余）。 */
enum class TaskVisualState { Downloading, Queued, Pending, Preparing, Verifying, Paused, Failed, Seeding, Missing, Completed }

/** [queued]：任务在引擎排队顺序里（有排序键）。 */
fun Task.visualState(queued: Boolean): TaskVisualState = when (status) {
    TaskStatus.Downloading -> TaskVisualState.Downloading
    TaskStatus.Pending -> if (queued) TaskVisualState.Queued else TaskVisualState.Pending
    TaskStatus.Preparing -> if (totalBytes > 0) TaskVisualState.Verifying else TaskVisualState.Preparing
    TaskStatus.Paused -> TaskVisualState.Paused
    TaskStatus.Failed -> TaskVisualState.Failed
    TaskStatus.Completed -> when {
        fileMissing -> TaskVisualState.Missing
        isSeeding -> TaskVisualState.Seeding
        else -> TaskVisualState.Completed
    }
    TaskStatus.Unknown -> TaskVisualState.Pending
}

/** 行尾环钮字形（数据可视化组件约定的映射）。 */
fun TaskVisualState.ringKind(): RingKind = when (this) {
    TaskVisualState.Downloading, TaskVisualState.Pending -> RingKind.Pause
    TaskVisualState.Queued -> RingKind.Queued
    TaskVisualState.Preparing, TaskVisualState.Verifying -> RingKind.Spinning
    TaskVisualState.Paused -> RingKind.Play
    TaskVisualState.Failed -> RingKind.Retry
    TaskVisualState.Missing -> RingKind.Redownload
    TaskVisualState.Seeding -> RingKind.Seeding
    TaskVisualState.Completed -> RingKind.Open
}
