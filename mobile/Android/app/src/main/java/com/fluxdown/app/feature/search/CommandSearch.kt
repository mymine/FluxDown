package com.fluxdown.app.feature.search

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.fluxdown.app.AppContainer
import com.fluxdown.app.R
import com.fluxdown.app.actions.LocalTaskActions
import com.fluxdown.app.actions.TaskActions
import com.fluxdown.app.data.ThemeMode
import com.fluxdown.app.feature.devices.GlyphTile
import com.fluxdown.app.i18n.fill
import com.fluxdown.app.i18n.str
import com.fluxdown.app.nav.AppNavigator
import com.fluxdown.app.nav.AppTab
import com.fluxdown.app.nav.LocalNavigator
import com.fluxdown.app.nav.Route
import com.fluxdown.app.feature.settings.LocalSettingsFocus
import com.fluxdown.app.feature.settings.SettingsIndex
import com.fluxdown.app.feature.settings.SettingsSearchContext
import com.fluxdown.core.model.HostRef
import com.fluxdown.core.protocol.HostSection
import com.fluxdown.core.protocol.SettingsForm
import com.fluxdown.core.protocol.preferences
import androidx.compose.runtime.collectAsState
import com.fluxdown.app.nav.SheetRoute
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.app.shell.hostState
import com.fluxdown.app.ui.fileCategory
import com.fluxdown.app.ui.tileIcon
import com.fluxdown.core.format.Format
import com.fluxdown.core.model.CategoryIndex
import com.fluxdown.core.model.Task
import com.fluxdown.core.model.TaskStatus
import com.fluxdown.fluxui.chrome.ScopeTab
import com.fluxdown.fluxui.chrome.ScopeTabs
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.data.FileTile
import com.fluxdown.fluxui.data.FileTileSize
import com.fluxdown.fluxui.feedback.FluxEmpty
import com.fluxdown.fluxui.feedback.FluxGlyph
import com.fluxdown.fluxui.icons.FluxIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.CommandSearchScaffold
import com.fluxdown.fluxui.overlay.CommandSearchSectionHeader
import com.fluxdown.fluxui.overlay.rememberFluxHighlight
import com.fluxdown.fluxui.theme.FileCategory
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import com.fluxdown.fluxui.theme.fluxPressable
import kotlinx.coroutines.launch

private enum class SearchScope { All, Tasks, Commands, Settings }

/** 命令 / 设置页条目：[keywords] 只做连续子串匹配（降权），[sub] 同样参与降权匹配。 */
@Immutable
private class Entry(
    val id: String,
    val title: String,
    val sub: String?,
    val keywords: List<String>,
    val icon: ImageVector,
    val run: () -> Unit,
)

/** 搜索用的任务投影：只含会影响结果的字段，进度字节变化不会使其失效。 */
@Immutable
private data class TaskRef(
    val id: String,
    val fileName: String,
    val status: TaskStatus,
    val totalBytes: Long,
    val createdAt: Long,
    val icon: ImageVector,
    val category: FileCategory,
)

@Immutable
private class Scored<T>(val item: T, val score: Int)

/** 键盘“搜索”键执行当前第一条结果。 */
private class FirstAction {
    var run: (() -> Unit)? = null
}

private const val CAP_ALL_TASKS = 5
private const val CAP_ALL_COMMANDS = 6
private const val CAP_ALL_COMMANDS_IDLE = 8
private const val CAP_ALL_SETTINGS = 3
private const val CAP_SCOPED = 30

/**
 * G1 命令搜索：任务（文件名，大小写不敏感，多词须全部命中）· 命令 · 设置页。
 * 选中结果 = 先关闭搜索，再执行导航 / 命令。拼音首字母匹配不在本期范围。
 */
@Composable
fun CommandSearch(visible: Boolean, onDismiss: () -> Unit) {
    val container = LocalAppContainer.current
    val nav = LocalNavigator.current
    val actions = LocalTaskActions.current
    val context = LocalContext.current
    val config = LocalConfiguration.current
    val dark = FluxTheme.colors.dark

    var query by remember { mutableStateOf("") }
    var scope by remember { mutableStateOf(SearchScope.All) }
    // `ui.show_activity_theme`（云同步偏好，通用设置「入口」）关闭 → 不提供明暗切换命令
    val host = hostState()
    val showThemeToggle by remember { derivedStateOf { host.value.preferences.bool("ui.show_activity_theme", true) } }
    LaunchedEffect(visible) {
        if (visible) {
            query = ""
            scope = SearchScope.All
        }
    }

    val groups = remember(context, config, dark, nav, actions, container, showThemeToggle) {
        buildEntries(context, dark, nav, actions, container, showThemeToggle)
    }
    val settings = rememberSettingsEntries(nav)
    val first = remember { FirstAction() }

    CommandSearchScaffold(
        visible = visible,
        query = query,
        onQueryChange = { query = it },
        onDismiss = onDismiss,
        placeholder = str(R.string.searchPlaceholder),
        onSearch = { first.run?.invoke() },
    ) {
        SearchBody(
            query = query,
            scope = scope,
            onScope = { scope = it },
            commands = groups.commands,
            settings = settings,
            nav = nav,
            onDismiss = onDismiss,
            first = first,
        )
    }
}

// ── 条目 ────────────────────────────────────────────────────────────────

private class EntryGroups(val commands: List<Entry>)

private fun String.words(): List<String> = split(',', '|', '，').map { it.trim() }.filter { it.isNotEmpty() }

private fun buildEntries(
    context: Context,
    dark: Boolean,
    nav: AppNavigator,
    actions: TaskActions,
    container: AppContainer,
    showThemeToggle: Boolean,
): EntryGroups {
    fun s(id: Int, vararg args: Pair<String, Any?>) = context.str(id, *args)

    val goTo = R.string.commandPaletteGoTo
    val commands = listOf(
        Entry("new", s(R.string.newDownload), null, emptyList(), FluxIcons.Plus) {
            nav.openSheet(SheetRoute.NewDownload())
        },
        Entry("pause-all", s(R.string.pauseAll), null, s(R.string.commandPalettePauseAllAliases).words(), FluxIcons.Pause) {
            actions.pauseAll()
        },
        Entry("resume-all", s(R.string.resumeAll), null, s(R.string.commandPaletteResumeAllAliases).words(), FluxIcons.Play) {
            actions.resumeAll()
        },
        Entry("view", s(R.string.viewOptionsTitle), null, emptyList(), FluxIcons.SlidersHorizontal) {
            nav.selectTab(AppTab.Downloads)
            nav.openSheet(SheetRoute.ViewOptions)
        },
        Entry("activity", s(R.string.mobileSearchCmdActivity), null, emptyList(), FluxIcons.Activity) {
            nav.openSheet(SheetRoute.Activity)
        },
        Entry(
            "theme",
            s(if (dark) R.string.toggleToLight else R.string.toggleToDark),
            null,
            listOf(s(R.string.themeMode), s(R.string.themeModeDark), s(R.string.themeModeLight), s(R.string.activityThemeToggle)),
            if (dark) FluxIcons.Sun else FluxIcons.Moon,
        ) {
            container.appScope.launch { container.appearance.setMode(if (dark) ThemeMode.Light else ThemeMode.Dark) }
        },
        Entry("go-dl", s(goTo, "page" to s(R.string.mobileNavDownloads)), null, emptyList(), FluxIcons.ArrowDown) {
            nav.selectTab(AppTab.Downloads)
        },
        Entry("go-rss", s(goTo, "page" to s(R.string.mobileNavRss)), null, emptyList(), FluxIcons.Rss) {
            nav.selectTab(AppTab.Rss)
        },
        Entry("go-dev", s(goTo, "page" to s(R.string.mobileNavDevices)), null, emptyList(), FluxIcons.Smartphone) {
            nav.selectTab(AppTab.Devices)
        },
    )
    return EntryGroups(commands.filter { showThemeToggle || it.id != "theme" })
}

/** 设置条目 = 设置首页搜索的同一份索引（[SettingsIndex]：分类入口 + 各页的行，按主机能力 / 配置过滤）。 */
@Composable
private fun rememberSettingsEntries(nav: AppNavigator): List<Entry> {
    val context = LocalContext.current
    val config = LocalConfiguration.current
    val container = LocalAppContainer.current
    val focus = LocalSettingsFocus.current
    val host = hostState()
    val hostRef by container.host.collectAsState()
    val cfg by remember { derivedStateOf { host.value.config } }
    val prefs by remember { derivedStateOf { host.value.sections[HostSection.agentPreferences] } }
    val info by remember { derivedStateOf { host.value.info } }
    return remember(context, config, cfg, prefs, info, hostRef, nav, focus) {
        val state = host.value
        val ctx = SettingsSearchContext(
            context, state, SettingsForm(state.config, prefs = state.preferences.values), hostRef is HostRef.Local,
        )
        SettingsIndex.entries(ctx).map { e ->
            Entry(e.id, e.title, e.breadcrumb, e.keywords + listOfNotNull(e.detail.ifEmpty { null }), e.icon) {
                focus.request(e)
                nav.push(Route.Settings(e.page))
            }
        }
    }
}

// ── 匹配 ────────────────────────────────────────────────────────────────

/** 相等 100 / 前缀 90 / 词边界 82 / 子串 76；未命中 0。 */
private fun tokenScore(text: String, tok: String): Int {
    if (text.equals(tok, ignoreCase = true)) return 100
    if (text.startsWith(tok, ignoreCase = true)) return 90
    val i = text.indexOf(tok, ignoreCase = true)
    return when {
        i < 0 -> 0
        i == 0 -> 90
        !text[i - 1].isLetterOrDigit() -> 82
        else -> 76
    }
}

/** 松散子序列（仅用于命令 / 设置标题）：24。 */
private fun looseScore(text: String, tok: String): Int {
    if (tok.length < 2) return 0
    var j = 0
    for (ch in text) if (j < tok.length && ch.equals(tok[j], ignoreCase = true)) j++
    return if (j == tok.length) 24 else 0
}

/** 每个词都必须命中（标题 > 关键词 50 > 描述 40）；返回各词最低分，0 = 不匹配。 */
private fun entryScore(e: Entry, tokens: List<String>): Int {
    var worst = Int.MAX_VALUE
    for (tok in tokens) {
        var best = maxOf(tokenScore(e.title, tok), looseScore(e.title, tok))
        if (best < 50) {
            if (e.keywords.any { it.contains(tok, ignoreCase = true) }) best = 50
            else if (e.sub?.contains(tok, ignoreCase = true) == true) best = 40
        }
        if (best <= 0) return 0
        worst = minOf(worst, best)
    }
    return if (worst == Int.MAX_VALUE) 0 else worst
}

private fun taskScore(name: String, tokens: List<String>): Int {
    var worst = Int.MAX_VALUE
    for (tok in tokens) {
        val s = tokenScore(name, tok)
        if (s <= 0) return 0
        worst = minOf(worst, s)
    }
    return if (worst == Int.MAX_VALUE) 0 else worst
}

/** 高亮目标：整串命中优先，否则取第一个命中标题的词。 */
private fun highlightTarget(text: String, query: String, tokens: List<String>): String {
    val q = query.trim()
    if (q.isEmpty()) return ""
    if (text.contains(q, ignoreCase = true)) return q
    return tokens.firstOrNull { text.contains(it, ignoreCase = true) } ?: ""
}

private val Whitespace = Regex("\\s+")

// ── 主体 ────────────────────────────────────────────────────────────────

@Composable
private fun ColumnScope.SearchBody(
    query: String,
    scope: SearchScope,
    onScope: (SearchScope) -> Unit,
    commands: List<Entry>,
    settings: List<Entry>,
    nav: AppNavigator,
    onDismiss: () -> Unit,
    first: FirstAction,
) {
    val host = hostState()
    val categories by remember { derivedStateOf { host.value.categories } }
    val index = remember(categories) { CategoryIndex(categories) }
    val refs by remember(index) { derivedStateOf { host.value.tasks.map { it.toRef(index) } } }

    val tokens = remember(query) { query.trim().split(Whitespace).filter { it.isNotEmpty() } }
    val searching = tokens.isNotEmpty()

    val commandHits = remember(tokens, commands) { scoreEntries(commands, tokens) }
    val settingHits = remember(tokens, settings) { scoreEntries(settings, tokens) }
    val taskHits = remember(tokens, refs) {
        if (tokens.isEmpty()) {
            emptyList()
        } else {
            refs.mapNotNull { r -> taskScore(r.fileName, tokens).takeIf { it > 0 }?.let { Scored(r, it) } }
                .sortedWith(compareByDescending<Scored<TaskRef>> { it.score }.thenByDescending { it.item.createdAt })
        }
    }

    val all = scope == SearchScope.All
    val shownTasks = if (all || scope == SearchScope.Tasks) taskHits.take(if (all) CAP_ALL_TASKS else CAP_SCOPED) else emptyList()
    val shownCommands = if (all || scope == SearchScope.Commands) {
        commandHits.take(if (!all) CAP_SCOPED else if (searching) CAP_ALL_COMMANDS else CAP_ALL_COMMANDS_IDLE)
    } else {
        emptyList()
    }
    val shownSettings = if (all || scope == SearchScope.Settings) settingHits.take(if (all) CAP_ALL_SETTINGS else CAP_SCOPED) else emptyList()

    val openTask: (TaskRef) -> Unit = { ref ->
        onDismiss()
        nav.selectTab(AppTab.Downloads)
        nav.push(Route.TaskDetail(ref.id))
    }
    val runEntry: (Entry) -> Unit = { e ->
        onDismiss()
        e.run()
    }
    SideEffect {
        first.run = shownTasks.firstOrNull()?.let { h -> { openTask(h.item) } }
            ?: shownCommands.firstOrNull()?.let { h -> { runEntry(h.item) } }
            ?: shownSettings.firstOrNull()?.let { h -> { runEntry(h.item) } }
    }

    val countTemplate = stringResource(R.string.mobileSearchResultsCount)
    val tabs = listOf(
        ScopeTab(SearchScope.All, str(R.string.tabAll), if (searching) taskHits.size + commandHits.size + settingHits.size else null),
        ScopeTab(SearchScope.Tasks, str(R.string.searchGroupTasks), if (searching) taskHits.size else null),
        ScopeTab(SearchScope.Commands, str(R.string.mobileSearchGroupCommands), if (searching) commandHits.size else null),
        ScopeTab(SearchScope.Settings, str(R.string.searchGroupSettings), if (searching) settingHits.size else null),
    )
    ScopeTabs(
        tabs = tabs,
        selected = scope,
        onSelect = onScope,
        modifier = Modifier.padding(horizontal = 16.dp),
        countDescription = { n -> countTemplate.fill("n" to n) },
    )

    val bottom = 40.dp + WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    LazyColumn(
        Modifier.weight(1f).fillMaxWidth(),
        contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = bottom),
    ) {
        if (shownTasks.isEmpty() && shownCommands.isEmpty() && shownSettings.isEmpty()) {
            item(key = "empty") {
                Box(Modifier.fillMaxWidth().padding(top = 48.dp), contentAlignment = Alignment.Center) {
                    if (searching) {
                        FluxEmpty(
                            glyph = FluxGlyph.Search,
                            title = str(R.string.commandPaletteNoResults),
                            subtitle = str(R.string.mobileSearchNoResultTip),
                        )
                    } else {
                        FluxEmpty(glyph = FluxGlyph.Search, title = str(R.string.searchTasksPlaceholder))
                    }
                }
            }
        }
        if (shownTasks.isNotEmpty()) {
            item(key = "h-tasks") {
                CommandSearchSectionHeader(str(R.string.searchGroupTasks), trailing = taskHits.size.toString())
            }
            item(key = "tasks") {
                GlassSection {
                    shownTasks.forEach { h ->
                        row(hasIcon = true) { TaskHit(h.item, query, tokens) { openTask(h.item) } }
                    }
                }
            }
            if (all && taskHits.size > shownTasks.size) {
                item(key = "tasks-more") {
                    Box(Modifier.fillMaxWidth().padding(top = 8.dp), contentAlignment = Alignment.Center) {
                        FluxButton(
                            text = str(R.string.mobileSearchShowAllTasks, "n" to taskHits.size),
                            onClick = { onScope(SearchScope.Tasks) },
                            variant = ButtonVariant.Ghost,
                            size = ButtonSize.Sm,
                        )
                    }
                }
            }
        }
        if (shownCommands.isNotEmpty()) {
            item(key = "h-commands") { CommandSearchSectionHeader(str(R.string.mobileSearchGroupCommands)) }
            item(key = "commands") {
                GlassSection {
                    shownCommands.forEach { h ->
                        row(hasIcon = true) { EntryHit(h.item, query, tokens, chevron = false) { runEntry(h.item) } }
                    }
                }
            }
        }
        if (shownSettings.isNotEmpty()) {
            item(key = "h-settings") { CommandSearchSectionHeader(str(R.string.searchGroupSettings)) }
            item(key = "settings") {
                GlassSection {
                    shownSettings.forEach { h ->
                        row(hasIcon = true) { EntryHit(h.item, query, tokens, chevron = true) { runEntry(h.item) } }
                    }
                }
            }
        }
    }
}

private fun scoreEntries(entries: List<Entry>, tokens: List<String>): List<Scored<Entry>> =
    if (tokens.isEmpty()) {
        entries.map { Scored(it, 100) }
    } else {
        entries.mapNotNull { e -> entryScore(e, tokens).takeIf { it > 0 }?.let { Scored(e, it) } }
            .sortedByDescending { it.score }
    }

private fun Task.toRef(index: CategoryIndex): TaskRef {
    val category = index.categoryOf(fileName, shareUrl)
    return TaskRef(taskId, fileName, status, totalBytes, createdAt, category.tileIcon(this), category.fileCategory())
}

// ── 结果行 ──────────────────────────────────────────────────────────────

@Composable
private fun TaskHit(ref: TaskRef, query: String, tokens: List<String>, onClick: () -> Unit) {
    val c = FluxTheme.colors
    val sub = listOfNotNull(
        statusLabel(ref.status),
        ref.totalBytes.takeIf { it > 0 }?.let { Format.bytes(it).toString() },
    ).joinToString(" · ").ifEmpty { null }
    HitRow(
        title = ref.fileName,
        sub = sub,
        query = query,
        tokens = tokens,
        chevron = true,
        onClick = onClick,
        leading = { FileTile(ref.icon, c.category(ref.category), size = FileTileSize.Sm) },
    )
}

@Composable
private fun EntryHit(entry: Entry, query: String, tokens: List<String>, chevron: Boolean, onClick: () -> Unit) {
    HitRow(
        title = entry.title,
        sub = entry.sub,
        query = query,
        tokens = tokens,
        chevron = chevron,
        onClick = onClick,
        leading = { GlyphTile(entry.icon, size = 32.dp) },
    )
}

@Composable
private fun HitRow(
    title: String,
    sub: String?,
    query: String,
    tokens: List<String>,
    chevron: Boolean,
    onClick: () -> Unit,
    leading: @Composable () -> Unit,
) {
    val c = FluxTheme.colors
    val t = FluxTheme.type
    val titleStyle = remember(t, c) { t.weight(t.body, 500).copy(color = c.ink) }
    val annotated = rememberFluxHighlight(title, highlightTarget(title, query, tokens), titleStyle)
    Row(
        Modifier
            .fillMaxWidth()
            .fluxPressable(onClick = onClick, role = Role.Button)
            .heightIn(min = 64.dp)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        leading()
        Column(Modifier.weight(1f)) {
            BasicText(annotated, style = titleStyle, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (sub != null) FluxText(sub, modifier = Modifier.padding(top = 2.dp), style = t.sm, color = c.inkMuted, maxLines = 1)
        }
        if (chevron) FluxIcon(FluxIcons.ChevronRight, null, size = 18.dp, tint = c.inkFaint)
    }
}

@Composable
private fun statusLabel(status: TaskStatus): String? = when (status) {
    TaskStatus.Pending -> str(R.string.statusPending)
    TaskStatus.Downloading -> str(R.string.statusDownloading)
    TaskStatus.Paused -> str(R.string.statusPaused)
    TaskStatus.Completed -> str(R.string.statusCompleted)
    TaskStatus.Failed -> str(R.string.statusError)
    TaskStatus.Preparing -> str(R.string.statusPreparing)
    TaskStatus.Unknown -> null
}
