import Foundation

// UI Gateway（API 服务，`agent.gateway.*`）DTO：镜像 `native/protocol/src/agent.rs` 的
// `GatewayStatusDto` / `GatewayPatchParams`（同 `web/src/lib/rpc/protocol/agent.ts` + `params.ts`）。

/// 网关运行状态；永远不携带 token 文本（用 `agent.gateway.revealToken` 获取）。
public struct GatewayStatusDto: Codable, Sendable, Hashable {
    public var takeoverEnabled: Bool
    public var jsonrpcEnabled: Bool
    public var apiEnabled: Bool
    public var mcpEnabled: Bool
    public var corsEnabled: Bool
    public var userTokenConfigured: Bool
    /// 当前已验证可用的实际监听端口；修改失败时保持原值。
    public var port: Int
    /// server 模式或环境固定监听地址时为 false。
    public var portEditable: Bool
    /// 是否对局域网开放兼容 API；修改后下次 agent 启动生效。
    public var lanEnabled: Bool
    /// 本次运行网关是否实际监听非回环地址（`lanEnabled` 改动需重启才生效）。
    public var lanActive: Bool

    /// 缺省端口（`default_gateway_port`）。
    public static let defaultPort = 17800
    /// `GatewayPatchParams.port` 的合法范围。
    public static let portRange = 1024 ... 65535

    public init(
        takeoverEnabled: Bool = false,
        jsonrpcEnabled: Bool = false,
        apiEnabled: Bool = false,
        mcpEnabled: Bool = false,
        corsEnabled: Bool = false,
        userTokenConfigured: Bool = false,
        port: Int = GatewayStatusDto.defaultPort,
        portEditable: Bool = true,
        lanEnabled: Bool = false,
        lanActive: Bool = false
    ) {
        self.takeoverEnabled = takeoverEnabled
        self.jsonrpcEnabled = jsonrpcEnabled
        self.apiEnabled = apiEnabled
        self.mcpEnabled = mcpEnabled
        self.corsEnabled = corsEnabled
        self.userTokenConfigured = userTokenConfigured
        self.port = port
        self.portEditable = portEditable
        self.lanEnabled = lanEnabled
        self.lanActive = lanActive
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        takeoverEnabled = try c.decode(Bool.self, forKey: .takeoverEnabled)
        jsonrpcEnabled = try c.decode(Bool.self, forKey: .jsonrpcEnabled)
        apiEnabled = try c.decode(Bool.self, forKey: .apiEnabled)
        mcpEnabled = try c.decode(Bool.self, forKey: .mcpEnabled)
        corsEnabled = try c.decode(Bool.self, forKey: .corsEnabled)
        userTokenConfigured = try c.decode(Bool.self, forKey: .userTokenConfigured)
        port = try c.decodeIfPresent(Int.self, forKey: .port) ?? Self.defaultPort
        portEditable = try c.decodeIfPresent(Bool.self, forKey: .portEditable) ?? true
        lanEnabled = try c.decodeIfPresent(Bool.self, forKey: .lanEnabled) ?? false
        lanActive = try c.decodeIfPresent(Bool.self, forKey: .lanActive) ?? false
    }
}

/// `agent.gateway.patch` 参数（原子）。`nil` 字段省略 = 保持不变。
///
/// `apiEnabled` / `mcpEnabled` 由关转开且 token 仍为空时主机自动生成 token；显式清空 token（且本次未
/// 开启上述开关）时主机同时关闭二者。
public struct GatewayPatchParams: Codable, Sendable, Hashable {
    public var takeoverEnabled: Bool?
    public var jsonrpcEnabled: Bool?
    public var apiEnabled: Bool?
    public var mcpEnabled: Bool?
    public var corsEnabled: Bool?
    public var lanEnabled: Bool?
    /// 1024…65535；验证新服务可用后立即切换，固定监听模式拒绝修改。
    public var port: Int?
    /// 空串 = 清除用户 token；省略 = 保持。
    public var userToken: String?
    /// true 生成新的随机 token（优先于 `userToken`）。
    public var regenerateUserToken: Bool?

    public init(
        takeoverEnabled: Bool? = nil,
        jsonrpcEnabled: Bool? = nil,
        apiEnabled: Bool? = nil,
        mcpEnabled: Bool? = nil,
        corsEnabled: Bool? = nil,
        lanEnabled: Bool? = nil,
        port: Int? = nil,
        userToken: String? = nil,
        regenerateUserToken: Bool? = nil
    ) {
        self.takeoverEnabled = takeoverEnabled
        self.jsonrpcEnabled = jsonrpcEnabled
        self.apiEnabled = apiEnabled
        self.mcpEnabled = mcpEnabled
        self.corsEnabled = corsEnabled
        self.lanEnabled = lanEnabled
        self.port = port
        self.userToken = userToken
        self.regenerateUserToken = regenerateUserToken
    }
}

/// `agent.gateway.revealToken` 结果（未配置为空串）。
public struct GatewayRevealTokenResult: Codable, Sendable, Hashable {
    public var userToken: String

    public init(userToken: String = "") {
        self.userToken = userToken
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        userToken = try c.decodeIfPresent(String.self, forKey: .userToken) ?? ""
    }
}

/// 网关功能开关（API 服务页「功能开关」分组；顺序同 GPUI / Web）。
public enum GatewayFeature: CaseIterable, Sendable, Hashable {
    case takeover, jsonrpc, api, mcp, cors

    public var titleKey: String {
        switch self {
        case .takeover: "apiServiceTakeover"
        case .jsonrpc: "apiServiceJsonrpc"
        case .api: "apiServiceApi"
        case .mcp: "apiServiceMcp"
        case .cors: "apiServiceCorsAllowAll"
        }
    }

    public var detailKey: String {
        switch self {
        case .takeover: "apiServiceTakeoverDesc"
        case .jsonrpc: "apiServiceJsonrpcDesc"
        case .api: "apiServiceApiDesc"
        case .mcp: "apiServiceMcpDesc"
        case .cors: "apiServiceCorsAllowAllDesc"
        }
    }

    public func isOn(_ status: GatewayStatusDto) -> Bool {
        switch self {
        case .takeover: status.takeoverEnabled
        case .jsonrpc: status.jsonrpcEnabled
        case .api: status.apiEnabled
        case .mcp: status.mcpEnabled
        case .cors: status.corsEnabled
        }
    }

    /// 只打开本开关的补丁。
    public func patch(_ on: Bool) -> GatewayPatchParams {
        switch self {
        case .takeover: GatewayPatchParams(takeoverEnabled: on)
        case .jsonrpc: GatewayPatchParams(jsonrpcEnabled: on)
        case .api: GatewayPatchParams(apiEnabled: on)
        case .mcp: GatewayPatchParams(mcpEnabled: on)
        case .cors: GatewayPatchParams(corsEnabled: on)
        }
    }
}
