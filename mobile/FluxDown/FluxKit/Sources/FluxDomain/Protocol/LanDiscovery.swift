import Foundation

/// iOS 局域网发现的纯逻辑：App 层用 `NWBrowser`（Bonjour，`_fluxdown._tcp`）浏览，这里负责把 TXT 记录清洗成广告项、
/// 去重排序成列表、拼 `host:port`。iOS 没有 multicast entitlement，Rust 的 mDNS 不可用，所以发现不经 agent。
///
/// TXT 键与 `native/link/src/discovery.rs` 的广播端一致：`fp`（64 位小写 hex 指纹）、`name`、`plat`、`ver`。
/// 广播内容来自局域网任意主机，一律不信任：指纹必须是 64 位小写 hex，名称 / 平台 / 版本剔除控制与双向控制字符并限长
/// （同 `native/link/src/text.rs::sanitize_text`）。
public enum LanDiscovery {
    public static let serviceType = "_fluxdown._tcp"
    public static let txtFingerprint = "fp"
    public static let txtName = "name"
    public static let txtPlatform = "plat"
    public static let txtVersion = "ver"

    public static let maxNameChars = 64
    public static let maxPlatformChars = 32
    public static let maxVersionChars = 64

    /// 浏览到的一条服务广告（已清洗）。
    public struct Advert: Sendable, Hashable {
        /// Bonjour 服务实例名（同一主机多网卡会产生多条同名广告）。
        public var serviceName: String
        public var fingerprint: String?
        public var name: String?
        public var platform: String?
        public var appVersion: String?

        public init(serviceName: String, fingerprint: String? = nil, name: String? = nil, platform: String? = nil, appVersion: String? = nil) {
            self.serviceName = serviceName
            self.fingerprint = fingerprint
            self.name = name
            self.platform = platform
            self.appVersion = appVersion
        }
    }

    /// 发现列表里的一台主机（已去重、标注配对状态）。
    public struct Entry: Sendable, Hashable, Identifiable {
        /// 指纹（缺失时退化为服务实例名）。
        public var id: String
        public var serviceName: String
        public var fingerprint: String?
        public var name: String
        public var platform: String?
        public var appVersion: String?
        /// 已与本机配对（排除已被对端解除的记录）。
        public var paired: Bool
        /// 已配对且本机可控制它（可直接取控制凭据连接，无需再输配对码）。
        public var controllable: Bool
    }

    /// TXT 记录 → 广告项。
    public static func advert(serviceName: String, txt: [String: String]) -> Advert {
        let fingerprint = txt[txtFingerprint].flatMap { isFingerprint($0) ? $0 : nil }
        return Advert(
            serviceName: serviceName,
            fingerprint: fingerprint,
            name: sanitize(txt[txtName], maxChars: maxNameChars),
            platform: sanitize(txt[txtPlatform], maxChars: maxPlatformChars),
            appVersion: sanitize(txt[txtVersion], maxChars: maxVersionChars)
        )
    }

    /// 64 位小写 hex。
    public static func isFingerprint(_ text: String) -> Bool {
        text.utf8.count == 64 && text.utf8.allSatisfy { ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66) }
    }

    /// 去掉控制 / 双向控制字符，去首尾空白，按字符数截断；结果为空 = nil。
    public static func sanitize(_ raw: String?, maxChars: Int) -> String? {
        guard let raw else { return nil }
        let cleaned = String(String.UnicodeScalarView(raw.unicodeScalars.filter { !isUnsafeDisplay($0) }))
        let truncated = String(cleaned.trimmingCharacters(in: .whitespacesAndNewlines).prefix(maxChars))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return truncated.isEmpty ? nil : truncated
    }

    private static func isUnsafeDisplay(_ scalar: Unicode.Scalar) -> Bool {
        if scalar.properties.generalCategory == .control { return true }
        switch scalar.value {
        case 0x061C, 0x200E, 0x200F, 0x202A ... 0x202E, 0x2066 ... 0x2069: return true
        default: return false
        }
    }

    /// 广告 → 列表：同一指纹（无指纹则同一服务名）只保留一条（多网卡 / 重复广告），已配对标记，按名称（不区分大小写）排序。
    public static func entries(_ adverts: [Advert], paired: [LinkDeviceInfo]) -> [Entry] {
        var known: [String: LinkDeviceInfo] = [:]
        for device in paired where !device.revoked { known[device.fingerprint] = device }
        var seen = Set<String>()
        var result: [Entry] = []
        for advert in adverts {
            let id = advert.fingerprint ?? advert.serviceName
            guard seen.insert(id).inserted else { continue }
            let record = advert.fingerprint.flatMap { known[$0] }
            let fallback = advert.fingerprint.map { String($0.prefix(12)) } ?? advert.serviceName
            result.append(Entry(
                id: id,
                serviceName: advert.serviceName,
                fingerprint: advert.fingerprint,
                name: advert.name ?? fallback,
                platform: advert.platform,
                appVersion: advert.appVersion,
                paired: record != nil,
                controllable: record?.controllable ?? false
            ))
        }
        return result.sorted {
            let order = $0.name.localizedCaseInsensitiveCompare($1.name)
            return order == .orderedSame ? $0.id < $1.id : order == .orderedAscending
        }
    }

    /// IPv4 地址字节（4 字节）→ 点分十进制；长度不对 = nil。
    public static func ipv4Literal(_ bytes: [UInt8]) -> String? {
        bytes.count == 4 ? bytes.map(String.init).joined(separator: ".") : nil
    }

    /// `host:port`：去掉 IPv6 zone（`%en0`），IPv6 加方括号。
    public static func hostPort(host: String, port: UInt16) -> String {
        var bare = host
        if let zone = bare.firstIndex(of: "%") { bare = String(bare[..<zone]) }
        if bare.hasPrefix("["), bare.hasSuffix("]") { bare = String(bare.dropFirst().dropLast()) }
        return "\(bare.contains(":") ? "[\(bare)]" : bare):\(port)"
    }
}
