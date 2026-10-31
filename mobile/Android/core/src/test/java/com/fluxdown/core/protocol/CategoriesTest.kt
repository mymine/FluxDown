package com.fluxdown.core.protocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** 自定义分类 wire 形状与编辑规则（对应 iOS `PreferencesProtocolTests` 的「分类」部分）。 */
class CategoriesTest {
    private fun success(result: CategoryBuild): CustomCategoryDto =
        (result as? CategoryBuild.Success)?.entry ?: error("expected success, got $result")

    private fun failure(result: CategoryBuild): CategoryValidationError =
        (result as? CategoryBuild.Failure)?.error ?: error("expected failure, got $result")

    // ── wire 形状 / 解析 ──

    @Test fun categoryJsonShapeMatchesDartAndRust() {
        val json = """[{"id":"x","name":"eBooks","icon":"library","matchMode":"extension","extensions":["epub"],"regexPattern":"","position":3,"visible":true,"isBuiltin":false,"builtinType":null,"saveDir":""}]"""
        val list = CustomCategoryDto.fromPreference(JsonValue.Str(json))
        assertEquals(listOf("epub"), list[0].extensions)
        assertEquals(3L, list[0].position)
        assertNull(list[0].builtinType)
        val back = CustomCategoryDto.preferenceValue(list).toJson()
        assertTrue(back.contains("\"builtinType\":null"))
        assertTrue(back.contains("\"matchMode\":\"extension\""))
        // 缺失字段取 serde 默认值。
        val sparse = CustomCategoryDto.fromPreference(JsonValue.Str("""[{"id":"a","name":"A"}]"""))
        assertEquals("file", sparse[0].icon)
        assertEquals("extension", sparse[0].matchMode)
        assertTrue(sparse[0].visible)
    }

    @Test fun categoriesFromPreferenceFallBackAndSortByPosition() {
        assertEquals(CustomCategoryDto.builtinDefaults, CustomCategoryDto.fromPreference(null))
        assertEquals(CustomCategoryDto.builtinDefaults, CustomCategoryDto.fromPreference(JsonValue.Str("not json")))
        assertEquals(CustomCategoryDto.builtinDefaults, CustomCategoryDto.fromPreference(JsonValue.Arr(emptyList())))
        // 任一项缺 id / name：整张列表视为损坏。
        val broken = JsonValue.Arr(listOf(jsonObject("id" to "a", "name" to "A"), jsonObject("id" to "b")))
        assertEquals(CustomCategoryDto.builtinDefaults, CustomCategoryDto.fromPreference(broken))
        val array = JsonValue.Arr(
            listOf(
                jsonObject("id" to "b", "name" to "B", "position" to 2),
                jsonObject("id" to "a", "name" to "A", "position" to 1),
            ),
        )
        assertEquals(listOf("a", "b"), CustomCategoryDto.fromPreference(array).map { it.id })
        // Flutter 偏好同形：JSON 数组字符串。
        val text = JsonValue.Str("""[{"id":"z","name":"Z","position":0}]""")
        assertEquals(listOf("z"), CustomCategoryDto.fromPreference(text).map { it.id })
    }

    @Test fun preferenceValueReindexesPositionsAndRoundTrips() {
        val list = CustomCategoryDto.builtinDefaults.reversed()
        val back = CustomCategoryDto.fromPreference(CustomCategoryDto.preferenceValue(list))
        assertEquals(list.map { it.id }, back.map { it.id })
        assertEquals((0L until list.size.toLong()).toList(), back.map { it.position })
    }

    @Test fun builtinDefaultsMatchTheProtocolBaseline() {
        val defaults = CustomCategoryDto.builtinDefaults
        assertEquals(
            listOf("all", "video", "audio", "document", "image", "program", "archive", "other"),
            defaults.map { it.builtinType },
        )
        assertTrue(defaults.all { it.isBuiltin })
        assertEquals("mp4", defaults[1].extensions.first())
        assertEquals("film", defaults[1].icon)
        assertTrue(defaults[0].isAll && !defaults[0].hasMatchRules)
        assertTrue(defaults[7].isOther && !defaults[7].hasMatchRules)
    }

    // ── 目录名 ──

    @Test fun dirNamesAreSanitizedLikeDesktop() {
        assertEquals("a b c", CategoryRules.sanitizeDirName("a/b:c "))
        assertEquals("name", CategoryRules.sanitizeDirName("name..."))
        assertEquals("", CategoryRules.sanitizeDirName("  "))
        assertEquals("tab here x", CategoryRules.sanitizeDirName("tab\there\u0001x"))
        assertEquals("a b c d e f g h", CategoryRules.sanitizeDirName("a\\b*c?d\"e<f>g|h"))
        assertEquals("电子书 合集", CategoryRules.sanitizeDirName("  电子书   合集 "))
        assertEquals("movies", CategoryRules.sanitizeDirName("movies. . "))
    }

    @Test fun dirUnderJoinsWithTheHostSeparator() {
        assertEquals("", CategoryRules.dirUnder("", "Video"))
        assertEquals("", CategoryRules.dirUnder("/tmp/dl", "///"))
        assertEquals("/Video", CategoryRules.dirUnder("/", "Video"))
        assertEquals("/tmp/dl/Video", CategoryRules.dirUnder("/tmp/dl/", "Video"))
        assertEquals("/tmp/dl/Video", CategoryRules.dirUnder("/tmp/dl//", "Video"))
        assertEquals("/srv/files/a b", CategoryRules.dirUnder(" /srv/files ", "a/b"))
        // Windows 主机：盘符或纯反斜杠路径用 `\`。
        assertEquals("D:\\Downloads\\Video", CategoryRules.dirUnder("D:\\Downloads", "Video"))
        assertEquals("D:/Downloads\\Video", CategoryRules.dirUnder("D:/Downloads", "Video"))
        assertEquals("\\\\nas\\share\\Audio", CategoryRules.dirUnder("\\\\nas\\share\\", "Audio"))
        assertEquals("C:\\Audio", CategoryRules.dirUnder("C:\\", "Audio"))
    }

    @Test fun autoDirsUseEnglishBuiltinLabelsAndSkipAll() {
        val list = CustomCategoryDto.builtinDefaults + CustomCategoryDto(id = "custom_1", name = "eBooks: new", position = 8)
        val updated = CategoryRules.autoDirs(list, "/dl/")!!
        val dirs = updated.associate { (it.builtinType ?: it.id) to it.saveDir }
        assertEquals("", dirs["all"])
        assertEquals("/dl/Video", dirs["video"])
        assertEquals("/dl/Audio", dirs["audio"])
        assertEquals("/dl/Document", dirs["document"])
        assertEquals("/dl/Image", dirs["image"])
        assertEquals("/dl/Programs", dirs["program"])
        assertEquals("/dl/Archive", dirs["archive"])
        assertEquals("/dl/Other", dirs["other"])
        assertEquals("/dl/eBooks new", dirs["custom_1"])
        assertNull(CategoryRules.autoDirs(list, "  "))
        assertEquals(8, CategoryRules.autoDirsChangeCount(list, "/dl/"))
        assertEquals(0, CategoryRules.autoDirsChangeCount(updated, "/dl/"))
    }

    // ── 扩展名 / 正则 ──

    @Test fun extensionTextParsing() {
        assertEquals(listOf("epub", "mobi", "azw3", "txt"), CategoryRules.parseExtensions(" .EPUB, mobi\uFF0Cazw3  txt "))
        assertTrue(CategoryRules.parseExtensions(" , ").isEmpty())
        assertEquals(listOf("targz"), CategoryRules.parseExtensions("tar.gz"))
    }

    @Test fun regexSanityCheck() {
        assertTrue(CategoryRules.regexLooksValid(""".*\.(epub|mobi)$"""))
        assertTrue(CategoryRules.regexLooksValid("[(]x"))
        assertFalse(CategoryRules.regexLooksValid("(abc"))
        assertFalse(CategoryRules.regexLooksValid("abc)"))
        assertFalse(CategoryRules.regexLooksValid("[abc"))
        assertFalse(CategoryRules.regexLooksValid("abc\\"))
        assertTrue(CategoryRules.regexLooksValid(""))
    }

    // ── 排序 ──

    @Test fun categoryReorderMovesToTheTargetSlot() {
        val list = CustomCategoryDto.builtinDefaults
        val ids = list.map { it.id }
        // 向下：a 落到 c 之后。
        val down = CategoryRules.reorder(list, ids[0], ids[2])!!
        assertEquals(listOf(ids[1], ids[2], ids[0]), down.map { it.id }.take(3))
        // 向上拖回原位。
        val up = CategoryRules.reorder(down, ids[0], ids[1])!!
        assertEquals(ids, up.map { it.id })
        assertNull(CategoryRules.reorder(list, ids[0], ids[0]))
        assertNull(CategoryRules.reorder(list, "missing", ids[0]))
    }

    // ── 表单校验 ──

    @Test fun newCategoryRequiresNameAndExtensions() {
        var draft = CategoryDraft()
        assertEquals(CategoryValidationError.NameRequired, failure(CategoryRules.build(draft, 1)))
        draft = draft.copy(name = "  ")
        assertEquals(CategoryValidationError.NameRequired, failure(CategoryRules.build(draft, 1)))
        draft = draft.copy(name = "eBooks")
        assertEquals(CategoryValidationError.ExtensionsRequired, failure(CategoryRules.build(draft, 1)))
        draft = draft.copy(extensionsText = ".EPUB, mobi", saveDir = "  /books  ")
        val entry = success(CategoryRules.build(draft, 1_700_000_000_123))
        assertEquals("custom_1700000000123", entry.id)
        assertEquals("eBooks", entry.name)
        assertEquals(listOf("epub", "mobi"), entry.extensions)
        assertEquals("", entry.regexPattern)
        assertEquals(CategoryRules.NEW_POSITION, entry.position)
        assertTrue(entry.visible && !entry.isBuiltin)
        assertNull(entry.builtinType)
        assertEquals("/books", entry.saveDir)
        assertEquals("file", entry.icon)
        assertEquals("extension", entry.matchMode)
    }

    @Test fun regexModeValidatesPatternAndClearsExtensions() {
        var draft = CategoryDraft().copy(name = "Rx", matchMode = "regex", extensionsText = "zip", regexText = "(oops")
        assertEquals(CategoryValidationError.RegexInvalid, failure(CategoryRules.build(draft, 1)))
        draft = draft.copy(regexText = """  .*\.(epub|mobi)$ """)
        val entry = success(CategoryRules.build(draft, 1))
        assertEquals("regex", entry.matchMode)
        assertEquals(""".*\.(epub|mobi)$""", entry.regexPattern)
        assertTrue(entry.extensions.isEmpty())
    }

    @Test fun builtinCategoriesKeepIdentityAndSkipRequiredChecks() {
        val video = CustomCategoryDto.builtinDefaults.first { it.builtinType == "video" }
        val draft = CategoryDraft(video).copy(name = "", extensionsText = "", saveDir = "/v")
        val edited = success(CategoryRules.build(draft, 1))
        assertEquals(video.id, edited.id)
        assertTrue(edited.isBuiltin)
        assertEquals("video", edited.builtinType)
        assertEquals(video.position, edited.position)
        assertTrue(edited.extensions.isEmpty())
        assertEquals("/v", edited.saveDir)

        // all / other：不展示匹配规则，规则字段保持原样。
        val other = CustomCategoryDto.builtinDefaults.first { it.isOther }
        val otherDraft = CategoryDraft(other).copy(extensionsText = "zip", matchMode = "regex", regexText = "((")
        val kept = success(CategoryRules.build(otherDraft, 1))
        assertEquals(other.extensions, kept.extensions)
        assertEquals(other.regexPattern, kept.regexPattern)
    }

    @Test fun upsertReplacesOrAppends() {
        val list = listOf(CustomCategoryDto("a", "A"), CustomCategoryDto("b", "B"))
        val replaced = CategoryRules.upserting(CustomCategoryDto("a", "A2"), list)
        assertEquals(listOf("A2", "B"), replaced.map { it.name })
        val appended = CategoryRules.upserting(CustomCategoryDto("c", "C"), list)
        assertEquals(listOf("a", "b", "c"), appended.map { it.id })
    }

    @Test fun iconCatalogHasTwentyFiveUniqueNames() {
        assertEquals(25, CategoryRules.iconNames.size)
        assertEquals(25, CategoryRules.iconNames.toSet().size)
        assertTrue("package2" in CategoryRules.iconNames && "hardDrive" in CategoryRules.iconNames)
    }

    // ── 下载页筛选区显隐 ──

    private fun prefs(vararg pairs: Pair<String, Boolean>) =
        AgentPreferences(values = pairs.associate { (k, v) -> k to JsonValue.of(v) })

    @Test fun everythingShowsByDefault() {
        assertEquals(FilterBarVisibility(), FilterBarVisibility.of(AgentPreferences.Empty))
    }

    @Test fun readsTheThreeSidebarPreferences() {
        val visibility = FilterBarVisibility.of(prefs("ui.show_sidebar_status" to false, "ui.show_sidebar_category" to false))
        assertFalse(visibility.status)
        assertTrue(visibility.queues)
        assertFalse(visibility.categories)
    }

    @Test fun emptyOnlyWhenNothingIsVisible() {
        val hidden = FilterBarVisibility(status = false, queues = false, categories = false)
        assertTrue(hidden.isEmpty(hasCategories = true, hasScopeChip = false))
        // 状态条显示 → 非空
        assertFalse(FilterBarVisibility(status = true, queues = false, categories = false).isEmpty(true, false))
        // 分类开着且有分类 → 非空；分类开着但没有分类 → 空
        val categoriesOnly = FilterBarVisibility(status = false, queues = false, categories = true)
        assertFalse(categoriesOnly.isEmpty(hasCategories = true, hasScopeChip = false))
        assertTrue(categoriesOnly.isEmpty(hasCategories = false, hasScopeChip = false))
        // 队列芯片显示 → 非空
        assertFalse(hidden.isEmpty(hasCategories = false, hasScopeChip = true))
    }

    // ── 外部唤起的分类保存目录（同 agent `category_dir.rs` 的用例）──

    private fun dirCategory(id: String, builtin: String?, exts: List<String>, saveDir: String, position: Long, visible: Boolean = true) =
        CustomCategoryDto(
            id = id, name = id, extensions = exts, position = position, visible = visible,
            isBuiltin = builtin != null, builtinType = builtin, saveDir = saveDir,
        )

    private fun sorted(vararg list: CustomCategoryDto) = list.sortedBy { it.position }

    @Test fun firstMatchingCategoryWithDirWinsInPositionOrder() {
        val list = sorted(
            dirCategory("video2", null, listOf("mp4"), "/second", 5),
            dirCategory("video", "video", listOf("mp4", "mkv"), "/videos", 1),
            dirCategory("docs", "document", listOf("pdf"), "", 2),
        )
        assertEquals("/videos", CategoryRules.saveDirFor(list, "Movie.MP4", ""))
        // 命中但未配置目录 → 不回退到其他分类的目录。
        assertNull(CategoryRules.saveDirFor(list, "a.pdf", ""))
    }

    @Test fun otherDirAppliesOnlyWhenNoNormalCategoryMatches() {
        val list = sorted(
            dirCategory("video", "video", listOf("mp4"), "", 1),
            dirCategory("other", "other", emptyList(), "/other", 7),
        )
        assertEquals("/other", CategoryRules.saveDirFor(list, "setup.exe", ""))
        assertNull(CategoryRules.saveDirFor(list, "clip.mp4", ""))
    }

    @Test fun urlSegmentFillsMissingNameAndHiddenCategoriesAreIgnored() {
        val list = sorted(
            dirCategory("music", null, listOf("flac"), "/music-hidden", 1, visible = false),
            dirCategory("audio", "audio", listOf("flac"), "/music", 2),
        )
        assertEquals("/music", CategoryRules.saveDirFor(list, "", "https://x.test/a/My%20Song.flac?x=1"))
        assertNull(CategoryRules.saveDirFor(list, "", "https://x.test/download"))
        // 非层级链接（magnet）没有路径末段可用。
        assertNull(CategoryRules.saveDirFor(list, "", "magnet:?xt=urn:btih:abc&dn=a.flac"))
    }

    @Test fun regexMatchesCaseInsensitivelyAndInvalidRegexNeverMatches() {
        val list = sorted(
            CustomCategoryDto(id = "bad", name = "bad", matchMode = "regex", regexPattern = "(", position = 1, saveDir = "/bad"),
            CustomCategoryDto(id = "iso", name = "iso", matchMode = "regex", regexPattern = "^ubuntu-.*\\.iso$", position = 2, saveDir = "/iso"),
        )
        assertEquals("/iso", CategoryRules.saveDirFor(list, "Ubuntu-24.04.ISO", ""))
    }

    // ── 网址规则（url 模式）──

    @Test fun urlPatternRoundTripsThroughPreferenceJson() {
        val list = listOf(
            CustomCategoryDto(
                id = "u1", name = "Site", matchMode = "url",
                urlPattern = "https://*.gamebanana.com/mods*", saveDir = "/mod",
            ),
        )
        val back = CustomCategoryDto.fromPreference(CustomCategoryDto.preferenceValue(list))
        assertEquals("url", back[0].matchMode)
        assertEquals("https://*.gamebanana.com/mods*", back[0].urlPattern)
        // 缺失字段取默认空串。
        val sparse = CustomCategoryDto.fromPreference(JsonValue.Str("""[{"id":"a","name":"A","matchMode":"url"}]"""))
        assertEquals("", sparse[0].urlPattern)
    }

    @Test fun draftOfUrlCategoryCarriesPatternAndEditedTextIsWritten() {
        val existing = CustomCategoryDto(
            id = "u1", name = "Site", matchMode = "url",
            urlPattern = "https://*.gamebanana.com/mods*", saveDir = "/mod",
        )
        // 编辑器打开既有网址分类：草稿带出模式与模式文本，未改动直接保存不丢规则。
        val draft = CategoryDraft(existing)
        assertEquals("url", draft.matchMode)
        assertEquals("https://*.gamebanana.com/mods*", draft.urlText)
        val untouched = success(CategoryRules.build(draft.copy(name = "Site2", saveDir = "/mod2"), nowMs = 42L))
        assertEquals("url", untouched.matchMode)
        assertEquals("https://*.gamebanana.com/mods*", untouched.urlPattern)
        assertTrue(untouched.extensions.isEmpty() && untouched.regexPattern.isEmpty())
        // 用户改了输入框：写入新的（去空白）模式。
        val edited = success(CategoryRules.build(draft.copy(urlText = "  https://mirror.test/*  "), nowMs = 42L))
        assertEquals("https://mirror.test/*", edited.urlPattern)
        assertEquals("/mod", edited.saveDir)
    }

    @Test fun urlModeRequiresPatternAndNewCategoryUsesUrlText() {
        val blank = CategoryDraft().copy(name = "Site", matchMode = "url", urlText = "   ", extensionsText = "zip")
        assertEquals(CategoryValidationError.UrlPatternRequired, failure(CategoryRules.build(blank, nowMs = 1L)))
        val entry = success(CategoryRules.build(blank.copy(urlText = "https://example.com/*", saveDir = "/x"), nowMs = 1L))
        assertEquals("url", entry.matchMode)
        assertEquals("https://example.com/*", entry.urlPattern)
        // 另一模式的残留输入不进入条目。
        assertTrue(entry.extensions.isEmpty())
        assertEquals("", entry.regexPattern)
        assertEquals("urlPatternRequired", CategoryValidationError.UrlPatternRequired.i18nKey)
    }

    @Test fun switchingModeAwayFromUrlClearsPattern() {
        val existing = CustomCategoryDto(
            id = "u1", name = "Site", matchMode = "url",
            urlPattern = "https://*.gamebanana.com/mods*", saveDir = "/mod",
        )
        val toExtension = success(CategoryRules.build(CategoryDraft(existing).copy(matchMode = "extension", extensionsText = "zip"), nowMs = 43L))
        assertEquals("extension", toExtension.matchMode)
        assertEquals("", toExtension.urlPattern)
        assertEquals(listOf("zip"), toExtension.extensions)
        val toRegex = success(CategoryRules.build(CategoryDraft(existing).copy(matchMode = "regex", regexText = ".*\\.zip$"), nowMs = 44L))
        assertEquals("regex", toRegex.matchMode)
        assertEquals("", toRegex.urlPattern)
        // 扩展名 / 正则分类切到网址模式：清空扩展名与正则。
        val ext = CustomCategoryDto(id = "e1", name = "Zip", extensions = listOf("zip"))
        val toUrl = success(CategoryRules.build(CategoryDraft(ext).copy(matchMode = "url", urlText = "https://a.test/*"), nowMs = 45L))
        assertEquals("url", toUrl.matchMode)
        assertTrue(toUrl.extensions.isEmpty())
        assertEquals("https://a.test/*", toUrl.urlPattern)
    }

    @Test fun saveDirForPutsUrlRulesAheadOfExtensionRules() {
        val list = listOf(
            CustomCategoryDto(id = "b1", name = "zip", extensions = listOf("zip"), saveDir = "/archives"),
            CustomCategoryDto(id = "u1", name = "site", matchMode = "url", urlPattern = "https://mirror.test/*", saveDir = "/mirror"),
            CustomCategoryDto(id = "o1", name = "other", isBuiltin = true, builtinType = "other", saveDir = "/other"),
        )
        // 网址规则排在后面也优先命中。
        assertEquals("/mirror", CategoryRules.saveDirFor(list, "a.zip", "https://mirror.test/a.zip"))
        // 未命中网址：按扩展名走，后续进 other。
        assertEquals("/archives", CategoryRules.saveDirFor(list, "a.zip", "https://x.test/a.zip"))
        assertEquals("/other", CategoryRules.saveDirFor(list, "setup.exe", "https://x.test/setup.exe"))
        // 文件名未知：只有网址规则参与；未命中网址规则时不进 other。
        assertNull(CategoryRules.saveDirFor(list, "", "https://x.test/download/blob"))
        assertEquals("/mirror", CategoryRules.saveDirFor(list, "", "https://mirror.test/download/blob"))
        // 空 / 非法模式不命中：照常落到 other。
        val blanks = listOf(
            CustomCategoryDto(id = "u0", name = "blank", matchMode = "url", urlPattern = "   ", saveDir = "/never"),
            CustomCategoryDto(id = "o1", name = "other", isBuiltin = true, builtinType = "other", saveDir = "/other"),
        )
        assertEquals("/other", CategoryRules.saveDirFor(blanks, "setup.exe", "https://nokeep.test/setup.exe"))
        // 命中但没配目录的网址规则阻止 other 兜底（同引擎夹具语义）。
        val noDir = listOf(
            CustomCategoryDto(id = "u0", name = "site", matchMode = "url", urlPattern = "https://nokeep.test/*"),
            CustomCategoryDto(id = "o1", name = "other", isBuiltin = true, builtinType = "other", saveDir = "/other"),
        )
        assertNull(CategoryRules.saveDirFor(noDir, "setup.exe", "https://nokeep.test/setup.exe"))
    }
}