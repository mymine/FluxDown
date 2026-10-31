import FluxDomain
import Foundation
import Testing
@testable import FluxDown

// 通用页的纯逻辑：Info.plist 链接声明、分类图标映射、行目录。
// 分类规则本身（解析 / 校验 / 重排 / 一键目录）由 FluxDomain 的 `PreferencesProtocolTests` 覆盖。

#if FULL_FLAVOR
@MainActor
struct GeneralLinkHandlingTests {
    private let shipped: [String: Any] = [
        "CFBundleURLTypes": [
            ["CFBundleURLSchemes": ["magnet", "ED2K", "fluxdown"]],
        ],
    ]

    @Test func readsSchemesCaseInsensitively() {
        #expect(GeneralLinkHandling.declaredSchemes(in: shipped) == ["magnet", "ed2k", "fluxdown"])
        #expect(GeneralLinkHandling.declared(in: shipped) == [.magnet, .ed2k])
    }

    @Test func emptyOrMalformedDeclaresNothing() {
        #expect(GeneralLinkHandling.declared(in: [:]).isEmpty)
        #expect(GeneralLinkHandling.declared(in: ["CFBundleURLTypes": "oops", "CFBundleDocumentTypes": 3]).isEmpty)
    }

    @Test func torrentNeedsADocumentType() {
        // 只导入类型、没有文档类型：不算可打开。
        let importedOnly: [String: Any] = [
            "UTImportedTypeDeclarations": [
                ["UTTypeIdentifier": "org.bittorrent.torrent", "UTTypeTagSpecification": ["public.filename-extension": ["torrent"]]],
            ],
        ]
        #expect(!GeneralLinkHandling.declaresTorrentDocument(in: importedOnly))

        let byUTI: [String: Any] = ["CFBundleDocumentTypes": [["LSItemContentTypes": ["org.bittorrent.torrent"]]]]
        #expect(GeneralLinkHandling.declared(in: byUTI) == [.torrent])

        let byExtension: [String: Any] = ["CFBundleDocumentTypes": [["CFBundleTypeExtensions": ["TORRENT"]]]]
        #expect(GeneralLinkHandling.declaresTorrentDocument(in: byExtension))
    }

    @Test func torrentThroughACustomImportedType() {
        let info: [String: Any] = [
            "UTImportedTypeDeclarations": [
                ["UTTypeIdentifier": "com.example.bt", "UTTypeTagSpecification": ["public.filename-extension": "torrent"]],
            ],
            "CFBundleDocumentTypes": [["LSItemContentTypes": ["com.example.bt"]]],
        ]
        #expect(GeneralLinkHandling.declaresTorrentDocument(in: info))
        let unrelated: [String: Any] = ["CFBundleDocumentTypes": [["LSItemContentTypes": ["public.json"]]]]
        #expect(!GeneralLinkHandling.declaresTorrentDocument(in: unrelated))
    }
}
#endif

@MainActor
struct GeneralCategoryIconTests {
    @Test func everyWireIconHasADistinctSymbol() {
        let symbols = CategoryRules.iconNames.map(GeneralCategoryIcon.symbol)
        #expect(symbols.count == 25)
        #expect(Set(symbols).count == 25)
    }

    @Test func unknownIconFallsBackToFile() {
        #expect(GeneralCategoryIcon.symbol("nope") == GeneralCategoryIcon.symbol("file"))
    }
}

@MainActor
struct GeneralRowTests {
    @Test func catalogKeyedRowsAreWiredToTheCatalog() {
        for row in GeneralRow.allCases {
            guard let item = row.item else { continue }
            #expect(SettingsCatalog.field(item.key) != nil, "\(row) has no catalog field")
            #expect(item.id == row.id)
        }
    }

    @Test func syncMarkersFollowTheCatalog() {
        #expect(GeneralRow.keepAwake.item?.isSynced == true)
        #expect(GeneralRow.activityRss.item?.isSynced == true)
        #expect(GeneralRow.sidebarCategory.item?.isSynced == true)
        #expect(GeneralRow.analytics.item?.isSynced == false)
    }

    @Test func searchIndexesEveryRow() {
        let ctx = SettingsSearchContext(form: SettingsConfigForm(), isLocalHost: true, capabilities: [])
        let ids = GeneralPage.searchEntries(ctx).map(\.id)
        #expect(ids == GeneralRow.allCases.filter { (FeatureFlags.webhooks || $0 != .activityWebhooks) && $0.isAvailable }.map(\.id))
        #expect(Set(ids).count == ids.count)
    }
}

@MainActor
struct GeneralCategoryTextTests {
    @Test func detailShowsExtensionsOrRegex() {
        let ext = CustomCategoryDto(id: "custom_1", name: "Books", extensions: ["epub", "mobi"])
        #expect(GeneralCategoryText.detail(ext) == ".epub, .mobi")
        let regex = CustomCategoryDto(id: "custom_2", name: "R", matchMode: "regex", regexPattern: ".*\\.pdf$")
        #expect(GeneralCategoryText.detail(regex)?.hasSuffix(": .*\\.pdf$") == true)
        let url = CustomCategoryDto(id: "custom_3", name: "U", matchMode: "url", urlPattern: "https://*.example.com/*")
        #expect(GeneralCategoryText.detail(url)?.hasSuffix(": https://*.example.com/*") == true)
        let all = CustomCategoryDto.builtinDefaults[0]
        #expect(GeneralCategoryText.detail(all) == nil)
    }

    @Test func builtinUsesTheLocalizedLabelAndCustomUsesItsName() {
        let video = CustomCategoryDto.builtinDefaults[1]
        #expect(GeneralCategoryText.displayName(video) == L("categoryVideo"))
        #expect(GeneralCategoryText.displayName(CustomCategoryDto(id: "custom_3", name: "Books")) == "Books")
    }
}
