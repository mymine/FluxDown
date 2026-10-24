import FluxDomain
import Foundation
import Network
import UIKit
import UserNotifications
import os

// 「此设备」诊断（03-settings §12）：只检查这台手机本身——通知授权、后台、存储、链接声明、网络、本地网络、引擎连接。
// 级别判定是纯函数（`MobileChecks`），探测（写入 / 空闲空间 / `NWPathMonitor`）在 `MobileCheckRunner`。

nonisolated enum MobileCheckID: String, CaseIterable, Sendable {
    case notifications, background, storage, network, localNetwork, engine
    #if FULL_FLAVOR
    case links
    #endif

    var titleKey: String {
        switch self {
        case .notifications: "mobileDiagCheckNotifications"
        case .background: "mobileDiagCheckBackground"
        case .storage: "mobileDiagCheckStorage"
        #if FULL_FLAVOR
        case .links: "mobileDiagCheckLinks"
        #endif
        case .network: "mobileDiagCheckNetwork"
        case .localNetwork: "mobileDiagCheckLocalNetwork"
        case .engine: "mobileDiagCheckEngine"
        }
    }
}

/// 一条设备检查结果（文案已本地化）。
nonisolated struct MobileCheck: Identifiable, Equatable, Sendable {
    var id: MobileCheckID
    var level: DiagnosticLevel
    var detail: String
    /// 可展开的处理建议；空 = 无。
    var hint: String = ""
    /// 提供「打开系统设置」修复。
    var canOpenSettings = false

    var title: String { L(id.titleKey) }
}

/// 一次性网络路径快照（`NWPath` 的 Sendable 投影）。
nonisolated struct MobileNetworkSnapshot: Equatable, Sendable {
    nonisolated enum Reachability: Sendable { case satisfied, unsatisfied, requiresConnection }
    nonisolated enum Interface: Sendable { case wifi, cellular, wired, other, none }

    var reachability: Reachability
    var interface: Interface
    /// 计费网络（蜂窝 / 个人热点）。
    var isExpensive: Bool
    /// 低数据模式。
    var isConstrained: Bool
    var supportsIPv4: Bool
    var supportsIPv6: Bool
}

nonisolated enum MobileChecks {
    /// 剩余空间低于此值 = 警告 / 异常（字节）。
    static let lowStorageWarning: UInt64 = 1 << 30
    static let lowStorageError: UInt64 = 100 << 20

    /// 级别严重度排序（unknown 按 info 处理）。
    static func severity(_ level: DiagnosticLevel) -> Int {
        switch level {
        case .error: 3
        case .warn: 2
        case .info, .unknown: 1
        case .ok: 0
        }
    }

    static func worst(_ levels: [DiagnosticLevel]) -> DiagnosticLevel {
        levels.max { severity($0) < severity($1) } ?? .ok
    }

    // MARK: 通知

    static func notifications(_ status: UNAuthorizationStatus) -> MobileCheck {
        switch status {
        case .authorized:
            return MobileCheck(id: .notifications, level: .ok, detail: L("mobileDiagNotifAllowed"))
        case .provisional, .ephemeral:
            return MobileCheck(
                id: .notifications, level: .info, detail: L("mobileDiagNotifQuiet"),
                hint: L("mobileDiagNotifQuietHint"), canOpenSettings: true
            )
        case .notDetermined:
            return MobileCheck(
                id: .notifications, level: .info, detail: L("mobileDiagNotifUnasked"), hint: L("mobileDiagNotifUnaskedHint")
            )
        case .denied:
            return MobileCheck(
                id: .notifications, level: .warn, detail: L("mobileDiagNotifDenied"),
                hint: L("mobileDiagNotifDeniedHint"), canOpenSettings: true
            )
        @unknown default:
            return MobileCheck(id: .notifications, level: .info, detail: L("mobileDiagNotifUnasked"))
        }
    }

    // MARK: 后台

    static func background(refresh: UIBackgroundRefreshStatus, lowPower: Bool, continueInBackground: Bool) -> MobileCheck {
        var levels: [DiagnosticLevel] = []
        var lines: [String] = []
        var hints: [String] = []

        if continueInBackground {
            lines.append(L("mobileDiagBgContinueOn"))
            levels.append(.ok)
        } else {
            lines.append(L("mobileDiagBgContinueOff"))
            levels.append(.info)
            hints.append(L("mobileDiagBgContinueOffHint"))
        }
        switch refresh {
        case .available:
            lines.append(L("mobileDiagBgRefreshOn"))
            levels.append(.ok)
        case .denied, .restricted:
            lines.append(L("mobileDiagBgRefreshOff"))
            // 后台下载关闭时，后台刷新是否可用无关紧要。
            levels.append(continueInBackground ? .warn : .info)
            if continueInBackground { hints.append(L("mobileDiagBgRefreshOffHint")) }
        @unknown default:
            break
        }
        if lowPower {
            lines.append(L("mobileDiagBgLowPower"))
            levels.append(.warn)
            hints.append(L("mobileDiagBgLowPowerHint"))
        }
        return MobileCheck(
            id: .background, level: worst(levels), detail: lines.joined(separator: "\n"),
            hint: hints.joined(separator: "\n"), canOpenSettings: !hints.isEmpty
        )
    }

    // MARK: 存储

    static func storage(writable: Bool, freeBytes: UInt64?) -> MobileCheck {
        guard writable else {
            return MobileCheck(
                id: .storage, level: .error, detail: L("mobileDiagStorageNotWritable"), hint: L("mobileDiagStorageNotWritableHint")
            )
        }
        guard let freeBytes else {
            return MobileCheck(id: .storage, level: .info, detail: L("mobileDiagStorageWritableUnknown"))
        }
        let free = Format.bytes(unsigned: freeBytes).description
        let detail = L("mobileDiagStorageWritableFree", ["free": free])
        if freeBytes < lowStorageError {
            return MobileCheck(id: .storage, level: .error, detail: detail, hint: L("mobileDiagStorageLowHint"))
        }
        if freeBytes < lowStorageWarning {
            return MobileCheck(id: .storage, level: .warn, detail: detail, hint: L("mobileDiagStorageLowHint"))
        }
        return MobileCheck(id: .storage, level: .ok, detail: detail)
    }

    #if FULL_FLAVOR
    // MARK: 链接与文件

    /// Info.plist `CFBundleURLTypes` 声明的全部 URL scheme（小写）。
    static func declaredURLSchemes(in info: [String: Any]) -> Set<String> {
        let types = info["CFBundleURLTypes"] as? [[String: Any]] ?? []
        var schemes: Set<String> = []
        for type in types {
            for scheme in type["CFBundleURLSchemes"] as? [String] ?? [] { schemes.insert(scheme.lowercased()) }
        }
        return schemes
    }

    /// Info.plist 是否声明了 `.torrent` 文档类型（`CFBundleDocumentTypes` 的扩展名或内容类型）。
    static func declaresTorrentDocument(in info: [String: Any]) -> Bool {
        let types = info["CFBundleDocumentTypes"] as? [[String: Any]] ?? []
        return types.contains { type in
            let extensions = (type["CFBundleTypeExtensions"] as? [String] ?? []).map { $0.lowercased() }
            let contentTypes = (type["LSItemContentTypes"] as? [String] ?? []).map { $0.lowercased() }
            return extensions.contains("torrent") || contentTypes.contains { $0.contains("torrent") }
        }
    }

    /// Info.plist `LSApplicationQueriesSchemes`（只有列在这里的 scheme 才能用 `canOpenURL` 探测）。
    static func queryableSchemes(in info: [String: Any]) -> Set<String> {
        Set((info["LSApplicationQueriesSchemes"] as? [String] ?? []).map { $0.lowercased() })
    }

    /// `handlers[scheme]`：`canOpenURL` 的结果；仅对可查询的 scheme 提供，缺省 = 只能看 Info.plist 声明。
    static func links(declaredSchemes: Set<String>, torrentDeclared: Bool, handlers: [String: Bool]) -> MobileCheck {
        var levels: [DiagnosticLevel] = []
        var lines: [String] = []
        for scheme in ["magnet", "ed2k"] {
            let declared = declaredSchemes.contains(scheme)
            let reachable = handlers[scheme] ?? declared
            let ok = declared && reachable
            lines.append("\(scheme):  " + L(ok ? "mobileDiagLinkDeclared" : "mobileDiagLinkMissing"))
            levels.append(ok ? .ok : .warn)
        }
        lines.append(L("mobileDiagLinkTorrent") + "  " + L(torrentDeclared ? "mobileDiagLinkDeclared" : "mobileDiagLinkMissing"))
        levels.append(torrentDeclared ? .ok : .info)
        let level = worst(levels)
        return MobileCheck(
            id: .links, level: level, detail: lines.joined(separator: "\n"),
            hint: level == .warn ? L("mobileDiagLinksHint") : ""
        )
    }
    #endif

    // MARK: 网络

    static func network(_ snapshot: MobileNetworkSnapshot?) -> MobileCheck {
        guard let snapshot else {
            return MobileCheck(id: .network, level: .info, detail: L("mobileDiagNetUnknown"))
        }
        guard snapshot.reachability == .satisfied else {
            return MobileCheck(
                id: .network, level: .error, detail: L("mobileDiagNetNone"), hint: L("mobileDiagNetNoneHint")
            )
        }
        let name: String = switch snapshot.interface {
        case .wifi: L("mobileDiagNetWifi")
        case .cellular: L("mobileDiagNetCellular")
        case .wired: L("mobileDiagNetWired")
        case .other: L("mobileDiagNetOther")
        case .none: L("mobileDiagNetUnknown")
        }
        var families: [String] = []
        if snapshot.supportsIPv4 { families.append("IPv4") }
        if snapshot.supportsIPv6 { families.append("IPv6") }
        var parts = [name]
        if !families.isEmpty { parts.append(families.joined(separator: " + ")) }
        var level = DiagnosticLevel.ok
        var hints: [String] = []
        if snapshot.isExpensive {
            parts.append(L("mobileDiagNetMetered"))
            level = .info
            hints.append(L("mobileDiagNetMeteredHint"))
        }
        if snapshot.isConstrained {
            parts.append(L("mobileDiagNetLowData"))
            level = .warn
            hints.append(L("mobileDiagNetLowDataHint"))
        }
        return MobileCheck(
            id: .network, level: level, detail: parts.joined(separator: " · "), hint: hints.joined(separator: "\n"),
            canOpenSettings: snapshot.isConstrained
        )
    }

    // MARK: 本地网络

    /// iOS 不公开「本地网络」授权状态：只给说明与设置入口（`info`，不判定好坏）。
    static func localNetwork() -> MobileCheck {
        MobileCheck(
            id: .localNetwork, level: .info, detail: L("mobileDiagLocalNetDetail"),
            hint: L("mobileDiagLocalNetHint"), canOpenSettings: true
        )
    }

    // MARK: 引擎

    /// `failureText`：`connection == .failed` 时的错误描述（`ErrorText.describe`）。
    static func engine(connection: Connection, info: HostInfo?, failureText: String?) -> MobileCheck {
        let version = info.map { L("mobileDiagEngineVersion", ["version": $0.serviceVersion, "protocol": $0.protocolVersion]) }
        switch connection {
        case .live:
            return MobileCheck(id: .engine, level: .ok, detail: [L("mobileDiagEngineConnected"), version].compactMap { $0 }.joined(separator: " · "))
        case .connecting:
            return MobileCheck(id: .engine, level: .info, detail: L("mobileSettingsConnConnecting"))
        case .stale:
            return MobileCheck(
                id: .engine, level: .warn, detail: L("mobileSettingsConnStale"), hint: L("mobileDiagEngineStaleHint")
            )
        case .failed:
            return MobileCheck(
                id: .engine, level: .error, detail: failureText ?? L("mobileSettingsConnFailed"),
                hint: L("mobileDiagEngineFailedHint")
            )
        }
    }

    /// 汇总：warn + error 的条数。
    static func issueCount(_ checks: [MobileCheck]) -> Int {
        checks.filter { $0.level.isIssue }.count
    }
}

// MARK: - 探测

nonisolated enum MobileProbes {
    private static let log = Logger(subsystem: "com.fluxdown.app", category: "diagnostics")

    /// 真实写入探测：在目录里建一个临时文件再删除。
    @concurrent
    static func isWritable(_ directory: URL) async -> Bool {
        let probe = directory.appendingPathComponent(".fluxdown-probe-\(UUID().uuidString)")
        do {
            try Data("ok".utf8).write(to: probe, options: .atomic)
        } catch {
            log.notice("write probe failed: \(error.localizedDescription, privacy: .public)")
            return false
        }
        do {
            try FileManager.default.removeItem(at: probe)
        } catch {
            log.notice("write probe cleanup failed: \(error.localizedDescription, privacy: .public)")
        }
        return true
    }

    /// 设备卷可用空间（含系统可清理的缓存，同「设置 › 储存空间」）。
    @concurrent
    static func deviceFreeBytes(at url: URL) async -> UInt64? {
        do {
            let values = try url.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
            return values.volumeAvailableCapacityForImportantUsage.flatMap { $0 >= 0 ? UInt64($0) : nil }
        } catch {
            log.notice("free space query failed: \(error.localizedDescription, privacy: .public)")
            return nil
        }
    }

    /// `NWPathMonitor` 一次性快照（取首个回调；`timeout` 内没有则 nil）。
    @concurrent
    static func networkSnapshot(timeout: Duration = .seconds(2)) async -> MobileNetworkSnapshot? {
        let monitor = NWPathMonitor()
        let stream = AsyncStream<MobileNetworkSnapshot> { continuation in
            monitor.pathUpdateHandler = { path in continuation.yield(Self.snapshot(of: path)) }
            continuation.onTermination = { _ in monitor.cancel() }
            monitor.start(queue: DispatchQueue(label: "com.fluxdown.app.diagnostics.path"))
        }
        return await withTaskGroup(of: MobileNetworkSnapshot?.self) { group in
            group.addTask {
                for await snapshot in stream { return snapshot }
                return nil
            }
            group.addTask {
                do {
                    try await Task.sleep(for: timeout)
                } catch {
                    return nil
                }
                return nil
            }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
    }

    private static func snapshot(of path: NWPath) -> MobileNetworkSnapshot {
        let reachability: MobileNetworkSnapshot.Reachability = switch path.status {
        case .satisfied: .satisfied
        case .requiresConnection: .requiresConnection
        default: .unsatisfied
        }
        let interface: MobileNetworkSnapshot.Interface = if path.usesInterfaceType(.wifi) {
            .wifi
        } else if path.usesInterfaceType(.cellular) {
            .cellular
        } else if path.usesInterfaceType(.wiredEthernet) {
            .wired
        } else if path.status == .satisfied {
            .other
        } else {
            .none
        }
        return MobileNetworkSnapshot(
            reachability: reachability, interface: interface, isExpensive: path.isExpensive,
            isConstrained: path.isConstrained, supportsIPv4: path.supportsIPv4, supportsIPv6: path.supportsIPv6
        )
    }
}

/// 收集全部设备检查（异步探测并发执行）。
@MainActor
enum MobileCheckRunner {
    static func run(container: AppContainer) async -> [MobileCheck] {
        let service = NotificationService.shared
        await service.refreshAuthorization()

        async let writable = MobileProbes.isWritable(LocalPaths.documents)
        async let deviceFree = MobileProbes.deviceFreeBytes(at: LocalPaths.documents)
        async let path = MobileProbes.networkSnapshot()

        let state = container.store.state
        // 本机引擎自己量的剩余空间优先；远端主机的 `diskFreeBytes` 是服务器磁盘，不能算作这台手机的。
        let engineFree = container.isLocalHost ? state.stats.diskFreeBytes : nil
        let measuredFree = await deviceFree
        let free = engineFree ?? measuredFree

        var checks: [MobileCheck] = [
            MobileChecks.notifications(service.authorization),
            MobileChecks.background(
                refresh: UIApplication.shared.backgroundRefreshStatus,
                lowPower: ProcessInfo.processInfo.isLowPowerModeEnabled,
                continueInBackground: DeviceSettings.shared.continueInBackground
            ),
            MobileChecks.storage(writable: await writable, freeBytes: free),
        ]
        #if FULL_FLAVOR
        let info = Bundle.main.infoDictionary ?? [:]
        let queryable = MobileChecks.queryableSchemes(in: info)
        var handlers: [String: Bool] = [:]
        for scheme in ["magnet", "ed2k"] where queryable.contains(scheme) {
            if let url = URL(string: "\(scheme):?") { handlers[scheme] = UIApplication.shared.canOpenURL(url) }
        }
        checks.append(MobileChecks.links(
            declaredSchemes: MobileChecks.declaredURLSchemes(in: info),
            torrentDeclared: MobileChecks.declaresTorrentDocument(in: info),
            handlers: handlers
        ))
        #endif

        var failureText: String?
        if case let .failed(error) = state.connection { failureText = ErrorText.describe(error) }

        checks.append(MobileChecks.network(await path))
        checks.append(MobileChecks.localNetwork())
        checks.append(MobileChecks.engine(connection: state.connection, info: state.info, failureText: failureText))
        return checks
    }
}
