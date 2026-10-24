import FluxDomain
import FluxUI
import SwiftUI

// 下载域的文案 / 颜色映射（视图层）。键全部是 Android 同名 `R.string.<key>`。

extension StatusFolder {
    var title: String {
        switch self {
        case .all: L("tabAll")
        case .active: L("tabDownloading")
        case .completed: L("tabCompleted")
        case .seeding: L("statusSeeding")
        case .failed: L("tabError")
        case .paused: L("tabPaused")
        }
    }
}

extension GroupBy {
    var title: String {
        switch self {
        case .none: L("viewGroupNone")
        case .status: L("viewGroupStatus")
        case .date: L("viewGroupDate")
        case .type: L("viewGroupType")
        case .queue: L("viewGroupQueue")
        case .site: L("viewGroupSite")
        case .group: L("viewGroupGroup")
        }
    }
}

extension SortKey {
    var title: String {
        switch self {
        case .smart: L("viewSortSmart")
        case .created: L("viewSortCreated")
        case .name: L("viewSortName")
        case .size: L("viewSortSize")
        case .progress: L("viewSortProgress")
        case .speed: L("viewSortSpeed")
        case .status: L("viewSortStatus")
        }
    }
}

extension Density {
    var title: String {
        switch self {
        case .comfortable: L("viewDensityComfortable")
        case .compact: L("viewDensityCompact")
        }
    }
}

extension CardField {
    var title: String {
        switch self {
        case .size: L("colSize")
        case .speed: L("colSpeed")
        case .eta: L("colEta")
        case .protocol: L("colProtocol")
        case .site: L("colSource")
        case .queue: L("colQueue")
        case .created: L("colCreated")
        }
    }
}

extension TaskQueue {
    /// 内置队列名走 i18n；用户队列用其名称。
    var displayName: String {
        switch queueId {
        case "", TaskQueue.main: L("mainQueue")
        case TaskQueue.later: L("downloadLater")
        default: name
        }
    }
}

extension TaskCategory {
    /// 内置分类走 i18n（App / GPUI / Web 共用键），自定义用其名称。
    var displayName: String {
        switch builtinType {
        case "all": L("categoryAll")
        case "video": L("categoryVideo")
        case "audio": L("categoryAudio")
        case "document": L("categoryDocument")
        case "image": L("categoryImage")
        case "program": L("categoryProgram")
        case "archive": L("categoryArchive")
        case "other": L("categoryOther")
        default: name
        }
    }

    /// 分类色点（§3.C）：内置按类型，自定义统一用电子书色（青），与 Android `fileCategory()` 同规则。
    var tint: Color {
        switch builtinType {
        case "video": .fdKindVideo
        case "audio": .fdKindAudio
        case "document": .fdKindDocument
        case "image": .fdKindImage
        case "program": .fdKindProgram
        case "archive": .fdKindArchive
        case nil: .fdKindEbook
        default: .fdKindOther
        }
    }
}

extension SectionTitle {
    var resolved: String {
        switch self {
        case .inFlight: L("mobileSectionInFlight")
        case let .history(folder): folder?.title ?? L("mobileSectionHistory")
        case let .key(key): L(key)
        case let .text(text): text
        case let .category(category): category.displayName
        case let .queue(queue): queue.displayName
        }
    }
}

extension StatusTone {
    func color(accent: FluxAccent) -> Color {
        switch self {
        case .accent: accent.text
        case .secondary: .secondary
        case .failure: .fdStatusFailedText
        case .success: .fdStatusSeedingText
        case .warning: .fdStatusWarningText
        }
    }
}

extension TaskVisual {
    /// 圆环色（§1.3）：下载 / 准备 / 完成 = 强调色；排队 = 灰；暂停 = 灰；失败 = 红；文件已删除 = 橙。
    var ringTint: Color {
        switch self {
        case .downloading, .preparing, .verifying, .seeding, .completed: .accentColor
        case .queued, .pending: .fdStatusQueued
        case .paused: .fdStatusPaused
        case .failed: .fdStatusFailed
        case .missing: .fdStatusWarning
        }
    }

    var badge: KindBadge {
        switch self {
        case .completed, .seeding: .completed
        case .missing: .warning
        case .failed: .failed
        default: .none
        }
    }
}
