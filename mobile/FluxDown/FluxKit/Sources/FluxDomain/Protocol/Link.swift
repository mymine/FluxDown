import Foundation

// 局域网直连（L1，`agent.link.*`，无需登录）。镜像 `native/protocol/src/agent.rs`（`LinkPairingRequestDto` /
// `LinkPairingCodeDto` / 各 Params）与 `daemon.rs`（`LinkDiscoveredPeer` / `LinkDeviceInfo` / `LinkPair*Response`）；
// 规则镜像 Web `account/pairing.ts` 与 GPUI `crates/account/src/link.rs`。

/// 一台已配对设备的对外视图（`LinkDeviceInfo`）。严禁含 link_secret 等敏感字段。
public struct LinkDeviceInfo: Sendable, Hashable, Identifiable, Codable {
    public var fingerprint: String
    public var name: String
    public var platform: String?
    public var online: Bool
    /// Unix 时间（秒或毫秒，按量级判断，见 ``LinkRules/date(fromUnix:)``）。
    public var pairedAt: Int64
    public var lastSeenAt: Int64
    public var defaultSaveDir: String?
    public var pathStyle: PathStyle?
    /// 本机备注名（`name` 已是生效名，备注名优先）；仅用于重命名对话框预填。
    public var customName: String?
    public var appVersion: String?
    /// 对端已登录云账号时的云设备 id；与云名册同 id 时合并为一条。
    public var cloudDeviceId: String?
    /// 对端已解除配对：拒绝其入站请求、不可再下发，只能删除。
    public var revoked: Bool
    /// 响应方侧：该对端已获批控制**本机**（它是以「请求完全控制」发起并经本机用户批准的）。
    public var grantsControl: Bool
    /// 发起方侧：本机可控制该对端（本机以「请求完全控制」发起并配对完成）；可向本机 agent 申请控制凭据。
    public var controllable: Bool

    public init(
        fingerprint: String, name: String, platform: String? = nil, online: Bool = false, pairedAt: Int64 = 0,
        lastSeenAt: Int64 = 0, defaultSaveDir: String? = nil, pathStyle: PathStyle? = nil,
        customName: String? = nil, appVersion: String? = nil, cloudDeviceId: String? = nil, revoked: Bool = false,
        grantsControl: Bool = false, controllable: Bool = false
    ) {
        self.fingerprint = fingerprint
        self.name = name
        self.platform = platform
        self.online = online
        self.pairedAt = pairedAt
        self.lastSeenAt = lastSeenAt
        self.defaultSaveDir = defaultSaveDir
        self.pathStyle = pathStyle
        self.customName = customName
        self.appVersion = appVersion
        self.cloudDeviceId = cloudDeviceId
        self.revoked = revoked
        self.grantsControl = grantsControl
        self.controllable = controllable
    }

    public var id: String { fingerprint }

    public var effectivePathStyle: PathStyle? { PathStyle.effective(reported: pathStyle, platform: platform) }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        fingerprint = try c.decode(String.self, forKey: .fingerprint)
        name = try c.decode(String.self, forKey: .name)
        platform = try c.decodeIfPresent(String.self, forKey: .platform)
        online = try c.decodeIfPresent(Bool.self, forKey: .online) ?? false
        pairedAt = try c.decodeIfPresent(Int64.self, forKey: .pairedAt) ?? 0
        lastSeenAt = try c.decodeIfPresent(Int64.self, forKey: .lastSeenAt) ?? 0
        defaultSaveDir = try c.decodeIfPresent(String.self, forKey: .defaultSaveDir)
        pathStyle = try c.decodeIfPresent(PathStyle.self, forKey: .pathStyle)
        customName = try c.decodeIfPresent(String.self, forKey: .customName)
        appVersion = try c.decodeIfPresent(String.self, forKey: .appVersion)
        cloudDeviceId = try c.decodeIfPresent(String.self, forKey: .cloudDeviceId)
        revoked = try c.decodeIfPresent(Bool.self, forKey: .revoked) ?? false
        grantsControl = try c.decodeIfPresent(Bool.self, forKey: .grantsControl) ?? false
        controllable = try c.decodeIfPresent(Bool.self, forKey: .controllable) ?? false
    }
}

/// `agent.link.rename` 参数：`name` 省略 / 空白 = 清除备注名；非空 trim 后 1–64 字符、无控制字符。
public struct LinkRenameParams: Sendable, Hashable, Codable {
    public var fingerprint: String
    public var name: String?

    public init(fingerprint: String, name: String?) {
        self.fingerprint = fingerprint
        self.name = name
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(fingerprint, forKey: .fingerprint)
        try c.encodeIfPresent(name, forKey: .name)
    }
}

/// `agent.linkDiscovered` 分区元素 / `agent.link.probe` 结果：被发现、尚未配对的设备。
public struct LinkDiscoveredPeer: Sendable, Hashable, Codable {
    /// 对端指纹（经 `/ping` TOFU 获得；mDNS 未探测到时为 nil）。
    public var fingerprint: String?
    public var name: String
    public var platform: String?
    public var host: String
    public var port: UInt16
    public var appVersion: String?
    /// `mdns` | `manual`。
    public var source: String

    public init(
        fingerprint: String? = nil, name: String, platform: String? = nil, host: String, port: UInt16,
        appVersion: String? = nil, source: String = "mdns"
    ) {
        self.fingerprint = fingerprint
        self.name = name
        self.platform = platform
        self.host = host
        self.port = port
        self.appVersion = appVersion
        self.source = source
    }
}

/// `agent.linkPairingRequests` 分区元素：等待本机确认的入站配对请求（对端已输入本机配对码）。
public struct LinkPairingRequestDto: Sendable, Hashable, Identifiable, Codable {
    public var sessionId: String
    public var peerName: String
    public var peerFingerprint: String
    public var peerPlatform: String?
    /// 双方肉眼核对的短认证串（6 位数字）。
    public var sas: String
    public var expiresAtUnixMs: Int64
    /// 对端请求完全控制本机（批准后它可用设备凭据连本机 `/rpc`）。
    public var requestsControl: Bool
    /// 生成 DTO 时刻的剩余毫秒（旧主机缺失）；过期以「收到时刻 + ttlMs」计算。
    public var ttlMs: Int64?

    public init(
        sessionId: String, peerName: String, peerFingerprint: String, peerPlatform: String? = nil, sas: String,
        expiresAtUnixMs: Int64, requestsControl: Bool = false, ttlMs: Int64? = nil
    ) {
        self.sessionId = sessionId
        self.peerName = peerName
        self.peerFingerprint = peerFingerprint
        self.peerPlatform = peerPlatform
        self.sas = sas
        self.expiresAtUnixMs = expiresAtUnixMs
        self.requestsControl = requestsControl
        self.ttlMs = ttlMs
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sessionId = try c.decode(String.self, forKey: .sessionId)
        peerName = try c.decode(String.self, forKey: .peerName)
        peerFingerprint = try c.decode(String.self, forKey: .peerFingerprint)
        peerPlatform = try c.decodeIfPresent(String.self, forKey: .peerPlatform)
        sas = try c.decode(String.self, forKey: .sas)
        expiresAtUnixMs = try c.decode(Int64.self, forKey: .expiresAtUnixMs)
        requestsControl = try c.decodeIfPresent(Bool.self, forKey: .requestsControl) ?? false
        ttlMs = try c.decodeIfPresent(Int64.self, forKey: .ttlMs)
    }

    public var id: String { sessionId }
}

/// `agent.link.pairingCode` 结果：本机当前展示的配对码。
public struct LinkPairingCodeDto: Sendable, Hashable, Codable {
    public var code: String
    public var expiresAtUnixMs: Int64
    /// 本机可被对端直连的地址（`http(s)://host:port`）；空 = 网关仅回环（局域网未开启）。
    public var addresses: [String]
    public var fingerprint: String
    public var deviceName: String
    /// 生成 DTO 时刻的剩余毫秒（旧主机缺失）；倒计时以「收到时刻 + ttlMs」计算，见 ``LinkRules/deadlineMs(ttlMs:expiresAtUnixMs:receivedAtMs:)``。
    public var ttlMs: Int64?

    public init(
        code: String, expiresAtUnixMs: Int64, addresses: [String] = [], fingerprint: String = "", deviceName: String = "",
        ttlMs: Int64? = nil
    ) {
        self.code = code
        self.expiresAtUnixMs = expiresAtUnixMs
        self.addresses = addresses
        self.fingerprint = fingerprint
        self.deviceName = deviceName
        self.ttlMs = ttlMs
    }
}

/// `agent.link.stopPairing` 参数：带 `code` 时只有它仍是当前配对码才作废（多客户端互不干扰）。
public struct LinkStopPairingParams: Sendable, Hashable, Codable {
    public var code: String

    public init(code: String) { self.code = code }
}

public struct LinkDiscoveryParams: Sendable, Hashable, Codable {
    public var enabled: Bool

    public init(enabled: Bool) { self.enabled = enabled }
}

/// `agent.link.probe` 参数：`address` 接受 `host`、`host:port`、`http(s)://host[:port][/base]`；
/// 未写协议按 `http`，未写端口 `http` 用 17800、`https` 用 443。
public struct LinkAddressParams: Sendable, Hashable, Codable {
    public var address: String

    public init(address: String) { self.address = address }
}

public struct LinkPairBeginParams: Sendable, Hashable, Codable {
    public var address: String
    public var code: String
    /// 请求完全控制对端（手机「添加主机」配对）；为 false 时不写入 wire，行为与旧版完全一致。
    public var requestControl: Bool

    private enum CodingKeys: String, CodingKey { case address, code, requestControl }

    public init(address: String, code: String, requestControl: Bool = false) {
        self.address = address
        self.code = code
        self.requestControl = requestControl
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        address = try c.decode(String.self, forKey: .address)
        code = try c.decode(String.self, forKey: .code)
        requestControl = try c.decodeIfPresent(Bool.self, forKey: .requestControl) ?? false
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(address, forKey: .address)
        try c.encode(code, forKey: .code)
        if requestControl { try c.encode(true, forKey: .requestControl) }
    }
}

public struct LinkPairBeginResponse: Sendable, Hashable, Codable {
    public var token: String
    /// 供双方肉眼核对的短认证串。
    public var sas: String
    public var peerName: String
    public var peerFingerprint: String

    public init(token: String, sas: String, peerName: String, peerFingerprint: String) {
        self.token = token
        self.sas = sas
        self.peerName = peerName
        self.peerFingerprint = peerFingerprint
    }
}

public struct LinkPairFinishParams: Sendable, Hashable, Codable {
    public var token: String
    public var accept: Bool

    public init(token: String, accept: Bool) {
        self.token = token
        self.accept = accept
    }
}

/// `accept=false` 或对端拒绝时 `paired=false`，`device` 省略。
public struct LinkPairFinishResponse: Sendable, Hashable, Codable {
    public var paired: Bool
    public var device: LinkDeviceInfo?

    public init(paired: Bool, device: LinkDeviceInfo? = nil) {
        self.paired = paired
        self.device = device
    }
}

public struct LinkApproveParams: Sendable, Hashable, Codable {
    public var sessionId: String
    public var accept: Bool

    public init(sessionId: String, accept: Bool) {
        self.sessionId = sessionId
        self.accept = accept
    }
}

/// `agent.link.remove` 的设备定位参数。
public struct LinkDeviceParams: Sendable, Hashable, Codable {
    public var fingerprint: String

    public init(fingerprint: String) { self.fingerprint = fingerprint }
}

/// `agent.link.controlCredential` 结果：设备专属控制凭据（`fdl1.…`），作为远程主机的访问凭据走 `/rpc` Bearer。
public struct LinkControlCredentialDto: Sendable, Hashable, Codable {
    public var token: String

    public init(token: String) { self.token = token }
}

/// `agent.link.dispatch` 参数：把下载直接下发到已配对的局域网设备。
public struct LinkDispatchParams: Sendable, Hashable, Codable {
    public var fingerprint: String
    public var url: String
    public var fileName: String?
    /// 目标设备上的保存目录；省略 = 目标设备默认下载目录。
    public var saveDir: String?

    public init(fingerprint: String, url: String, fileName: String? = nil, saveDir: String? = nil) {
        self.fingerprint = fingerprint
        self.url = url
        self.fileName = fileName
        self.saveDir = saveDir
    }
}

/// `agent.link.dispatch` 结果：目标设备上新建任务的 ID。
public struct LinkDispatchResult: Sendable, Hashable, Codable {
    public var taskId: String

    public init(taskId: String) { self.taskId = taskId }
}

// MARK: - 纯规则

public enum LinkRules {
    public static let pairingCodeLength = 6

    public static let renameMaxLength = 64

    /// 备注名校验：空白 = 清除（合法）；否则 trim 后 1–64 字符且无控制字符。
    public static func isValidRename(_ input: String) -> Bool {
        let name = input.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty { return true }
        return name.unicodeScalars.count <= renameMaxLength && !name.unicodeScalars.contains { $0.properties.generalCategory == .control }
    }

    /// 备注名提交值：空白 = nil（清除）。
    public static func renameValue(_ input: String) -> String? {
        let name = input.trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? nil : name
    }

    /// 本机局域网直连三态（契约「本机配对码区三态」）。
    public enum LanAccess: Sendable, Hashable {
        /// 未开启「允许局域网访问」：隐藏配对码，提示去设置开启。
        case disabled
        /// 已开启但本次运行未生效（需重启）。
        case restartRequired
        /// 网关实际监听非回环地址：正常显示配对码。
        case active
    }

    /// 由 `agent.gateway` 状态判定三态；分区缺失（远端主机 / server 模式不带网关分区）按 active，由申请配对码的结果如实报错。
    public static func lanAccess(_ gateway: GatewayStatusDto?) -> LanAccess {
        guard let gateway else { return .active }
        if gateway.lanActive { return .active }
        return gateway.lanEnabled ? .restartRequired : .disabled
    }

    /// 发现的对端 → `agent.link.pairBegin` 接受的 `host:port`（IPv6 加方括号）。
    public static func address(of peer: LinkDiscoveredPeer) -> String {
        let host = peer.host.contains(":") && !peer.host.hasPrefix("[") ? "[\(peer.host)]" : peer.host
        return "\(host):\(peer.port)"
    }

    /// 配对码只保留数字并截断到 6 位。
    public static func normalizePairingCode(_ raw: String) -> String {
        String(raw.filter { $0.isASCII && $0.isNumber }.prefix(pairingCodeLength))
    }

    public static func isPairingCodeComplete(_ code: String) -> Bool {
        code.count == pairingCodeLength && code.allSatisfy { $0.isASCII && $0.isNumber }
    }

    /// 手动地址只做非空 / 无空白检查：`host`、`host:port`、`http(s)://…` 的解析与端口校验由 agent 负责。
    public static func isManualAddressValid(_ address: String) -> Bool {
        let text = address.trimmingCharacters(in: .whitespacesAndNewlines)
        return !text.isEmpty && !text.contains(where: \.isWhitespace)
    }

    /// 手动输入无法提交的原因（映射到本地化提示）。
    public enum InputError: Error, Sendable, Hashable {
        case addressMissing, codeIncomplete

        public var key: String {
            switch self {
            case .addressMissing: "localPairingHostRequired"
            case .codeIncomplete: "localPairingCodeIncomplete"
            }
        }
    }

    /// 校验并规整 `pairBegin` 输入：地址原样交给 agent 解析，配对码去掉空白 / 连字符后必须是 6 位数字。
    public static func validate(address: String, code: String, requestControl: Bool = false) -> Result<LinkPairBeginParams, InputError> {
        let address = address.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isManualAddressValid(address) else { return .failure(.addressMissing) }
        let code = String(code.filter { !$0.isWhitespace && $0 != "-" })
        guard isPairingCodeComplete(code) else { return .failure(.codeIncomplete) }
        return .success(LinkPairBeginParams(address: address, code: code, requestControl: requestControl))
    }

    /// 发现列表里的一台设备（已去重、标注是否已配对）。
    public struct DiscoveredEntry: Sendable, Hashable, Identifiable {
        public var name: String
        public var platform: String?
        /// 交给 `pairBegin` / `probe` 的地址。
        public var address: String
        /// 已与本机配对（按指纹判断），不可再选。
        public var paired: Bool

        public var id: String { address }
    }

    /// 发现快照 → 列表：已配对标记、同一地址只保留一条、按名称（不区分大小写）排序。
    public static func discoveredEntries(_ peers: [LinkDiscoveredPeer], paired: [LinkDeviceInfo]) -> [DiscoveredEntry] {
        // 已被对端解除（revoked）的设备不算已配对：允许在发现列表里重新配对。
        let known = Set(paired.filter { !$0.revoked }.map(\.fingerprint))
        var seen = Set<String>()
        var entries: [DiscoveredEntry] = []
        for peer in peers {
            let hostPort = Self.address(of: peer)
            guard seen.insert(hostPort).inserted else { continue }
            let trimmed = peer.name.trimmingCharacters(in: .whitespacesAndNewlines)
            entries.append(DiscoveredEntry(
                name: trimmed.isEmpty ? peer.host : peer.name,
                platform: peer.platform,
                address: hostPort,
                paired: peer.fingerprint.map(known.contains) ?? false
            ))
        }
        return entries.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// 过期时刻（本机 Unix 毫秒）：有 `ttlMs`（生成 DTO 时刻的剩余毫秒）就以「收到 DTO 的本机时刻 + ttlMs」为准——
    /// 免疫主机与本机的时钟偏差；`ttlMs` 缺失（旧主机）才回退主机时钟的 `expiresAtUnixMs`。
    public static func deadlineMs(ttlMs: Int64?, expiresAtUnixMs: Int64, receivedAtMs: Int64) -> Int64 {
        // 主机把「缺失」序列化成 0（旧主机无该字段）：按缺失处理，回退主机时钟。
        guard let ttlMs, ttlMs > 0 else { return expiresAtUnixMs }
        let sum = receivedAtMs.addingReportingOverflow(ttlMs)
        return sum.overflow ? Int64.max : sum.partialValue
    }

    /// 配对错误是否表示对端不支持「请求完全控制」（旧版本 / 协议不匹配）。
    public static func isControlUnsupported(_ error: HostError) -> Bool {
        switch error.reason {
        case "pairingVersionMismatch"?, "peerOutdated"?: true
        default: false
        }
    }

    /// 配对码 / 入站请求距离过期还剩多少秒（向上取整，已过期为 0）。
    public static func secondsUntil(expiresAtUnixMs: Int64, nowMs: Int64) -> Int64 {
        let remaining = expiresAtUnixMs.subtractingReportingOverflow(nowMs)
        let ms = remaining.overflow ? Int64.max : remaining.partialValue
        if ms <= 0 { return 0 }
        return (ms.addingReportingOverflow(999).overflow ? Int64.max : ms + 999) / 1000
    }

    /// 短认证串按 3 位一组展示（`123456` → `123 456`）。
    public static func groupSAS(_ sas: String) -> String {
        let chars = Array(sas.filter { !$0.isWhitespace })
        return stride(from: 0, to: chars.count, by: 3)
            .map { String(chars[$0 ..< min($0 + 3, chars.count)]) }
            .joined(separator: " ")
    }

    /// 配对码按 3-3 分组展示。
    public static func groupCode(_ code: String) -> String { groupSAS(code) }

    /// `LinkDeviceInfo.pairedAt / lastSeenAt` 时间戳：量级 < 1e12 视为秒，否则毫秒；≤ 0 = nil。
    public static func date(fromUnix value: Int64) -> Date? {
        guard value > 0 else { return nil }
        let seconds = value < 1_000_000_000_000 ? Double(value) : Double(value) / 1000
        return Date(timeIntervalSince1970: seconds)
    }

    /// 入站请求排序：先到期的在前（最紧迫先处理）。
    public static func queue(_ requests: [LinkPairingRequestDto]) -> [LinkPairingRequestDto] {
        requests.enumerated().sorted { lhs, rhs in
            if lhs.element.expiresAtUnixMs != rhs.element.expiresAtUnixMs {
                return lhs.element.expiresAtUnixMs < rhs.element.expiresAtUnixMs
            }
            return lhs.offset < rhs.offset
        }.map(\.element)
    }
}
