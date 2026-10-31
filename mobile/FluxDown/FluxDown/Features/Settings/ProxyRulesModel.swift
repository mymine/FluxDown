import FluxDomain
import FluxUI
import Foundation
import Observation

/// 站点规则的异步状态：规则写入（读-改-写 + 冲突重放）、编辑器检测、「测试网址」。
/// 模型不持有会话：每次调用由视图传入当前 `HostSession`；过期结果用代计数器丢弃。
@MainActor
@Observable
final class ProxyRulesModel {
    nonisolated enum Explain: Equatable {
        case idle
        case running
        /// `stamp = proxy_mode + "|" + 规则原文`：发送前记下，渲染时只在与当前值相同时显示。
        case done(stamp: String, response: ProxyRouteResponse)
        case failed(stamp: String, text: String)

        var stamp: String? {
            switch self {
            case let .done(stamp, _), let .failed(stamp, _): stamp
            case .idle, .running: nil
            }
        }
    }

    private(set) var explain: Explain = .idle
    private var generation = 0

    @ObservationIgnored private let container: AppContainer
    @ObservationIgnored private let writer: ProxyRuleWriter

    init(container: AppContainer) {
        self.container = container
        writer = ProxyRuleWriter(port: ContainerDaemonConfigPort(container))
    }

    // MARK: 写入

    func save(_ draft: ProxyRule) async -> Bool {
        await write { (writer) throws(HostError) in try await writer.upsert(draft) }
    }

    func setEnabled(_ rule: ProxyRule, _ enabled: Bool) async {
        _ = await write { (writer) throws(HostError) in try await writer.setEnabled(id: rule.id, enabled: enabled) }
    }

    @discardableResult
    func remove(_ rule: ProxyRule) async -> Bool {
        await write { (writer) throws(HostError) in try await writer.remove(id: rule.id) }
    }

    func move(_ rule: ProxyRule, by delta: Int) async {
        _ = await write { (writer) throws(HostError) in try await writer.move(id: rule.id, by: delta) }
    }

    func move(_ rule: ProxyRule, toOffset destination: Int) async {
        _ = await write { (writer) throws(HostError) in try await writer.move(id: rule.id, toOffset: destination) }
    }

    private func write(_ operation: (ProxyRuleWriter) async throws(HostError) -> Void) async -> Bool {
        do throws(HostError) {
            try await operation(writer)
            return true
        } catch {
            container.toasts.show(text: WebhookErrorText.write(error), tone: .error)
            return false
        }
    }

    // MARK: 检测

    /// 编辑器检测：发送 `rules = encode(rules)`；调用方传 `[draft]`（失焦 / 试一试，只看下标 0）
    /// 或 `upserting(draft, in: 当前列表)`（保存前，看 draft 所在下标与 field 为空的 issue）。
    func check(
        rules: [ProxyRule], url: String, using session: any HostSession
    ) async throws(HostError) -> ProxyRouteResponse {
        let request = ProxyRouteRequest(url: url, rules: try ProxyRule.encodeList(rules))
        return try await session.call(HostMethod.daemonConfigProxyRoute, params: request)
    }

    /// 「测试网址」：`rulesRaw` 是界面渲染所用的 `proxy_rules` 原文，原样透传。
    func runExplain(url: String, rulesRaw: String, stamp: String, using session: any HostSession) async {
        if explain == .running { return }
        generation += 1
        let current = generation
        explain = .running
        let next: Explain
        do throws(HostError) {
            let response: ProxyRouteResponse = try await session.call(
                HostMethod.daemonConfigProxyRoute, params: ProxyRouteRequest(url: url, rules: rulesRaw)
            )
            next = .done(stamp: stamp, response: response)
        } catch {
            next = .failed(stamp: stamp, text: ErrorText.describe(error))
        }
        guard current == generation else { return }
        explain = next
    }

    /// 主机切换后旧结果作废。
    func resetExplain() {
        guard explain != .idle else { return }
        generation += 1
        explain = .idle
    }
}
