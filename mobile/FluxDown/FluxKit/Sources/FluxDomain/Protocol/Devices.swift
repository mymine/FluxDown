import Foundation

// 云端受信任设备（`agent.device.*`）。镜像 `native/protocol/src/agent.rs::CloudDevice` / `PathStyle`，
// 规则镜像 Web `deviceList.ts`、`pages/downloads/dialogs/target.ts` 与 `crates/downloads/src/model/dispatch.rs`。
//
// 与 `Model/Models.swift` 里的 `CloudDevice`（快照里的只读子集，没有云端行 id）不同：`CloudDeviceRecord` 是协议完整形状，
// `rename` / `delete` 需要它的 `id`（不是 `deviceId`）。

/// 设备本地文件路径的书写风格，决定远程下发时保存目录的合法形态。
public enum PathStyle: WireStringEnum {
    /// `C:\dir` / `\\server\share`。
    case windows
    /// `/dir`。
    case posix
    /// 对端发送了本端不认识的风格。
    case unknown(String)

    public init(wire: String) {
        switch wire {
        case "windows": self = .windows
        case "posix": self = .posix
        default: self = .unknown(wire)
        }
    }

    public var wire: String {
        switch self {
        case .windows: "windows"
        case .posix: "posix"
        case let .unknown(raw): raw
        }
    }

    /// 按设备平台名推断（`windows` / `macos` / `linux` / `android` / `ios` …）；未知平台 nil。
    public static func from(platform: String) -> PathStyle? {
        switch platform.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "windows", "win32": .windows
        case "macos", "darwin", "linux", "android", "ios", "freebsd", "openbsd", "netbsd": .posix
        default: nil
        }
    }

    /// `path` 是否为该风格下的绝对路径（`unknown` 恒 false）。
    public func isAbsolute(_ path: String) -> Bool {
        let path = path.trimmingCharacters(in: .whitespacesAndNewlines)
        switch self {
        case .windows:
            let bytes = Array(path.utf8)
            let isLetter: (UInt8) -> Bool = { ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122) }
            let drive = bytes.count >= 3 && isLetter(bytes[0]) && bytes[1] == UInt8(ascii: ":")
                && (bytes[2] == UInt8(ascii: "\\") || bytes[2] == UInt8(ascii: "/"))
            return drive || path.hasPrefix("\\\\")
        case .posix:
            return path.hasPrefix("/")
        case .unknown:
            return false
        }
    }

    /// 校验失败提示里的路径示例。
    public static func example(_ style: PathStyle?) -> String {
        if case .windows? = style { return "D:\\Downloads" }
        return "/home/user/Downloads"
    }

    /// 设备自报的路径风格；缺失 / 不认识时按平台推断。
    public static func effective(reported: PathStyle?, platform: String?) -> PathStyle? {
        switch reported {
        case .windows?, .posix?: return reported
        case .unknown?, nil: return platform.flatMap(from(platform:))
        }
    }
}

/// `agent.device.list` 元素 / `agent.session.device`：受信任设备公开投影（`CloudDevice`）。
public struct CloudDeviceRecord: Sendable, Hashable, Identifiable, Codable {
    /// 云端行 id（`rename` / `delete` 用它；不是 `deviceId`）。
    public var id: String
    /// 设备 id（`agent.remote.dispatch.toDevice` 用它）。
    public var deviceId: String
    public var name: String
    public var platform: String?
    public var createdAt: String
    public var lastSeenAt: String
    public var lastIp: String?
    public var appVersion: String?
    public var isOnline: Bool
    public var isCurrent: Bool
    /// 设备自报的默认下载目录（目标设备本地路径）。
    public var defaultSaveDir: String?
    public var pathStyle: PathStyle?

    public init(
        id: String, deviceId: String, name: String = "", platform: String? = nil, createdAt: String = "",
        lastSeenAt: String = "", lastIp: String? = nil, appVersion: String? = nil, isOnline: Bool = false,
        isCurrent: Bool = false, defaultSaveDir: String? = nil, pathStyle: PathStyle? = nil
    ) {
        self.id = id
        self.deviceId = deviceId
        self.name = name
        self.platform = platform
        self.createdAt = createdAt
        self.lastSeenAt = lastSeenAt
        self.lastIp = lastIp
        self.appVersion = appVersion
        self.isOnline = isOnline
        self.isCurrent = isCurrent
        self.defaultSaveDir = defaultSaveDir
        self.pathStyle = pathStyle
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        deviceId = try c.decode(String.self, forKey: .deviceId)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        platform = try c.decodeIfPresent(String.self, forKey: .platform)
        createdAt = try c.decodeIfPresent(String.self, forKey: .createdAt) ?? ""
        lastSeenAt = try c.decodeIfPresent(String.self, forKey: .lastSeenAt) ?? ""
        lastIp = try c.decodeIfPresent(String.self, forKey: .lastIp)
        appVersion = try c.decodeIfPresent(String.self, forKey: .appVersion)
        isOnline = try c.decodeIfPresent(Bool.self, forKey: .isOnline) ?? false
        isCurrent = try c.decodeIfPresent(Bool.self, forKey: .isCurrent) ?? false
        defaultSaveDir = try c.decodeIfPresent(String.self, forKey: .defaultSaveDir)
        pathStyle = try c.decodeIfPresent(PathStyle.self, forKey: .pathStyle)
    }

    /// 有效路径风格（自报 → 平台推断）。
    public var effectivePathStyle: PathStyle? { PathStyle.effective(reported: pathStyle, platform: platform) }
}

/// `agent.device.list` 结果（同时刷新快照里的 `cloudDevices`）。
public struct CloudDeviceList: Sendable, Hashable, Codable {
    public var devices: [CloudDeviceRecord]

    public init(devices: [CloudDeviceRecord]) { self.devices = devices }
}

public struct DeviceRenameParams: Sendable, Hashable, Codable {
    /// `CloudDeviceRecord.id`（不是 deviceId）。
    public var id: String
    /// 1–64 字符。
    public var name: String

    public init(id: String, name: String) {
        self.id = id
        self.name = name
    }
}

public struct DeviceIdParams: Sendable, Hashable, Codable {
    public var id: String

    public init(id: String) { self.id = id }
}

/// `agent.remote.reconnect` 结果：`accepted` 不表示已连上。
public struct RemoteReconnectResult: Sendable, Hashable, Codable {
    public var accepted: Bool

    public init(accepted: Bool) { self.accepted = accepted }
}

// MARK: - 纯规则

/// 设备列表 / 下发目标的纯规则。
public enum DeviceRules {
    /// 云端设备排序（`crates/account/src/device_list.rs::sorted`）：本机在前 → 在线（presence 已知时）→ 名称（不区分大小写）；
    /// 稳定（同名保持原序）。
    public static func sorted(_ devices: [CloudDeviceRecord], presenceKnown: Bool) -> [CloudDeviceRecord] {
        devices.enumerated().sorted { lhs, rhs in
            let a = lhs.element, b = rhs.element
            if a.isCurrent != b.isCurrent { return a.isCurrent }
            let aOnline = presenceKnown && a.isOnline, bOnline = presenceKnown && b.isOnline
            if aOnline != bOnline { return aOnline }
            switch a.name.compare(b.name, options: .caseInsensitive) {
            case .orderedAscending: return true
            case .orderedDescending: return false
            case .orderedSame: return lhs.offset < rhs.offset
            }
        }.map(\.element)
    }

    /// 「其他设备」：账号设备去掉本机。
    public static func others(_ devices: [CloudDeviceRecord]) -> [CloudDeviceRecord] {
        devices.filter { !$0.isCurrent }
    }

    /// 本机在 FluxCloud 的 deviceId：优先会话，其次设备列表里的 `isCurrent`。
    public static func currentDeviceId(session: AgentSessionDto?, devices: [CloudDeviceRecord]) -> String? {
        session?.device.deviceId ?? devices.first(where: \.isCurrent)?.deviceId
    }

    /// 搜索（名称 / 平台，不区分大小写与变音）；空查询返回全部。
    public static func filter(_ devices: [CloudDeviceRecord], query: String) -> [CloudDeviceRecord] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return devices }
        let options: String.CompareOptions = [.caseInsensitive, .diacriticInsensitive]
        return devices.filter {
            $0.name.range(of: needle, options: options) != nil || ($0.platform?.range(of: needle, options: options) != nil)
        }
    }

    /// 远端保存目录输入的校验结果（`dispatch.rs::check_remote_save_dir`）。
    public enum SaveDirCheck: Sendable, Hashable {
        /// 输入为空：提交时不带 `saveDir`，目标设备用它自己的默认目录。
        case useDefault
        /// 合法目录（已去首尾空白）。
        case explicit(String)
        /// 不是目标路径风格下的绝对路径。
        case invalid
    }

    /// 风格未知（Web 端 / 新平台）时无法判断，交给目标设备回退默认目录，只要求非空即放行。
    public static func checkSaveDir(_ input: String, style: PathStyle?) -> SaveDirCheck {
        let dir = input.trimmingCharacters(in: .whitespacesAndNewlines)
        if dir.isEmpty { return .useDefault }
        if let style {
            if case .unknown = style { return .explicit(dir) }
            return style.isAbsolute(dir) ? .explicit(dir) : .invalid
        }
        return .explicit(dir)
    }
}

/// 远程下发目标：云账号其他设备（经 FluxCloud）或局域网已配对设备（直连）。设备页下发与新建下载「下载到」共用，
/// 镜像 `crates/downloads/src/model/devices.rs::DeviceEntry`。
///
/// 同一台机器既是同账号云设备又是直连设备时合并为一条云条目（`linkFingerprint` 挂接）：直连在线走局域网直连，否则走云。
public struct DispatchTarget: Sendable, Hashable, Identifiable {
    public enum Kind: Sendable, Hashable { case cloud, link }

    public var kind: Kind
    /// 云设备 `deviceId`（`RemoteDispatchParams.toDevice`）/ 已配对设备指纹（`LinkDispatchParams.fingerprint`）。
    public var deviceId: String
    public var name: String
    /// 在线状态；nil = 未知（云端 presence 不可信 / 本地服务未就绪）。挂接条目 = 云与直连任一在线即在线。
    public var online: Bool?
    /// 目标自报的默认下载目录（已去首尾空白；空 = nil）。
    public var defaultSaveDir: String?
    /// 目标的有效路径风格（自报 → 平台推断）；未知为 nil。
    public var pathStyle: PathStyle?
    /// 挂接到该云条目的直连设备指纹（仅云条目）。
    public var linkFingerprint: String?
    /// 挂接直连设备的在线状态；nil = 未知（本地服务未就绪）。
    public var linkOnline: Bool?

    public init(
        kind: Kind, deviceId: String, name: String, online: Bool?, defaultSaveDir: String?, pathStyle: PathStyle?,
        linkFingerprint: String? = nil, linkOnline: Bool? = nil
    ) {
        self.kind = kind
        self.deviceId = deviceId
        self.name = name
        self.online = online
        let dir = defaultSaveDir?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        self.defaultSaveDir = dir.isEmpty ? nil : dir
        self.pathStyle = pathStyle
        self.linkFingerprint = linkFingerprint
        self.linkOnline = linkOnline
    }

    /// 云端完整记录（设备页）；presence 不可信时在线状态记为未知。`link` = 挂接的直连记录（同账号同机）。
    public init(cloud record: CloudDeviceRecord, presenceKnown: Bool, link: LinkDeviceInfo? = nil) {
        let cloudOnline: Bool? = presenceKnown ? record.isOnline : nil
        let attached = link.flatMap { $0.revoked || $0.fingerprint.isEmpty ? nil : $0 }
        self.init(
            kind: .cloud,
            deviceId: record.deviceId,
            name: record.name,
            online: attached.map { DeviceRules.combineOnline(cloudOnline, $0.online) } ?? cloudOnline,
            defaultSaveDir: Self.nonBlank(record.defaultSaveDir) ?? attached?.defaultSaveDir,
            pathStyle: record.effectivePathStyle ?? attached?.effectivePathStyle,
            linkFingerprint: attached?.fingerprint,
            linkOnline: attached?.online
        )
    }

    /// 局域网已配对设备完整信息（设备页）。
    public init(link info: LinkDeviceInfo) {
        self.init(
            kind: .link,
            deviceId: info.fingerprint,
            name: info.name,
            online: info.online,
            defaultSaveDir: info.defaultSaveDir,
            pathStyle: info.effectivePathStyle
        )
    }

    private static func nonBlank(_ value: String?) -> String? {
        let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return trimmed.isEmpty ? nil : trimmed
    }

    /// 选择键：`cloud:<deviceId>` / `link:<fingerprint>`（挂接条目用 `cloud:`；同 GPUI `DispatchTarget::to_pref`）。
    public var id: String { (kind == .cloud ? "cloud:" : "link:") + deviceId }

    public var isCloud: Bool { kind == .cloud }

    /// 本次下发是否走局域网直连：独立直连条目，或云条目挂接的直连设备在线。
    public var routesViaLink: Bool { kind == .link || (linkFingerprint != nil && linkOnline == true) }

    /// 直连下发使用的指纹（`routesViaLink` 为 true 时有值）。
    public var dispatchFingerprint: String? {
        guard routesViaLink else { return nil }
        return kind == .link ? deviceId : linkFingerprint
    }

    /// 已知离线：云设备由云端排队、上线后执行；局域网设备直连送不到。
    public var isOffline: Bool { online == false }
}

extension DeviceRules {
    /// 挂接条目在线合并：任一为 true → true；否则任一未知 → 未知；否则 false。
    static func combineOnline(_ a: Bool?, _ b: Bool?) -> Bool? {
        if a == true || b == true { return true }
        if a == nil || b == nil { return nil }
        return false
    }

    /// 「下载到」候选（`crates/downloads/src/model/devices.rs::other_devices`），按 pairing-contract 合并规则：
    /// 云设备去掉本机与空 id、按 id 去重；已配对设备去掉空指纹与 revoked、按指纹去重；link 的 `cloudDeviceId`
    /// 命中尚未挂接的云条目时挂接到该条目，否则成为独立直连条目。顺序：云条目（含挂接）→ 独立直连；
    /// 名称为空用短码，多台同名（忽略大小写与首尾空白）时全部追加 ` · 短码`。
    ///
    /// - `cloudPresenceKnown`：云端在线状态可信（``CloudPresence/isKnown(_:localReady:)``），否则云设备在线未知；
    /// - `localReady`：本地服务已就绪，否则局域网设备在线未知。
    public static func dispatchTargets(
        cloud: [CloudDevice],
        link: [LinkDevice],
        cloudPresenceKnown: Bool,
        localReady: Bool
    ) -> [DispatchTarget] {
        var seenCloud = Set<String>()
        var seenLink = Set<String>()
        var targets: [DispatchTarget] = []
        var cloudIndex: [String: Int] = [:]
        for device in cloud where !device.isCurrent && !device.deviceId.isEmpty {
            guard seenCloud.insert(device.deviceId).inserted else { continue }
            cloudIndex[device.deviceId] = targets.count
            targets.append(DispatchTarget(
                kind: .cloud,
                deviceId: device.deviceId,
                name: device.name.trimmingCharacters(in: .whitespacesAndNewlines),
                online: cloudPresenceKnown && localReady ? device.isOnline : nil,
                defaultSaveDir: device.defaultSaveDir,
                pathStyle: device.effectivePathStyle
            ))
        }
        var standalone: [DispatchTarget] = []
        for device in link where !device.fingerprint.isEmpty && !device.revoked {
            guard seenLink.insert(device.fingerprint).inserted else { continue }
            let linkOnline: Bool? = localReady ? device.online : nil
            if let cloudId = device.cloudDeviceId, !cloudId.isEmpty,
               let index = cloudIndex[cloudId], targets[index].linkFingerprint == nil
            {
                targets[index].linkFingerprint = device.fingerprint
                targets[index].linkOnline = linkOnline
                targets[index].online = combineOnline(targets[index].online, linkOnline)
                if targets[index].defaultSaveDir == nil {
                    let dir = device.defaultSaveDir?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                    targets[index].defaultSaveDir = dir.isEmpty ? nil : dir
                }
                if targets[index].pathStyle == nil { targets[index].pathStyle = device.effectivePathStyle }
                continue
            }
            standalone.append(DispatchTarget(
                kind: .link,
                deviceId: device.fingerprint,
                name: device.name.trimmingCharacters(in: .whitespacesAndNewlines),
                online: linkOnline,
                defaultSaveDir: device.defaultSaveDir,
                pathStyle: device.effectivePathStyle
            ))
        }
        targets += standalone
        var counts: [String: Int] = [:]
        for target in targets { counts[target.name.lowercased(), default: 0] += 1 }
        for index in targets.indices {
            let name = targets[index].name
            if name.isEmpty {
                targets[index].name = shortCode(targets[index].deviceId)
            } else if counts[name.lowercased(), default: 0] > 1 {
                targets[index].name = "\(name) · \(shortCode(targets[index].deviceId))"
            }
        }
        return targets
    }

    /// 与同账号云设备合并为一条的直连记录指纹（`dispatchTargets` 的实际挂接结果；revoked / 本机 / 不在名册 / 已挂接的都不算）。
    /// 同 Android `DeviceRules.attachedLinkFingerprints`。
    public static func attachedLinkFingerprints(cloud: [CloudDevice], link: [LinkDevice]) -> Set<String> {
        Set(dispatchTargets(cloud: cloud, link: link, cloudPresenceKnown: false, localReady: false).compactMap(\.linkFingerprint))
    }

    /// 解析选择 / 持久化 id：`link:<fp>` 已挂接到某云条目时解析为该云条目；找不到返回 nil。
    public static func resolveTarget(id: String, in targets: [DispatchTarget]) -> DispatchTarget? {
        if let exact = targets.first(where: { $0.id == id }) { return exact }
        guard id.hasPrefix("link:") else { return nil }
        let fingerprint = String(id.dropFirst("link:".count))
        return targets.first { $0.linkFingerprint == fingerprint }
    }

    /// 设备 id 的短码（末 4 位 ASCII 字母数字），用于同名设备消歧。
    static func shortCode(_ id: String) -> String {
        let alnum = id.unicodeScalars.filter { $0.isASCII && CharacterSet.alphanumerics.contains($0) }
        return String(String.UnicodeScalarView(alnum.suffix(4)))
    }
}
