import FluxDomain
import Foundation
import Observation
import OSLog

/// 「添加主机 → 局域网配对」的流程控制器：持有**本机主机的独立会话**（与当前连着哪台主机无关，面板关闭即关闭）、
/// 复用 `PairingModel` 的配对状态机（请求完全控制），并在配对完成后收尾——向本机 agent 取设备控制凭据、
/// 保存为远程主机并切换过去（`AppContainer.addRemoteHost(linkFingerprint:)`）。
///
/// 发现走 `PairingModel.browser`（NWBrowser，iOS 上 Rust mDNS 不可用）。已配对且 `controllable` 的主机可直接连接
/// （取凭据 → 保存 / 更新 → 切换，不必再输配对码）。
@MainActor
@Observable
final class HostPairingFlow {
    enum Phase: Equatable {
        case opening
        case ready
        case failed(String)
    }

    private(set) var phase: Phase = .opening
    private(set) var model: PairingModel?
    /// 本机会话上的已配对设备（含 `controllable`）；`agent.link.refresh` 回来之前为空。
    private(set) var paired: [LinkDeviceInfo] = []
    /// 正在直接连接的条目 id（行内转圈 + 禁用重复点击）。
    private(set) var connectingId: String?

    @ObservationIgnored private unowned let container: AppContainer
    @ObservationIgnored private var session: (any HostSession)?
    @ObservationIgnored private var drain: Task<Void, Never>?
    @ObservationIgnored private var closed = false
    @ObservationIgnored private var opening = false
    private static let log = Logger(subsystem: "com.fluxdown.app", category: "pairing")

    init(container: AppContainer) {
        self.container = container
    }

    private var api: AgentAPI? { session.map { AgentAPI(session: $0) } }

    // MARK: 生命周期

    /// 打开本机会话并建好状态机；`prefill` = 「重新配对」入口给的主机地址。
    func open(prefill: String?) async {
        guard !opening, model == nil, !closed else { return }
        opening = true
        phase = .opening
        defer { opening = false }
        switch await container.openLocalSession() {
        case let .success(opened):
            if closed {
                opened.close()
                return
            }
            session = opened
            // 本会话只用来发 RPC：把信号流读干净（否则未消费的快照 / 事件在会话存活期间无限堆积）。
            let signals = opened.signals
            drain = Task { for await _ in signals {} }
            let model = PairingModel(
                container: container,
                api: { [weak self] in
                    // 会话在关闭后不会再被调用；占位仅为满足类型。
                    AgentAPI(session: self?.session ?? UnavailableSession(HostError(.cancelled)))
                },
                options: .init(requestControl: true, agentDiscovery: false),
                completion: { [weak self] verify, response in
                    guard let self else { return .failed(message: L("localServiceActionFailed")) }
                    return await complete(verify, response)
                }
            )
            if let prefill { model.address = prefill }
            self.model = model
            phase = .ready
            refreshPaired()
        case let .failure(error):
            phase = .failed(ErrorText.describe(error))
        }
    }

    /// 重试打开（失败页的按钮）。
    func retry(prefill: String?) async {
        guard case .failed = phase else { return }
        await open(prefill: prefill)
    }

    /// 面板关闭：放弃未完成的配对、停浏览，等收尾调用跑完再关会话。
    func close() async {
        closed = true
        if let model { await model.shutdown() }
        drain?.cancel()
        drain = nil
        session?.close()
        session = nil
    }

    private func refreshPaired() {
        guard let api else { return }
        Task { [weak self] in
            do throws(HostError) {
                let list = try await api.linkRefresh()
                guard let self, !self.closed else { return }
                self.paired = list
            } catch {
                Self.log.info("link refresh failed: \(error.description, privacy: .public)")
            }
        }
    }

    // MARK: 配对完成收尾

    /// 配对成立：取控制凭据 → 保存并切换。失败返回可见文案（配对记录已建立，用户可重试配对覆盖它）。
    private func complete(_ verify: PairingModel.Verify, _ response: LinkPairFinishResponse) async -> PairingModel.Completion {
        guard let model, let api else { return .failed(message: L("localServiceActionFailed")) }
        guard case let .ok(endpoint, _, _) = HostInput.parseEndpoint(model.address) else {
            return .failed(message: L("localPairingAddressInvalid"))
        }
        let fingerprint = response.device?.fingerprint ?? verify.peerFingerprint
        let name = response.device?.name ?? verify.peerName
        return await connect(api: api, fingerprint: fingerprint, name: name, endpoint: endpoint)
    }

    /// 取凭据 → `addRemoteHost`（新增，或同指纹主机原地更新）→ 切换。
    private func connect(api: AgentAPI, fingerprint: String, name: String, endpoint: String) async -> PairingModel.Completion {
        let token: String
        do throws(HostError) {
            token = try await api.linkControlCredential(fingerprint: fingerprint).token
        } catch {
            return .failed(message: AccountText.error(error, context: .pairing))
        }
        switch await container.addRemoteHost(name: name, endpoint: endpoint, accessKey: token, linkFingerprint: fingerprint) {
        case let .success(ref):
            // 主机已保存；无论切换是否成功都算完成（切换结果由 Toast 回执）。
            await HostFlow.switchTo(ref, container: container)
            return .done(name: ref.localizedName)
        case let .failure(error):
            return .failed(message: HostFlow.errorText(error, paired: true))
        }
    }

    // MARK: 直接连接（已配对且可控制）

    /// 直接连接一台已配对且 `controllable` 的主机。成功返回 nil；失败返回可见文案，`unauthorized` 时第二项为 true
    /// （主机侧已撤销授权：调用方转到配对码输入）。
    func connectDirect(fingerprint: String, name: String, address: String) async -> (message: String, repair: Bool)? {
        guard connectingId == nil, let api, let model else { return nil }
        connectingId = fingerprint
        defer { connectingId = nil }
        guard case let .ok(endpoint, _, _) = HostInput.parseEndpoint(address) else {
            return (L("localPairingAddressInvalid"), false)
        }
        let token: String
        do throws(HostError) {
            token = try await api.linkControlCredential(fingerprint: fingerprint).token
        } catch {
            // 本机名册里这条记录已不可控制（被撤销 / 重配对没带控制）：走配对。
            let repair = error.reason == "peerNotPaired"
            return (repair ? L("mobileHostRepairNeeded") : AccountText.error(error, context: .pairing), repair)
        }
        switch await container.addRemoteHost(name: name, endpoint: endpoint, accessKey: token, linkFingerprint: fingerprint) {
        case let .success(ref):
            await HostFlow.switchTo(ref, container: container)
            model.markDirectDone(name: ref.localizedName)
            return nil
        case let .failure(error):
            return (HostFlow.errorText(error, paired: true), error.code == .unauthorized)
        }
    }
}
