import FluxDomain
import Foundation
import Network
import Observation

/// iOS 局域网发现：`NWBrowser` 浏览 `_fluxdown._tcp`（Info.plist 已声明 `NSBonjourServices`；首次使用由系统弹本地网络授权）。
///
/// iOS 没有 multicast entitlement，Rust 的 mDNS 在这里不可用，所以发现不经 agent：TXT 记录里的指纹 / 名称 / 平台（键同
/// `native/link/src/discovery.rs`）由 `LanDiscovery` 清洗成列表；用户选中一台时才用短暂的 `NWConnection` 把 Bonjour 服务
/// 解析成 IPv4 `host:port`（系统挑出真正可达的那个地址，而不是启发式挑第一个）。
/// 面板关闭必须 `stop()`：取消浏览，不留后台 Bonjour 查询。
@MainActor
@Observable
final class LanBrowser {
    /// 去重前的广告项（浏览结果的当前全量快照）。
    private(set) var adverts: [LanDiscovery.Advert] = []
    /// 开始 / 重新搜索后的 8 秒窗口内（列表为空时显示「正在搜索…」）。
    private(set) var searching = false
    /// 浏览器启动失败 / 本地网络权限被拒的原因（本地化文案）；nil = 正常。
    private(set) var failure: String?
    /// 正在解析的条目 id（行内转圈 + 禁用重复点击）。
    private(set) var resolvingId: String?
    /// 已解析的 `ip:port`（条目 id → 地址）；用于标记当前选中的行。
    private(set) var resolved: [String: String] = [:]

    @ObservationIgnored private var browser: NWBrowser?
    /// 服务实例名 → 浏览到的端点（同一主机多网卡有多个）。
    @ObservationIgnored private var endpoints: [String: [NWEndpoint]] = [:]
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var resolver: ServiceResolver?

    private static let searchWindow: Duration = .seconds(8)
    private static let resolveTimeout: Duration = .seconds(6)

    var isRunning: Bool { browser != nil }

    /// 开始浏览（幂等）。
    func start() {
        guard browser == nil else { return }
        failure = nil
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: LanDiscovery.serviceType, domain: nil), using: .tcp)
        browser.stateUpdateHandler = { [weak self] state in
            MainActor.assumeIsolated { self?.handle(state: state) }
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            MainActor.assumeIsolated { self?.apply(results) }
        }
        self.browser = browser
        browser.start(queue: .main)
        beginSearching()
    }

    /// 取消浏览并清空（面板关闭 / 切到别的模式）。
    func stop() {
        browser?.cancel()
        browser = nil
        searchTask?.cancel()
        searchTask = nil
        resolver?.cancel()
        resolver = nil
        searching = false
        resolvingId = nil
        adverts = []
        endpoints = [:]
        resolved = [:]
    }

    /// 重新搜索。
    func restart() {
        stop()
        start()
    }

    private func beginSearching() {
        searchTask?.cancel()
        searching = true
        searchTask = Task { [weak self] in
            do {
                try await Task.sleep(for: Self.searchWindow)
            } catch {
                return
            }
            self?.searching = false
        }
    }

    private func handle(state: NWBrowser.State) {
        switch state {
        case let .failed(error):
            failure = Self.describe(error)
            browser?.cancel()
            browser = nil
            searching = false
        case let .waiting(error):
            // 多为本地网络权限被拒 / 未连接 Wi‑Fi：给出可见提示，恢复后系统会自动继续。
            failure = Self.describe(error)
        case .ready:
            failure = nil
        default:
            break
        }
    }

    private static func describe(_ error: NWError) -> String {
        L("localServiceActionFailed")
    }

    private func apply(_ results: Set<NWBrowser.Result>) {
        var list: [LanDiscovery.Advert] = []
        var map: [String: [NWEndpoint]] = [:]
        for result in results {
            guard case let .service(name, _, _, _) = result.endpoint else { continue }
            var txt: [String: String] = [:]
            if case let .bonjour(record) = result.metadata { txt = record.dictionary }
            list.append(LanDiscovery.advert(serviceName: name, txt: txt))
            map[name, default: []].append(result.endpoint)
        }
        // 浏览结果是无序集合：排序后再交给去重，保证同一指纹每次留下同一条广告。
        adverts = list.sorted { $0.serviceName < $1.serviceName }
        endpoints = map
    }

    /// 把选中的条目解析成 IPv4 `host:port`；失败（主机已离线 / 只有 IPv6 / 超时）返回 nil。
    func resolve(_ entry: LanDiscovery.Entry) async -> String? {
        guard resolvingId == nil, let targets = endpoints[entry.serviceName], !targets.isEmpty else { return nil }
        resolvingId = entry.id
        defer { resolvingId = nil }
        for endpoint in targets {
            let resolver = ServiceResolver(endpoint: endpoint)
            self.resolver = resolver
            let address = await resolver.resolve(timeout: Self.resolveTimeout)
            self.resolver = nil
            if let address {
                resolved[entry.id] = address
                return address
            }
        }
        return nil
    }
}

/// 用一次性 `NWConnection`（仅 IPv4）连到 Bonjour 服务端点，连上后读出系统实际选中的远端 `ip:port`，随即取消。
/// 对端只会看到一次立即关闭的 TCP 连接。
@MainActor
private final class ServiceResolver {
    private let connection: NWConnection
    private var continuation: CheckedContinuation<String?, Never>?

    init(endpoint: NWEndpoint) {
        let parameters = NWParameters.tcp
        if let ip = parameters.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options {
            ip.version = .v4
        }
        connection = NWConnection(to: endpoint, using: parameters)
    }

    func resolve(timeout: Duration) async -> String? {
        await withCheckedContinuation { continuation in
            self.continuation = continuation
            connection.stateUpdateHandler = { [weak self] state in
                MainActor.assumeIsolated { self?.handle(state) }
            }
            connection.start(queue: .main)
            Task { [weak self] in
                do {
                    try await Task.sleep(for: timeout)
                } catch {
                    return
                }
                self?.finish(nil)
            }
        }
    }

    func cancel() { finish(nil) }

    private func handle(_ state: NWConnection.State) {
        switch state {
        case .ready:
            guard case let .hostPort(host, port)? = connection.currentPath?.remoteEndpoint,
                  case let .ipv4(address) = host
            else {
                finish(nil)
                return
            }
            finish(LanDiscovery.ipv4Literal([UInt8](address.rawValue)).map {
                LanDiscovery.hostPort(host: $0, port: port.rawValue)
            })
        case .failed, .cancelled:
            finish(nil)
        default:
            break
        }
    }

    private func finish(_ address: String?) {
        guard let continuation else { return }
        self.continuation = nil
        connection.stateUpdateHandler = nil
        connection.cancel()
        continuation.resume(returning: address)
    }
}
