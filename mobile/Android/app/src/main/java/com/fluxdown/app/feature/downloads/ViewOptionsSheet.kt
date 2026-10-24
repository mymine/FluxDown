package com.fluxdown.app.feature.downloads

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.actions.LocalTaskActions
import com.fluxdown.app.data.CardField
import com.fluxdown.app.data.Density
import com.fluxdown.app.data.GroupBy
import com.fluxdown.app.data.SortKey
import com.fluxdown.app.data.ViewPrefs
import com.fluxdown.app.nav.LocalNavigator
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.app.ui.label
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.TaskStatus
import com.fluxdown.fluxui.chrome.FluxPill
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.SegOption
import com.fluxdown.fluxui.controls.FluxSegmented
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetDetent
import com.fluxdown.fluxui.overlay.FluxSheetFooter
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.launch

private fun GroupBy.labelRes() = when (this) {
    GroupBy.None -> R.string.viewGroupNone
    GroupBy.Status -> R.string.viewGroupStatus
    GroupBy.Date -> R.string.viewGroupDate
    GroupBy.Type -> R.string.viewGroupType
    GroupBy.Queue -> R.string.viewGroupQueue
    GroupBy.Site -> R.string.viewGroupSite
    GroupBy.Group -> R.string.viewGroupGroup
}

private fun SortKey.labelRes() = when (this) {
    SortKey.Smart -> R.string.viewSortSmart
    SortKey.Created -> R.string.viewSortCreated
    SortKey.Name -> R.string.viewSortName
    SortKey.Size -> R.string.viewSortSize
    SortKey.Progress -> R.string.viewSortProgress
    SortKey.Speed -> R.string.viewSortSpeed
    SortKey.Status -> R.string.viewSortStatus
}

private fun CardField.labelRes() = when (this) {
    CardField.Size -> R.string.colSize
    CardField.Speed -> R.string.colSpeed
    CardField.Eta -> R.string.colEta
    CardField.Protocol -> R.string.colProtocol
    CardField.Site -> R.string.colSource
    CardField.Queue -> R.string.colQueue
    CardField.Created -> R.string.colCreated
}

/** D1v 视图 Sheet：范围 · 分组 · 排序 · 密度 · 卡片字段 · 任务动作；偏好经 [DownloadsView.updatePrefs] 持久化。 */
@Composable
fun ViewOptionsSheet(visible: Boolean, onDismiss: () -> Unit) {
    val title = stringResource(R.string.viewMenuLabel)
    val sub = stringResource(R.string.mobileViewSheetSub)
    FluxSheet(
        visible = visible,
        onDismissRequest = onDismiss,
        detent = FluxSheetDetent.Full,
        title = title,
        header = { FluxSheetHeader(title = title, subtitle = sub, onClose = onDismiss) },
        footer = { ViewFooter(onDismiss) },
    ) {
        ViewOptionsBody(onDismiss)
    }
}

@Composable
private fun ViewFooter(onDismiss: () -> Unit) {
    val view = LocalDownloadsView.current
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val resetToast = stringResource(R.string.viewResetToast)
    FluxSheetFooter {
        FluxButton(
            text = stringResource(R.string.viewResetDefault),
            onClick = {
                view.updatePrefs(ViewPrefs())
                if (view.filter.queueId != null) view.setQueue(null)
                haptics.confirm()
                overlays.toast(resetToast, FluxToastKind.Success)
            },
            modifier = Modifier.weight(1f),
            variant = ButtonVariant.Secondary,
            icon = FluxIcons.RotateCcw,
        )
        FluxButton(
            text = stringResource(R.string.mobileViewDone),
            onClick = onDismiss,
            modifier = Modifier.weight(1f),
            variant = ButtonVariant.Primary,
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ColumnScope.ViewOptionsBody(onDismiss: () -> Unit) {
    val view = LocalDownloadsView.current
    val nav = LocalNavigator.current
    val actions = LocalTaskActions.current
    val haptics = FluxTheme.haptics
    val prefs = view.prefs ?: ViewPrefs()
    val filter = view.filter
    val queues = view.facets.queues

    // 范围：队列（仅有多个队列、且 `ui.show_sidebar_queues` 开启时）
    if (queues.size > 1 && view.filterVisibility.queues) {
        GlassSection(title = stringResource(R.string.mobileViewScope)) {
            custom(padded = true) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    FluxPill(
                        text = stringResource(R.string.mobileViewAllQueues),
                        selected = filter.queueId == null,
                        onClick = {
                            haptics.tick()
                            view.setQueue(null)
                        },
                    )
                    for (qf in queues) {
                        val selected = filter.queueId == normQueue(qf.queue.queueId)
                        FluxPill(
                            text = qf.queue.label(),
                            selected = selected,
                            onClick = {
                                haptics.tick()
                                if (!selected) view.setQueue(qf.queue.queueId)
                            },
                            count = qf.count,
                        )
                    }
                }
            }
        }
    }

    // 分组
    GlassSection(title = stringResource(R.string.viewSectionGroupBy)) {
        custom(padded = true) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                for (g in GroupBy.entries) {
                    FluxPill(
                        text = stringResource(g.labelRes()),
                        selected = prefs.groupBy == g,
                        onClick = {
                            haptics.tick()
                            view.updatePrefs(prefs.copy(groupBy = g))
                        },
                    )
                }
            }
        }
    }

    // 排序 + 方向
    GlassSection(
        title = stringResource(R.string.viewSectionSort),
        footer = stringResource(if (prefs.sortKey == SortKey.Smart) R.string.mobileViewSortNoteSmart else R.string.mobileViewSortNoteReset),
    ) {
        custom(padded = true) {
            androidx.compose.foundation.layout.Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (k in SortKey.entries) {
                        FluxPill(
                            text = stringResource(k.labelRes()),
                            selected = prefs.sortKey == k,
                            onClick = {
                                haptics.tick()
                                // 切换排序键：方向重置为该键的默认方向（名称升序，其余降序）
                                if (prefs.sortKey != k) view.updatePrefs(prefs.copy(sortKey = k, ascending = k == SortKey.Name))
                            },
                        )
                    }
                }
                FluxSegmented(
                    options = listOf(
                        SegOption(true, stringResource(R.string.viewSortAscending), FluxIcons.SortAsc),
                        SegOption(false, stringResource(R.string.viewSortDescending), FluxIcons.SortDesc),
                    ),
                    selected = prefs.ascending,
                    onSelect = { view.updatePrefs(prefs.copy(ascending = it)) },
                )
            }
        }
    }

    // 密度
    GlassSection(title = stringResource(R.string.viewSectionDensity)) {
        custom(padded = true) {
            FluxSegmented(
                options = listOf(
                    SegOption(Density.Comfortable, stringResource(R.string.viewDensityComfortable)),
                    SegOption(Density.Compact, stringResource(R.string.viewDensityCompact)),
                ),
                selected = prefs.density,
                onSelect = { view.updatePrefs(prefs.copy(density = it)) },
            )
        }
    }

    // 卡片字段（多选，至少保留 1 项）
    GlassSection(title = stringResource(R.string.mobileViewFields), footer = stringResource(R.string.mobileViewFieldsNote)) {
        custom(padded = true) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                for (f in CardField.entries) {
                    val selected = f in prefs.fields
                    FluxPill(
                        text = stringResource(f.labelRes()),
                        selected = selected,
                        onClick = {
                            if (selected && prefs.fields.size == 1) {
                                haptics.reject()
                            } else {
                                haptics.tick()
                                val next = if (selected) prefs.fields - f else prefs.fields + f
                                view.updatePrefs(prefs.copy(fields = CardField.entries.filterTo(linkedSetOf()) { it in next }))
                            }
                        },
                    )
                }
            }
        }
    }

    // 任务动作
    GlassSection(title = stringResource(R.string.mobileViewTasks)) {
        row(hasIcon = true) {
            FluxListRow(
                title = stringResource(R.string.mobileViewSelectTasks),
                icon = FluxIcons.ListChecks,
                onClick = {
                    onDismiss()
                    nav.enterSelection(view.list.visibleIds.firstOrNull())
                },
            )
        }
        row(hasIcon = true) {
            FluxListRow(
                title = stringResource(R.string.pauseAll),
                icon = FluxIcons.Pause,
                onClick = {
                    onDismiss()
                    actions.pauseAll()
                },
            )
        }
        row(hasIcon = true) {
            FluxListRow(
                title = stringResource(R.string.resumeAll),
                icon = FluxIcons.Play,
                onClick = {
                    onDismiss()
                    actions.resumeAll()
                },
            )
        }
        row(hasIcon = true) {
            val clear = rememberClearFinished(onDismiss)
            FluxListRow(
                title = stringResource(R.string.mobileViewClearFinished),
                subtitle = stringResource(R.string.mobileViewClearFinishedSub),
                icon = FluxIcons.CheckCheck,
                onClick = clear,
            )
        }
    }
}

/** 清除已完成任务：确认 → `deleteMany(delete_files = false)`（只移除记录，保留文件）。 */
@Composable
private fun rememberClearFinished(onDismiss: () -> Unit): () -> Unit {
    val container = LocalAppContainer.current
    val overlays = LocalFluxOverlays.current
    val actions = LocalTaskActions.current
    val haptics = FluxTheme.haptics
    val none = stringResource(R.string.mobileClearFinishedNone)
    val title = stringResource(R.string.mobileClearFinishedTitle)
    val cancel = stringResource(R.string.cancel)
    val disconnected = stringResource(R.string.localServiceDisconnected)
    val loadingText = stringResource(R.string.mobileTasksLoading)
    val messageFmt = stringResource(R.string.mobileClearFinishedMessage)
    val actionFmt = stringResource(R.string.mobileClearFinishedAction)
    val toastFmt = stringResource(R.string.mobileToastClearedFinished)
    return {
        onDismiss()
        val state = container.store.state.value
        val ids = state.tasks.filter { it.status == TaskStatus.Completed }.map { it.taskId }
        when {
            state.isReadOnly -> {
                haptics.reject()
                overlays.toast(disconnected, FluxToastKind.Error, FluxIcons.WifiOff)
            }
            // 任务表还在载入：ids 为空不代表“没有已完成任务”。
            state.tasksLoading -> overlays.toast(loadingText, FluxToastKind.Info)
            ids.isEmpty() -> overlays.toast(none, FluxToastKind.Info)
            else -> overlays.showDialog(
                FluxDialogSpec(
                    title = title,
                    message = messageFmt.replace("{n}", ids.size.toString()),
                    icon = FluxIcons.CheckCheck,
                    buttons = listOf(
                        FluxDialogButton(cancel),
                        FluxDialogButton(actionFmt.replace("{n}", ids.size.toString()), FluxDialogButtonStyle.Destructive) {
                            container.appScope.launch {
                                try {
                                    container.session.deleteMany(ids, deleteFiles = false)
                                    haptics.confirm()
                                    overlays.toast(toastFmt.replace("{n}", ids.size.toString()), FluxToastKind.Success)
                                } catch (e: HostException) {
                                    haptics.reject()
                                    overlays.toast(actions.errorText(e), FluxToastKind.Error)
                                }
                            }
                        },
                    ),
                ),
            )
        }
    }
}
