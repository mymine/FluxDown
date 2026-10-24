import FluxDomain
import Foundation

/// 设备列表的纯展示规则（排序 / 平台图标与名称 / 副标题），同 Android `DevicesCommon.kt` + `DevicesScreen.kt`。
enum DevicePresentation {
    /// 平台 → SF Symbol。其它设备一律用通用图形（`smartphone`）：`iphone` 等 Apple 产品符号只能指本机。
    static func symbol(platform: String?) -> String {
        switch platform?.lowercased() {
        case "android", "ios": "smartphone"
        case "macos": "laptopcomputer"
        case "windows": "desktopcomputer"
        case "linux": "apple.terminal"
        case "web": "globe"
        default: "network"
        }
    }

    /// 平台显示名（`accountDevicePlatform*`）；未知平台原样显示，空 = nil。
    static func label(platform: String?) -> String? {
        switch platform?.lowercased() {
        case nil, "": nil
        case "windows": L("accountDevicePlatformWindows")
        case "macos": L("accountDevicePlatformMacos")
        case "linux": L("accountDevicePlatformLinux")
        case "android": L("accountDevicePlatformAndroid")
        case "ios": L("accountDevicePlatformIos")
        case "web": L("accountDevicePlatformWeb")
        default: platform
        }
    }

    /// 「在线 / 离线 · 平台 · vX」。
    static func subtitle(online: Bool, platform: String?, version: String?) -> String {
        subtitle(status: L(online ? "deviceOnline" : "deviceOffline"), platform: platform, version: version)
    }

    /// 「状态 · 平台 · vX」：状态词由调用方决定（云端 presence 未知时显示「状态未知」）。
    static func subtitle(status: String, platform: String?, version: String?) -> String {
        let trimmed = version?.trimmingCharacters(in: .whitespacesAndNewlines)
        return [
            status,
            label(platform: platform),
            trimmed.flatMap { $0.isEmpty ? nil : "v\($0)" },
        ]
        .compactMap { $0 }
        .joined(separator: " · ")
    }

    /// 云端已信任设备：本机在前 → 在线 → 离线，同组按名称（不区分大小写）。
    static func sorted(cloud devices: [CloudDevice]) -> [CloudDevice] {
        devices.enumerated().sorted { lhs, rhs in
            if lhs.element.isCurrent != rhs.element.isCurrent { return lhs.element.isCurrent }
            if lhs.element.isOnline != rhs.element.isOnline { return lhs.element.isOnline }
            return orderedBefore(lhs.element.name, lhs.offset, rhs.element.name, rhs.offset)
        }.map(\.element)
    }

    /// 局域网已配对设备：在线优先，其次名称（不区分大小写）。
    static func sorted(link devices: [LinkDevice]) -> [LinkDevice] {
        devices.enumerated().sorted { lhs, rhs in
            if lhs.element.online != rhs.element.online { return lhs.element.online }
            return orderedBefore(lhs.element.name, lhs.offset, rhs.element.name, rhs.offset)
        }.map(\.element)
    }

    private static func orderedBefore(_ a: String, _ ai: Int, _ b: String, _ bi: Int) -> Bool {
        switch a.compare(b, options: .caseInsensitive) {
        case .orderedAscending: true
        case .orderedDescending: false
        case .orderedSame: ai < bi
        }
    }
}

/// 当前主机卡的派生事实（任务状态计数 O(n) 一次遍历）。
nonisolated struct HostFacts: Equatable {
    var version: String?
    var diskFree: UInt64?
    var active: Int
    var waiting: Int
    var completed: Int
    var paused: Int
    var failed: Int

    init(state: HostState) {
        var completed = 0
        var paused = 0
        var failed = 0
        for task in state.tasks {
            switch task.status {
            case .completed: completed += 1
            case .paused: paused += 1
            case .failed: failed += 1
            default: break
            }
        }
        version = state.info?.serviceVersion
        diskFree = state.stats.diskFreeBytes
        active = Int(state.stats.activeTasks)
        waiting = Int(state.stats.pendingTasks)
        self.completed = completed
        self.paused = paused
        self.failed = failed
    }

    var hasBreakdown: Bool { waiting + completed + paused + failed > 0 }
}
