import FluxDomain
import Foundation

/// 缓存解码的通用分区读取器：只在分区字节变化时才重新解码（`HostState.section` 每次调用都会解码，
/// 而视图 body 会随任务进度每 100ms 重算一次）。分区缺失 / 解码失败 / JSON `null` 都读作 nil。
@MainActor
final class SectionReader<Value: Decodable> {
    private let key: String
    private var lastData: Data?
    private var cached: Value?

    init(_ key: String) { self.key = key }

    func read(_ state: HostState) -> Value? {
        let data = state.sections[key]
        if data == lastData { return cached }
        lastData = data
        cached = data.flatMap { try? ProtocolJSON.makeDecoder().decode(Value.self, from: $0) }
        return cached
    }
}

/// 账户 / 设备 / 同步 / 局域网相关分区的缓存读取器集合；视图以 `@State private var sections = AgentSections()` 持有。
@MainActor
final class AgentSections {
    private let session = SectionReader<AgentSessionDto>(HostSection.agentSession)
    private let sync = SectionReader<SyncStatusDto>(HostSection.agentSync)
    private let connection = SectionReader<CloudConnectionDto>(HostSection.agentCloudConnection)
    private let remoteTasks = SectionReader<[RemoteTaskDto]>(HostSection.agentRemoteTasks)
    private let pairingRequests = SectionReader<[LinkPairingRequestDto]>(HostSection.agentLinkPairingRequests)
    private let discovered = SectionReader<[LinkDiscoveredPeer]>(HostSection.agentLinkDiscovered)
    private let gateway = SectionReader<GatewayStatusDto>(HostSection.agentGateway)

    /// 当前云账户会话；未登录 / 分区缺失为 nil。
    func session(_ state: HostState) -> AgentSessionDto? { session.read(state) }

    func sync(_ state: HostState) -> SyncStatusDto { sync.read(state) ?? SyncStatusDto() }

    func connection(_ state: HostState) -> CloudConnectionDto? { connection.read(state) }

    func remoteTasks(_ state: HostState) -> [RemoteTaskDto] { remoteTasks.read(state) ?? [] }

    /// 等待本机确认的入站配对请求，按到期先后排序。
    func pairingRequests(_ state: HostState) -> [LinkPairingRequestDto] {
        LinkRules.queue(pairingRequests.read(state) ?? [])
    }

    func discovered(_ state: HostState) -> [LinkDiscoveredPeer] { discovered.read(state) ?? [] }

    /// 本机局域网直连三态（网关分区缺失按 active）。
    func lanAccess(_ state: HostState) -> LinkRules.LanAccess { LinkRules.lanAccess(gateway.read(state)) }
}
