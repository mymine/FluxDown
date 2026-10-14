package com.fluxdown.app.feature.settings.general

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import com.fluxdown.app.FeatureFlags
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.SettingRowBox
import com.fluxdown.app.feature.settings.SettingsCtx
import com.fluxdown.app.feature.settings.SettingsEntry
import com.fluxdown.app.feature.settings.SettingsPageFrame
import com.fluxdown.app.feature.settings.SettingsSearchContext
import com.fluxdown.app.feature.settings.flowItem
import com.fluxdown.app.feature.settings.rememberSettingsCtx
import com.fluxdown.app.feature.settings.settingSwitch
import com.fluxdown.app.feature.settings.startSettingsIntent
import com.fluxdown.app.i18n.str
import com.fluxdown.app.nav.LocalNavigator
import com.fluxdown.app.nav.SettingsPage
import com.fluxdown.core.protocol.CustomCategoryDto
import com.fluxdown.fluxui.controls.FluxActionRow
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxSectionFoot
import com.fluxdown.fluxui.controls.FluxTag
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.GlassSectionScope
import com.fluxdown.fluxui.controls.Tone
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.rememberFlowInGate
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.LocalFluxOverlays

// ───────────────────────────── 行目录（页面渲染与设置搜索共用） ─────────────────────────────

/** 通用页的分组；[key] = 该分组所在 `LazyColumn` 项的 key（搜索据此滚动定位）。 */
internal enum class GeneralGroup(val key: String, @StringRes val title: Int) {
    Background("background", R.string.mobileGeneralGroupBackground),
    System("system", R.string.settingsGroupSystem),
    DownloadsView("downloadsView", R.string.mobileGeneralGroupDownloadsView),
    Entries("entries", R.string.mobileGeneralGroupEntries),
    Categories("categories", R.string.customCategories),
}

/** 通用页的每一行；行 id 与 iOS `GeneralRow` 一致（`general.<slug>`）。[key] = 目录键（agent 偏好），无则为设备 / 只读 / 列表行。 */
internal enum class GeneralRow(
    slug: String,
    val group: GeneralGroup,
    @StringRes val title: Int,
    @StringRes val detail: Int?,
    val key: String?,
    val icon: ImageVector,
) {
    KeepAwake("keepAwake", GeneralGroup.Background, R.string.keepAwakeWhileDownloading, R.string.mobileGeneralKeepAwakeDesc, "download.keep_awake", FluxIcons.Sun),
    BatteryOptimization("batteryOptimization", GeneralGroup.Background, R.string.mobileGeneralBatteryOpt, R.string.mobileGeneralBatteryOptDesc, null, FluxIcons.BatteryCharging),
    Analytics("analytics", GeneralGroup.System, R.string.analyticsEnabled, R.string.analyticsEnabledDesc, "analytics_enabled", FluxIcons.Activity),
    LinkHandling("linkHandling", GeneralGroup.System, R.string.mobileGeneralLinkHandling, R.string.mobileGeneralLinkHandlingDescAndroid, null, FluxIcons.Link),
    SidebarStatus("sidebarStatus", GeneralGroup.DownloadsView, R.string.showSidebarStatus, null, "ui.show_sidebar_status", FluxIcons.Layers),
    SidebarQueues("sidebarQueues", GeneralGroup.DownloadsView, R.string.showSidebarQueues, null, "ui.show_sidebar_queues", FluxIcons.Rows3),
    SidebarCategory("sidebarCategory", GeneralGroup.DownloadsView, R.string.showSidebarCategory, R.string.showSidebarCategoryNestedDesc, "ui.show_sidebar_category", FluxIcons.Folders),
    ActivityRss("activityRss", GeneralGroup.Entries, R.string.showActivityRss, null, "ui.show_activity_rss", FluxIcons.Rss),
    ActivityWebhooks("activityWebhooks", GeneralGroup.Entries, R.string.showActivityWebhooks, null, "ui.show_activity_webhooks", FluxIcons.Webhook),
    ActivityTheme("activityTheme", GeneralGroup.Entries, R.string.showActivityTheme, null, "ui.show_activity_theme", FluxIcons.Palette),
    Categories("categories", GeneralGroup.Categories, R.string.customCategories, R.string.categoryPriorityNote, CustomCategoryDto.PREFERENCE_KEY, FluxIcons.Folder);

    val id: String = "general.$slug"
}

private fun GlassSectionScope.toggleRow(ctx: SettingsCtx, row: GeneralRow) {
    val key = row.key ?: return
    settingSwitch(ctx, key, row.title, row.detail, id = row.id)
}

// ───────────────────────────── 页面 ─────────────────────────────

/**
 * S3 · 通用。分组（同 iOS）：后台下载 → 系统 → 下载页显示 → 入口 → 自定义分类。
 * 偏好键一律经 `ConfigEditor` 写入（agent 偏好，☁︎ 键同步）。iOS 的「离开时继续下载」是 iOS 后台任务的开关；
 * Android 本机下载靠前台服务常驻，没有对应的可调行为，因此改为电池优化入口。
 */
@Composable
internal fun GeneralPage() {
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val ctx = rememberSettingsCtx(openPicker = {})
    val gate = rememberFlowInGate()
    var editing by remember { mutableStateOf<CategoryTarget?>(null) }
    var reordering by rememberSaveable { mutableStateOf(false) }
    val store = remember(ctx, overlays, context) { CategoryStore(ctx, overlays, context) }
    val list = store.list
    val hasBase = store.defaultSaveDir.isNotEmpty()

    fun confirmDelete(entry: CustomCategoryDto) {
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.deleteCategory),
                message = context.str(R.string.deleteCategoryConfirm),
                icon = FluxIcons.Trash2,
                buttons = listOf(
                    FluxDialogButton(context.str(R.string.cancel)),
                    FluxDialogButton(context.str(R.string.delete), FluxDialogButtonStyle.Destructive) { store.delete(entry) },
                ),
            ),
        )
    }

    fun confirmReset() {
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.resetBuiltinCategories),
                message = context.str(R.string.resetAllCategoriesConfirm),
                icon = FluxIcons.RotateCcw,
                buttons = listOf(
                    FluxDialogButton(context.str(R.string.cancel)),
                    FluxDialogButton(context.str(R.string.resetBuiltinCategories), FluxDialogButtonStyle.Destructive) { store.resetBuiltin() },
                ),
            ),
        )
    }

    Box(Modifier.fillMaxSize()) {
        SettingsPageFrame(title = str(R.string.settingsCatGeneral), onBack = { nav.pop() }) {
            var index = 0
            if (ctx.readOnly) {
                flowItem(index++, gate, "banner") {
                    FluxBanner(str(R.string.localServiceDisconnected), kind = FluxBannerKind.Warn, slim = true)
                }
            }
            flowItem(index++, gate, GeneralGroup.Background.key) {
                GlassSection(
                    title = str(GeneralGroup.Background.title),
                    footer = str(R.string.mobileGeneralBgFootnote),
                ) {
                    toggleRow(ctx, GeneralRow.KeepAwake)
                    row { BatteryOptimizationRow(ctx) }
                }
            }
            flowItem(index++, gate, GeneralGroup.System.key) {
                GlassSection(title = str(GeneralGroup.System.title)) {
                    toggleRow(ctx, GeneralRow.Analytics)
                    linkHandlingRows(ctx)
                }
            }
            flowItem(index++, gate, GeneralGroup.DownloadsView.key) {
                GlassSection(
                    title = str(GeneralGroup.DownloadsView.title),
                    footer = str(R.string.mobileGeneralDownloadsViewFooterAndroid),
                ) {
                    toggleRow(ctx, GeneralRow.SidebarStatus)
                    toggleRow(ctx, GeneralRow.SidebarQueues)
                    toggleRow(ctx, GeneralRow.SidebarCategory)
                }
            }
            flowItem(index++, gate, GeneralGroup.Entries.key) {
                GlassSection(
                    title = str(GeneralGroup.Entries.title),
                    footer = str(R.string.mobileGeneralEntriesFooter),
                ) {
                    toggleRow(ctx, GeneralRow.ActivityRss)
                    if (FeatureFlags.WEBHOOKS) toggleRow(ctx, GeneralRow.ActivityWebhooks)
                    toggleRow(ctx, GeneralRow.ActivityTheme)
                }
            }
            flowItem(index++, gate, GeneralGroup.Categories.key) {
                GlassSection {
                    categoryRows(
                        ctx = ctx,
                        store = store,
                        list = list,
                        reordering = reordering,
                        onReorderingChange = { reordering = it },
                        onEdit = { editing = CategoryTarget(it) },
                        onDeleteRequest = ::confirmDelete,
                    )
                }
            }
            flowItem(index++, gate, "categoryActions") {
                GlassSection(footer = if (hasBase) null else str(R.string.mobileGeneralAutoDirsNeedDefault)) {
                    row(hasIcon = true) {
                        FluxActionRow(
                            title = str(R.string.addCategory),
                            onClick = { editing = CategoryTarget(null) },
                            icon = FluxIcons.Plus,
                            enabled = !ctx.readOnly,
                        )
                    }
                    row(hasIcon = true) {
                        FluxActionRow(
                            title = str(R.string.autoCategoryDirs),
                            onClick = { store.applyAutoDirs() },
                            icon = FluxIcons.FolderOpen,
                            enabled = hasBase && !ctx.readOnly,
                        )
                    }
                    row(hasIcon = true) {
                        FluxActionRow(
                            title = str(R.string.resetBuiltinCategories),
                            onClick = ::confirmReset,
                            tone = Tone.Coral,
                            icon = FluxIcons.RotateCcw,
                            enabled = !ctx.readOnly,
                        )
                    }
                }
            }
            flowItem(index++, gate, "legend") {
                FluxSectionFoot(str(R.string.settingsSyncLegend))
            }
        }
        CategoryEditorSheet(
            target = editing,
            isLocalHost = ctx.isLocalHost,
            onSave = { store.save(it) },
            onDelete = { store.delete(it) },
            onDismiss = { editing = null },
        )
    }
}

// ───────────────────────────── 后台：电池优化 ─────────────────────────────

private fun isIgnoringBatteryOptimizations(context: Context): Boolean =
    context.getSystemService(PowerManager::class.java)?.isIgnoringBatteryOptimizations(context.packageName) == true

/** 电池优化：不受限制时息屏后下载不会被系统中断；点按打开系统的电池优化列表。 */
@Composable
private fun BatteryOptimizationRow(ctx: SettingsCtx) {
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    var unrestricted by remember { mutableStateOf(isIgnoringBatteryOptimizations(context)) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { unrestricted = isIgnoringBatteryOptimizations(context) }
    val noApp = str(R.string.mobileNoSettingsApp)
    SettingRowBox(ctx, GeneralRow.BatteryOptimization.id, null) {
        FluxListRow(
            title = str(GeneralRow.BatteryOptimization.title),
            subtitle = str(R.string.mobileGeneralBatteryOptDesc),
            value = str(if (unrestricted) R.string.mobileGeneralBatteryUnrestricted else R.string.mobileGeneralBatteryOptimized),
            chevron = true,
            onClick = {
                startSettingsIntent(context, overlays, Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS), noApp)
            },
        )
    }
}

// ───────────────────────────── 系统：链接与文件打开方式 ─────────────────────────────

/** 本应用 Manifest 里声明的链接协议（专有名词，不本地化）。 */
private enum class LinkKind(val label: String, val sample: String) {
    Magnet("magnet:", "magnet:?xt=urn:btih:0000000000000000000000000000000000000000"),
    Ed2k("ed2k://", "ed2k://|file|a.bin|1|00000000000000000000000000000000|/"),
}

/** 本应用是否声明了处理该链接的 Activity（读 Manifest 的静态声明；系统默认应用的选择权在用户，应用无法检测）。 */
@Suppress("DEPRECATION")
private fun declares(context: Context, kind: LinkKind): Boolean {
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(kind.sample))
        .addCategory(Intent.CATEGORY_BROWSABLE)
        .setPackage(context.packageName)
    val packages = context.packageManager
    val handlers = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        packages.queryIntentActivities(intent, PackageManager.ResolveInfoFlags.of(0))
    } else {
        packages.queryIntentActivities(intent, 0)
    }
    return handlers.isNotEmpty()
}

private fun GlassSectionScope.linkHandlingRows(ctx: SettingsCtx) {
    row {
        SettingRowBox(ctx, GeneralRow.LinkHandling.id, null) {
            FluxListRow(
                title = str(GeneralRow.LinkHandling.title),
                subtitle = str(R.string.mobileGeneralLinkHandlingDescAndroid),
            )
        }
    }
    for (kind in LinkKind.entries) {
        row {
            val context = LocalContext.current
            val declared = remember(kind, context) { declares(context, kind) }
            FluxListRow(
                title = kind.label,
                trailing = {
                    FluxTag(
                        text = str(if (declared) R.string.mobileGeneralLinkDeclared else R.string.mobileGeneralLinkNotDeclared),
                        tone = if (declared) Tone.Mint else Tone.Amber,
                        icon = if (declared) FluxIcons.CircleCheck else FluxIcons.TriangleAlert,
                    )
                },
            )
        }
    }
    row {
        val context = LocalContext.current
        val overlays = LocalFluxOverlays.current
        val noApp = str(R.string.mobileNoSettingsApp)
        FluxListRow(
            title = str(R.string.mobileGeneralOpenByDefault),
            subtitle = str(R.string.mobileGeneralOpenByDefaultDesc),
            chevron = true,
            onClick = {
                val pkg = Uri.parse("package:${context.packageName}")
                // 「默认打开」独立设置页为 Android 12 新增；更低版本该入口位于应用详情页
                val action = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    Settings.ACTION_APP_OPEN_BY_DEFAULT_SETTINGS
                } else {
                    Settings.ACTION_APPLICATION_DETAILS_SETTINGS
                }
                startSettingsIntent(context, overlays, Intent(action, pkg), noApp)
            },
        )
    }
}

// ───────────────────────────── 搜索 / 读数 ─────────────────────────────

/** 设置搜索索引：本页所有行（偏好键恒可用，没有条件隐藏的行，与页面渲染一致）。 */
internal fun generalSearchEntries(ctx: SettingsSearchContext): List<SettingsEntry> =
    GeneralRow.entries.filter { FeatureFlags.WEBHOOKS || it != GeneralRow.ActivityWebhooks }.map { row ->
        ctx.entry(
            id = row.id,
            page = SettingsPage.General,
            itemKey = row.group.key,
            title = row.title,
            detail = row.detail,
            breadcrumb = ctx.crumb(R.string.settingsCatGeneral, row.group.title),
            icon = row.icon,
        )
    }

/** 设置首页读数：分类条数（偏好尚未下发时无读数，显示分类说明）。 */
internal fun generalReadout(ctx: SettingsSearchContext): String? {
    val pref = ctx.form.pref(CustomCategoryDto.PREFERENCE_KEY) ?: return null
    return ctx.str(R.string.mobileGeneralReadoutCategories, "n" to CustomCategoryDto.fromPreference(pref).size)
}
