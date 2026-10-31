import FluxDomain
import Foundation

// 通用页（S3 / S3a）的纯逻辑：行目录、设备协同三态、链接打开方式（Info.plist 声明）、分类图标映射。
// 不依赖 UI / 主机，便于单测。

// MARK: - 行目录

/// 通用页的每一行。页面渲染、设置搜索共用这份目录（行 id = `.settingsRow(id)` 的 id）。
nonisolated enum GeneralRow: String, CaseIterable, Identifiable {
    case bgContinue, keepAwake
    case analytics, linkHandling
    case sidebarStatus, sidebarQueues, sidebarCategory
    case activityRss, activityWebhooks, activityTheme

    nonisolated enum Group: CaseIterable {
        case background, system, downloadsView, entries

        var titleKey: String {
            switch self {
            case .background: "mobileGeneralGroupBackground"
            case .system: "settingsGroupSystem"
            case .downloadsView: "mobileGeneralGroupDownloadsView"
            case .entries: "mobileGeneralGroupEntries"
            }
        }
    }

    var id: String { "general.\(rawValue)" }

    var group: Group {
        switch self {
        case .bgContinue, .keepAwake: .background
        case .analytics, .linkHandling: .system
        case .sidebarStatus, .sidebarQueues, .sidebarCategory: .downloadsView
        case .activityRss, .activityWebhooks, .activityTheme: .entries
        }
    }

    var titleKey: String {
        switch self {
        case .bgContinue: "mobileGeneralBgContinue"
        case .keepAwake: "keepAwakeWhileDownloading"
        case .analytics: "analyticsEnabled"
        case .linkHandling: "mobileGeneralLinkHandling"
        case .sidebarStatus: "showSidebarStatus"
        case .sidebarQueues: "showSidebarQueues"
        case .sidebarCategory: "showSidebarCategory"
        case .activityRss: "showActivityRss"
        case .activityWebhooks: "showActivityWebhooks"
        case .activityTheme: "showActivityTheme"
        }
    }

    var detailKey: String? {
        switch self {
        case .bgContinue: "mobileGeneralBgContinueDesc"
        case .keepAwake: "mobileGeneralKeepAwakeDesc"
        case .analytics: "analyticsEnabledDesc"
        case .linkHandling: "mobileGeneralLinkHandlingDesc"
        case .sidebarCategory: "showSidebarCategoryNestedDesc"
        case .sidebarStatus, .sidebarQueues, .activityRss, .activityWebhooks, .activityTheme: nil
        }
    }

    /// 目录键（agent 偏好）；设备本地 UserDefaults 行 / 只读块 / 分类列表没有。
    var preferenceKey: String? {
        switch self {
        case .keepAwake: "download.keep_awake"
        case .analytics: "analytics_enabled"
        case .sidebarStatus: "ui.show_sidebar_status"
        case .sidebarQueues: "ui.show_sidebar_queues"
        case .sidebarCategory: "ui.show_sidebar_category"
        case .activityRss: "ui.show_activity_rss"
        case .activityWebhooks: "ui.show_activity_webhooks"
        case .activityTheme: "ui.show_activity_theme"
        case .bgContinue, .linkHandling: nil
        }
    }

    /// 共享行组件用的设置项（仅有目录键的行）。
    var item: SettingsItem? {
        preferenceKey.map { SettingsItem(id: id, key: $0, titleKey: titleKey, detailKey: detailKey) }
    }

    var symbol: String {
        switch self {
        case .bgContinue: "arrow.down.app.fill"
        case .keepAwake: "sun.max.fill"
        case .analytics: "chart.bar.fill"
        case .linkHandling: "link"
        case .sidebarStatus, .sidebarQueues, .sidebarCategory: "sidebar.left"
        case .activityRss, .activityWebhooks, .activityTheme: "square.grid.2x2.fill"
        }
    }

    /// 「入口」分组实际展示的行（Webhook 活动开关受 `FeatureFlags.webhooks` 门控）。
    static var entriesRows: [GeneralRow] {
        FeatureFlags.webhooks ? [.activityRss, .activityWebhooks, .activityTheme] : [.activityRss, .activityTheme]
    }

    /// 本构建是否提供该行（store 构建没有 magnet / ed2k / .torrent 打开方式声明，不展示「链接与文件」）。
    var isAvailable: Bool {
        #if FULL_FLAVOR
        return true
        #else
        return self != .linkHandling
        #endif
    }
}

#if FULL_FLAVOR
// MARK: - 链接与文件打开方式

/// iOS 的打开方式是 Info.plist 的静态声明（无法检测是否被其它 App 抢占）；这里只如实读取本 App 的声明。
nonisolated enum GeneralLinkKind: String, CaseIterable, Identifiable, Sendable {
    case magnet, ed2k, torrent

    var id: String { rawValue }

    /// 协议 / 扩展名的字面写法（专有名词，不本地化）。
    var label: String {
        switch self {
        case .magnet: "magnet:"
        case .ed2k: "ed2k://"
        case .torrent: ".torrent"
        }
    }
}

enum GeneralLinkHandling {
    static let torrentTypeIdentifier = "org.bittorrent.torrent"

    /// 已声明的链接 / 文件类型。
    static func declared(in info: [String: Any]) -> Set<GeneralLinkKind> {
        var result: Set<GeneralLinkKind> = []
        let schemes = declaredSchemes(in: info)
        if schemes.contains("magnet") { result.insert(.magnet) }
        if schemes.contains("ed2k") { result.insert(.ed2k) }
        if declaresTorrentDocument(in: info) { result.insert(.torrent) }
        return result
    }

    /// `CFBundleURLTypes[].CFBundleURLSchemes`（小写）。
    static func declaredSchemes(in info: [String: Any]) -> Set<String> {
        let types = info["CFBundleURLTypes"] as? [[String: Any]] ?? []
        return Set(types.flatMap { ($0["CFBundleURLSchemes"] as? [String]) ?? [] }.map { $0.lowercased() })
    }

    /// `CFBundleDocumentTypes` 中有一项接收 `.torrent`：直接列出 `org.bittorrent.torrent`、
    /// 列出某个把 `torrent` 扩展名映射到自己的导入 / 导出类型，或用 `CFBundleTypeExtensions`。
    static func declaresTorrentDocument(in info: [String: Any]) -> Bool {
        var identifiers: Set<String> = [torrentTypeIdentifier]
        for key in ["UTImportedTypeDeclarations", "UTExportedTypeDeclarations"] {
            for declaration in info[key] as? [[String: Any]] ?? [] {
                guard let identifier = declaration["UTTypeIdentifier"] as? String,
                      let tags = declaration["UTTypeTagSpecification"] as? [String: Any] else { continue }
                if filenameExtensions(tags["public.filename-extension"]).contains("torrent") {
                    identifiers.insert(identifier)
                }
            }
        }
        let documents = info["CFBundleDocumentTypes"] as? [[String: Any]] ?? []
        return documents.contains { document in
            let contentTypes = document["LSItemContentTypes"] as? [String] ?? []
            if contentTypes.contains(where: identifiers.contains) { return true }
            let extensions = document["CFBundleTypeExtensions"] as? [String] ?? []
            return extensions.contains { $0.lowercased() == "torrent" }
        }
    }

    private static func filenameExtensions(_ value: Any?) -> Set<String> {
        switch value {
        case let text as String: [text.lowercased()]
        case let list as [String]: Set(list.map { $0.lowercased() })
        default: []
        }
    }
}
#endif

// MARK: - 分类图标

/// 分类图标 wire 名 → SF Symbol（03-settings §4.6）。wire 名原样保留（与 PC / Web / Android 互通）。
enum GeneralCategoryIcon {
    static func symbol(_ wire: String) -> String {
        switch wire {
        case "folders": "square.stack.fill"
        case "film": "film"
        case "music": "music.note"
        case "fileText": "text.document"
        case "image": "photo"
        case "archive": "archivebox"
        case "file": "document"
        case "code": "chevron.left.forwardslash.chevron.right"
        case "database": "cylinder"
        case "gamepad": "gamecontroller"
        case "globe": "globe"
        case "bookmark": "bookmark"
        case "box": "shippingbox"
        case "cpu": "cpu"
        case "disc": "opticaldisc"
        case "font": "textformat"
        case "hardDrive": "externaldrive"
        case "library": "books.vertical"
        case "package2": "cube"
        case "pen": "pencil"
        case "printer": "printer"
        case "smartphone": "smartphone"
        case "subtitles": "captions.bubble"
        case "type": "character"
        case "zap": "bolt"
        default: "document" // 未知 wire 名回退到「文件」（同 `file`）
        }
    }

    /// VoiceOver 名称的文案键。
    static func nameKey(_ wire: String) -> String { "mobileGeneralIconName_\(wire)" }
}

// MARK: - 分类行文案

enum GeneralCategoryText {
    /// 显示名：内置分类用本地化标签，自定义分类用其名称（同 GPUI `display_name`）。
    static func displayName(_ entry: CustomCategoryDto) -> String {
        if entry.isBuiltin, let type = entry.builtinType { return L(CategoryRules.builtinLabelKey(type)) }
        return entry.name
    }

    /// 详情行：`.ext, .ext` / `正则表达式: pattern` / `网址模式: pattern`；无规则（全部 / 其他）为 nil。
    static func detail(_ entry: CustomCategoryDto) -> String? {
        guard entry.hasMatchRules else { return nil }
        if entry.isRegex {
            return entry.regexPattern.isEmpty ? nil : "\(L("regexLabel")): \(entry.regexPattern)"
        }
        if entry.isUrl {
            return entry.urlPattern.isEmpty ? nil : "\(L("urlPatternLabel")): \(entry.urlPattern)"
        }
        return entry.extensions.isEmpty ? nil : entry.extensions.map { ".\($0)" }.joined(separator: ", ")
    }
}
