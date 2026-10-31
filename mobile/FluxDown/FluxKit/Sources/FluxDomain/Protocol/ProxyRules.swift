import Foundation

// 站点代理规则（配置键 `proxy_rules` + `daemon.config.proxyRoute`）：DTO 与解析镜像 `native/protocol/src/proxy_rules.rs`、
// `native/protocol/src/daemon.rs` 的 ProxyRoute 段与 `web/src/lib/proxyRules.ts`。规则匹配只在引擎里实现，客户端只调用 RPC。
//
// 编码总是写出全部 7 个字段；对象内字段顺序不作要求（`ProtocolJSON` 固定 `.sortedKeys`，各端解析器都宽松）。

// MARK: - 规则

/// 一条站点规则；数组顺序就是优先级。
public struct ProxyRule: Codable, Sendable, Hashable, Identifiable {
    public static let configKey = "proxy_rules"

    public static let matchDomain = "domain"
    public static let matchRegex = "regex"
    public static let actionDirect = "direct"
    public static let actionSystem = "system"
    public static let actionManual = "manual"
    public static let actionCustom = "custom"
    public static let actions = [actionDirect, actionSystem, actionManual, actionCustom]

    public var id: String
    public var enabled: Bool
    public var name: String
    /// wire 字段名 `match`：`domain` / `regex`。
    public var match: String
    public var pattern: String
    /// `direct` / `system` / `manual` / `custom`。
    public var action: String
    /// 仅 `custom` 使用。
    public var proxy: String

    public init(
        id: String,
        enabled: Bool = true,
        name: String = "",
        match: String = ProxyRule.matchDomain,
        pattern: String = "",
        action: String = ProxyRule.actionDirect,
        proxy: String = ""
    ) {
        self.id = id
        self.enabled = enabled
        self.name = name
        self.match = match
        self.pattern = pattern
        self.action = action
        self.proxy = proxy
    }

    /// 宽松解析一个元素：非对象为 nil；缺失 / 类型错误 / 未知取值回退默认值（`enabled` 缺省 true，`match` 缺省 domain，`action` 缺省 direct）。
    public init?(lenient value: JSONValue) {
        guard case let .object(item) = value else { return nil }
        func text(_ key: String) -> String { item[key]?.stringValue ?? "" }
        let match = item["match"]?.stringValue
        let action = item["action"]?.stringValue
        self.init(
            id: text("id"),
            enabled: item["enabled"]?.boolValue ?? true,
            name: text("name"),
            match: match == ProxyRule.matchRegex ? ProxyRule.matchRegex : ProxyRule.matchDomain,
            pattern: text("pattern"),
            action: action.flatMap { ProxyRule.actions.contains($0) ? $0 : nil } ?? ProxyRule.actionDirect,
            proxy: text("proxy")
        )
    }

    private enum CodingKeys: String, CodingKey { case id, enabled, name, match, pattern, action, proxy }

    public init(from decoder: any Decoder) throws {
        let value = try JSONValue(from: decoder)
        guard let parsed = ProxyRule(lenient: value) else {
            throw DecodingError.typeMismatch(
                ProxyRule.self,
                DecodingError.Context(codingPath: decoder.codingPath, debugDescription: "proxy rule must be an object")
            )
        }
        self = parsed
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(enabled, forKey: .enabled)
        try container.encode(name, forKey: .name)
        try container.encode(match, forKey: .match)
        try container.encode(pattern, forKey: .pattern)
        try container.encode(action, forKey: .action)
        try container.encode(proxy, forKey: .proxy)
    }

    /// 配置串 → 规则列表：空串 / 非法 JSON / 非数组为空列表；非对象元素跳过。
    public static func parseList(_ raw: String?) -> [ProxyRule] {
        guard let raw, !raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let value = try? ProtocolJSON.makeDecoder().decode(JSONValue.self, from: Data(raw.utf8)),
              case let .array(items) = value
        else { return [] }
        return items.compactMap { ProxyRule(lenient: $0) }
    }

    /// 规则列表 → 配置串（JSON 数组）。
    public static func encodeList(_ list: [ProxyRule]) throws(HostError) -> String {
        let data = try ProtocolJSON.encode(list, what: ProxyRule.configKey)
        return String(decoding: data, as: UTF8.self)
    }

    /// 启用的规则条数（状态提示用）。
    public static func enabledCount(_ raw: String?) -> Int {
        parseList(raw).filter(\.enabled).count
    }

    /// 展示用代理端点：去掉凭据与末尾 `/`，如 `socks5://u:p@h:1/` → `socks5://h:1`。
    public static func displayEndpoint(_ url: String) -> String {
        let trimmed = url.trimmingCharacters(in: .whitespacesAndNewlines)
        var scheme: String?
        var rest = Substring(trimmed)
        if let range = trimmed.range(of: "://") {
            scheme = String(trimmed[..<range.lowerBound])
            rest = trimmed[range.upperBound...]
        }
        if let at = rest.lastIndex(of: "@") { rest = rest[rest.index(after: at)...] }
        while rest.hasSuffix("/") { rest = rest.dropLast() }
        if let scheme { return "\(scheme)://\(rest)" }
        return String(rest)
    }

    /// 按 id 覆盖或追加（保存前检测与写入共用，保证两者的列表一致）。
    public static func upserting(_ draft: ProxyRule, in list: [ProxyRule]) -> [ProxyRule] {
        var next = list
        if let index = next.firstIndex(where: { $0.id == draft.id }) {
            next[index] = draft
        } else {
            next.append(draft)
        }
        return next
    }

    /// 按 `delta` 移动；id 缺失、`delta == 0` 或越界返回 nil（不写入）。
    public static func moving(id: String, by delta: Int, in list: [ProxyRule]) -> [ProxyRule]? {
        guard let from = list.firstIndex(where: { $0.id == id }) else { return nil }
        let to = from + delta
        guard delta != 0, list.indices.contains(to) else { return nil }
        var next = list
        next.insert(next.remove(at: from), at: to)
        return next
    }
}

// MARK: - 读-改-写

/// 规则写入（读-改-写 + 冲突重放，委托给 `DaemonConfigListWriter`）。
@MainActor
public final class ProxyRuleWriter {
    private let writer: DaemonConfigListWriter<ProxyRule>

    public init(port: any DaemonConfigPort) {
        writer = DaemonConfigListWriter(
            port: port,
            key: ProxyRule.configKey,
            parse: { ProxyRule.parseList($0) },
            encode: { list throws(HostError) in try ProxyRule.encodeList(list) }
        )
    }

    /// 新增或按 id 覆盖。
    public func upsert(_ draft: ProxyRule) async throws(HostError) {
        try await writer.mutate { list in ProxyRule.upserting(draft, in: list) }
    }

    public func setEnabled(id: String, enabled: Bool) async throws(HostError) {
        try await writer.mutate { list in
            guard list.contains(where: { $0.id == id }) else { return nil }
            return list.map { entry in
                var copy = entry
                if copy.id == id { copy.enabled = enabled }
                return copy
            }
        }
    }

    public func remove(id: String) async throws(HostError) {
        try await writer.mutate { list in list.filter { $0.id != id } }
    }

    /// 上移 / 下移（`delta` = -1 / +1）；越界或 id 缺失不写入。
    public func move(id: String, by delta: Int) async throws(HostError) {
        try await writer.mutate { list in ProxyRule.moving(id: id, by: delta, in: list) }
    }

    /// 把 `id` 移到 `destination`（`ForEach.onMove` 的语义：插入到移除前的下标 `destination` 之前）。
    public func move(id: String, toOffset destination: Int) async throws(HostError) {
        try await writer.mutate { list in
            guard let from = list.firstIndex(where: { $0.id == id }) else { return nil }
            var next = list
            let item = next.remove(at: from)
            let target = destination > from ? destination - 1 : destination
            next.insert(item, at: min(max(target, 0), next.count))
            return next == list ? nil : next
        }
    }
}

// MARK: - DTO

/// `daemon.config.proxyRoute` 参数：`rules` 为 nil 时检查已保存的规则；`url` 为空只做校验。
public struct ProxyRouteRequest: Sendable, Hashable, Codable {
    public var url: String
    public var rules: String?

    public init(url: String, rules: String? = nil) {
        self.url = url
        self.rules = rules
    }
}

/// 严格校验的一条问题；`index` 从 0 起（= 数组下标）。
public struct ProxyRuleIssue: Sendable, Hashable, Codable {
    public var index: Int
    public var field: String
    public var code: String
    public var detail: String

    public init(index: Int, field: String, code: String, detail: String = "") {
        self.index = index
        self.field = field
        self.code = code
        self.detail = detail
    }

    private enum CodingKeys: String, CodingKey { case index, field, code, detail }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        index = try c.decodeIfPresent(Int.self, forKey: .index) ?? 0
        field = try c.decodeIfPresent(String.self, forKey: .field) ?? ""
        code = try c.decodeIfPresent(String.self, forKey: .code) ?? ""
        detail = try c.decodeIfPresent(String.self, forKey: .detail) ?? ""
    }
}

public struct ProxyRouteMatch: Sendable, Hashable, Codable {
    public var index: Int
    public var id: String
    public var name: String

    public init(index: Int, id: String, name: String = "") {
        self.index = index
        self.id = id
        self.name = name
    }

    private enum CodingKeys: String, CodingKey { case index, id, name }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        index = try c.decodeIfPresent(Int.self, forKey: .index) ?? 0
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
    }
}

/// `daemon.config.proxyRoute` 结果；`reason == ""` 表示没有给出 url。
public struct ProxyRouteResponse: Sendable, Hashable, Codable {
    public var issues: [ProxyRuleIssue]
    public var matched: ProxyRouteMatch?
    public var reason: String
    public var endpoint: String

    public init(issues: [ProxyRuleIssue] = [], matched: ProxyRouteMatch? = nil, reason: String = "", endpoint: String = "") {
        self.issues = issues
        self.matched = matched
        self.reason = reason
        self.endpoint = endpoint
    }

    private enum CodingKeys: String, CodingKey { case issues, matched, reason, endpoint }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        issues = try c.decodeIfPresent([ProxyRuleIssue].self, forKey: .issues) ?? []
        matched = try c.decodeIfPresent(ProxyRouteMatch.self, forKey: .matched)
        reason = try c.decodeIfPresent(String.self, forKey: .reason) ?? ""
        endpoint = try c.decodeIfPresent(String.self, forKey: .endpoint) ?? ""
    }
}
