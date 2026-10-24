package com.fluxdown.app.feature.settings

import android.content.Context
import androidx.annotation.StringRes
import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.vector.ImageVector
import com.fluxdown.app.FeatureFlags
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.account.accountSearchEntries
import com.fluxdown.app.feature.settings.bt.btSearchEntries
import com.fluxdown.app.feature.settings.bt.ed2kSearchEntries
import com.fluxdown.app.feature.settings.diagnostics.diagnosticsSearchEntries
import com.fluxdown.app.feature.settings.extensions.extensionsSearchEntries
import com.fluxdown.app.feature.settings.general.generalSearchEntries
import com.fluxdown.app.feature.settings.general.notifySearchEntries
import com.fluxdown.app.feature.settings.network.networkSearchEntries
import com.fluxdown.app.feature.settings.service.apiServiceSearchEntries
import com.fluxdown.app.feature.settings.service.webhookSearchEntries
import com.fluxdown.app.i18n.str
import com.fluxdown.app.nav.SettingsPage
import com.fluxdown.core.protocol.HostCapability
import com.fluxdown.core.protocol.SettingsForm
import com.fluxdown.core.protocol.has
import com.fluxdown.core.protocol.preferences
import com.fluxdown.core.store.HostState
import com.fluxdown.fluxui.icons.FluxIcons
import java.text.Normalizer
import java.util.Locale

/**
 * 搜索索引项（设置首页搜索与全局命令搜索共用）。
 * - [id]：行标识（如 `download.maxConcurrent`），命中后目标行经 [settingsFocus] 脉冲高亮；分类入口为 `category.<page>`。
 * - [itemKey]：该行所在 `LazyColumn` 项的 key（[SettingsPageFrame] 据此滚动），分类入口为 null。
 * - [keywords]：PC 命令面板的别名词表（`searchKeywords…`），只做子串匹配、排在标题 / 说明之后。
 */
@Immutable
internal data class SettingsEntry(
    val id: String,
    val page: SettingsPage,
    val itemKey: String?,
    val title: String,
    val detail: String,
    val breadcrumb: String,
    val icon: ImageVector,
    val keywords: List<String> = emptyList(),
)

/**
 * 各设置页向索引贡献条目时的上下文：与页面渲染共用同一份可见性判定，
 * 因此被条件隐藏 / 能力缺失 / 桌面专属的行不会出现在结果里。
 */
internal class SettingsSearchContext(
    private val context: Context,
    val state: HostState,
    val form: SettingsForm,
    val isLocalHost: Boolean,
) {
    fun has(capability: String): Boolean = state.has(capability)

    fun str(@StringRes id: Int, vararg args: Pair<String, Any?>): String = context.str(id, *args)

    /** 便捷构造：`breadcrumb` 如「下载 › 连接与性能」。 */
    fun entry(
        id: String,
        page: SettingsPage,
        itemKey: String?,
        @StringRes title: Int,
        @StringRes detail: Int?,
        breadcrumb: String,
        icon: ImageVector,
    ) = SettingsEntry(id, page, itemKey, str(title), detail?.let { str(it) }.orEmpty(), breadcrumb, icon)

    /** `分类 › 分组` 面包屑。 */
    fun crumb(@StringRes page: Int, @StringRes group: Int? = null): String =
        if (group == null) str(page) else str(page) + " › " + str(group)
}

/** 设置首页可见的分类（顺序 = 首页 / 搜索顺序，同 iOS `SettingsStack`）。 */
internal fun visibleSettingsPages(state: HostState, isLocalHost: Boolean): List<SettingsPage> = buildList {
    if (state.has(HostCapability.agentAuth)) add(SettingsPage.Account)
    add(SettingsPage.General)
    add(SettingsPage.Appearance)
    add(SettingsPage.Notify)
    add(SettingsPage.Download)
    add(SettingsPage.Bt)
    add(SettingsPage.Ed2k)
    add(SettingsPage.Network)
    add(SettingsPage.Extensions)
    // 功能开关关闭时无入口；开启后 `ui.show_activity_webhooks` = false 仍可隐藏首页行（搜索仍可进入）。
    if (FeatureFlags.WEBHOOKS && state.preferences.bool("ui.show_activity_webhooks", true)) add(SettingsPage.Webhook)
    if (!isLocalHost && state.has(HostCapability.agentGateway)) add(SettingsPage.Api)
    add(SettingsPage.Diagnostics)
    add(SettingsPage.About)
}

/** 分类的标题 / 说明 / 图标（首页行与搜索入口共用）。 */
internal class SettingsCategoryMeta(@StringRes val title: Int, @StringRes val desc: Int?, val icon: ImageVector)

internal fun SettingsPage.meta(): SettingsCategoryMeta = when (this) {
    SettingsPage.Account -> SettingsCategoryMeta(R.string.settingsCatAccount, R.string.settingsCatAccountDesc, FluxIcons.CircleUser)
    SettingsPage.General -> SettingsCategoryMeta(R.string.settingsCatGeneral, R.string.settingsCatGeneralDesc, FluxIcons.Settings)
    SettingsPage.Appearance -> SettingsCategoryMeta(R.string.settingsCatAppearance, R.string.settingsCatAppearanceDesc, FluxIcons.Palette)
    SettingsPage.Notify -> SettingsCategoryMeta(R.string.settingsCatNotify, R.string.settingsCatNotifyDesc, FluxIcons.Bell)
    SettingsPage.Download -> SettingsCategoryMeta(R.string.settingsCatDownload, R.string.settingsCatDownloadDesc, FluxIcons.Download)
    SettingsPage.Bt -> SettingsCategoryMeta(R.string.settingsCatBt, R.string.settingsCatBtDesc, FluxIcons.Magnet)
    SettingsPage.Ed2k -> SettingsCategoryMeta(R.string.ed2kSettings, R.string.ed2kSettingsDesc, FluxIcons.Server)
    SettingsPage.Network -> SettingsCategoryMeta(R.string.settingsCatProxy, R.string.settingsCatProxyDesc, FluxIcons.Globe)
    SettingsPage.Extensions -> SettingsCategoryMeta(R.string.settingsCatExtensions, R.string.settingsCatExtensionsDesc, FluxIcons.Puzzle)
    SettingsPage.PluginMarket -> SettingsCategoryMeta(R.string.settingsCatExtensions, null, FluxIcons.Puzzle)
    SettingsPage.PluginDetail -> SettingsCategoryMeta(R.string.settingsCatExtensions, null, FluxIcons.Puzzle)
    SettingsPage.Webhook -> SettingsCategoryMeta(R.string.webhookNavTitle, null, FluxIcons.Webhook)
    SettingsPage.Api -> SettingsCategoryMeta(R.string.settingsCatApiService, R.string.settingsCatApiServiceDesc, FluxIcons.Code)
    SettingsPage.Diagnostics -> SettingsCategoryMeta(R.string.settingsCatDoctor, R.string.settingsCatDoctorDesc, FluxIcons.Stethoscope)
    SettingsPage.About -> SettingsCategoryMeta(R.string.settingsCatAbout, R.string.settingsCatAboutDesc, FluxIcons.Info)
}

internal object SettingsIndex {
    /** 全部条目，顺序同首页分类顺序（分类入口在前，其下是该页的行）。 */
    fun entries(ctx: SettingsSearchContext): List<SettingsEntry> {
        val list = ArrayList<SettingsEntry>()
        // 搜索可进入 Webhook 页（即使首页隐藏），因此单独补上；功能开关关闭时不补。
        val pages = visibleSettingsPages(ctx.state, ctx.isLocalHost).let {
            if (!FeatureFlags.WEBHOOKS || SettingsPage.Webhook in it) it else it + SettingsPage.Webhook
        }
        for (page in pages.sortedBy { it.ordinal }) {
            list += category(ctx, page)
            list += when (page) {
                SettingsPage.Account -> accountSearchEntries(ctx)
                SettingsPage.General -> generalSearchEntries(ctx)
                SettingsPage.Appearance -> appearanceSearchEntries(ctx)
                SettingsPage.Notify -> notifySearchEntries(ctx)
                SettingsPage.Download -> downloadSearchEntries(ctx)
                SettingsPage.Bt -> btSearchEntries(ctx)
                SettingsPage.Ed2k -> ed2kSearchEntries(ctx)
                SettingsPage.Network -> networkSearchEntries(ctx)
                SettingsPage.Extensions -> extensionsSearchEntries(ctx)
                SettingsPage.Webhook -> webhookSearchEntries(ctx)
                SettingsPage.Api -> apiServiceSearchEntries(ctx)
                SettingsPage.Diagnostics -> diagnosticsSearchEntries(ctx)
                SettingsPage.About, SettingsPage.PluginMarket, SettingsPage.PluginDetail -> emptyList()
            }
        }
        return list.map { withKeywords(ctx, it) }
    }

    private fun category(ctx: SettingsSearchContext, page: SettingsPage): SettingsEntry {
        val meta = page.meta()
        return SettingsEntry(
            id = "category.${page.name}",
            page = page,
            itemKey = null,
            title = ctx.str(meta.title),
            detail = meta.desc?.let { ctx.str(it) }.orEmpty(),
            breadcrumb = ctx.str(R.string.settings),
            icon = meta.icon,
            keywords = categoryKeywordKeys[page].orEmpty().flatMap { words(ctx, it) },
        )
    }

    private fun withKeywords(ctx: SettingsSearchContext, entry: SettingsEntry): SettingsEntry {
        if (entry.keywords.isNotEmpty()) return entry
        val keys = rowKeywordKeys[entry.id] ?: return entry
        return entry.copy(keywords = keys.flatMap { words(ctx, it) })
    }

    private val categoryKeywordKeys: Map<SettingsPage, List<Int>> = mapOf(
        SettingsPage.General to listOf(R.string.searchKeywordsClipboardWatch),
        SettingsPage.Appearance to listOf(R.string.searchKeywordsThemeMode, R.string.searchKeywordsThemeColor),
        SettingsPage.Notify to listOf(R.string.searchKeywordsNotifyOnComplete),
        SettingsPage.Download to listOf(
            R.string.searchKeywordsSaveDir, R.string.searchKeywordsThreads,
            R.string.searchKeywordsConcurrent, R.string.searchKeywordsSpeedLimit,
        ),
        SettingsPage.Bt to listOf(R.string.searchKeywordsBtSettings),
        SettingsPage.Ed2k to listOf(R.string.searchKeywordsEd2kSettings),
        SettingsPage.Network to listOf(R.string.searchKeywordsProxy),
        SettingsPage.Webhook to listOf(R.string.searchKeywordsWebhook),
        SettingsPage.Api to listOf(R.string.searchKeywordsApiService),
        SettingsPage.Diagnostics to listOf(R.string.searchKeywordsDoctor),
        SettingsPage.About to listOf(R.string.searchKeywordsUpdate),
    )

    /** 行 id → PC 命令面板别名词表键（只列 PC 有别名的行；行 id 与 iOS 相同）。 */
    private val rowKeywordKeys: Map<String, List<Int>> = mapOf(
        "general.keepAwake" to listOf(R.string.searchKeywordsKeepAwake),
        "general.sidebarStatus" to listOf(R.string.searchKeywordsSidebarVisibility),
        "general.sidebarQueues" to listOf(R.string.searchKeywordsSidebarVisibility),
        "general.sidebarCategory" to listOf(R.string.searchKeywordsSidebarVisibility),
        "download.categories" to listOf(R.string.searchKeywordsCustomCategories),
        "appearance.language" to listOf(R.string.searchKeywordsLanguage),
        "appearance.mode" to listOf(R.string.searchKeywordsThemeMode),
        "appearance.color" to listOf(R.string.searchKeywordsThemeColor),
        "download.saveDir" to listOf(R.string.searchKeywordsSaveDir),
        "download.silentDownload" to listOf(R.string.searchKeywordsSilentDownload),
        "download.silentSkipSelection" to listOf(R.string.searchKeywordsSilentSkipSelection),
        "download.useServerTime" to listOf(R.string.searchKeywordsUseServerTime),
        "download.fileExistsBehavior" to listOf(R.string.searchKeywordsFileExists),
        "download.fileMissingAction" to listOf(R.string.searchKeywordsFileMissing),
        "download.defaultSegments" to listOf(R.string.searchKeywordsThreads),
        "download.cdnMulti" to listOf(R.string.searchKeywordsCdnMulti),
        "download.multiNic" to listOf(R.string.searchKeywordsMultiNic),
        "download.maxConcurrent" to listOf(R.string.searchKeywordsConcurrent),
        "download.speedLimit" to listOf(R.string.searchKeywordsSpeedLimit),
        "download.userAgent" to listOf(R.string.searchKeywordsUserAgent),
        "logs.maxSize" to listOf(R.string.searchKeywordsLogExport),
        "logs.export" to listOf(R.string.searchKeywordsLogExport),
    )

    /** `, | ，` 分隔的别名词表 → 词。 */
    private fun words(ctx: SettingsSearchContext, @StringRes key: Int): List<String> =
        ctx.str(key).split(',', '|', '\uFF0C').map { it.trim() }.filter { it.isNotEmpty() }
}

internal object SettingsSearch {
    /** 大小写 / 变音 / 全半角不敏感的子串匹配；标题命中在前，其次说明，再次别名词，同级保持索引序。 */
    fun filter(entries: List<SettingsEntry>, query: String): List<SettingsEntry> {
        val needle = fold(query.trim())
        if (needle.isEmpty()) return emptyList()
        val title = ArrayList<SettingsEntry>()
        val detail = ArrayList<SettingsEntry>()
        val keyword = ArrayList<SettingsEntry>()
        for (e in entries) {
            when {
                fold(e.title).contains(needle) -> title += e
                fold(e.detail).contains(needle) -> detail += e
                e.keywords.any { fold(it).contains(needle) } -> keyword += e
            }
        }
        return title + detail + keyword
    }

    /** NFKD 去变音 + 小写（兼顾全角）。 */
    fun fold(text: String): String =
        Normalizer.normalize(text, Normalizer.Form.NFKD).replace(Diacritics, "").lowercase(Locale.ROOT)

    private val Diacritics = Regex("\\p{Mn}+")
}
