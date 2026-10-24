import FluxDomain
import FluxUI
import Foundation
import Observation
import OSLog

/// 配对状态机（本机配对码 / 发现 / 手动配对 / SAS 核对），两处共用：
/// - V4「添加设备」（`AddDeviceSheet`）：对**当前主机**的 agent 配对，互为设备；
/// - 「添加主机 → 局域网配对」（`AddHostSheet`）：对**本机主机的独立会话**配对并请求完全控制对端
///   （`Options.requestControl`），配对完成后由 `completion` 取控制凭据并保存主机。
///
/// 生命周期：页签出现 `start()`、本机局域网实际可达（`LanAccess.active`）时开启发现 + 申请配对码；
/// 页签消失 / sheet 关闭 / 局域网不可达则 `stop()`（关闭发现 + `stopPairing`，并放弃进行中的发起流程）；
/// 发起流程在 begin 进行中 / SAS 核对 / 等待对方确认时被放弃，一律 `pairFinish(accept:false)` 释放会话
/// （等待确认阶段也一样：token 保留到流程结束，agent 据此立即中止对端的在途确认）。
/// 所有失败都落到可见的错误文案（`AccountText.error(_, context: .pairing)`），不伪造数据；只有拆除阶段（无界面可显示）才写日志。
@MainActor
@Observable
final class PairingModel {
    nonisolated struct Verify: Equatable {
        var token: String
        var sas: String
        var peerName: String
        var peerFingerprint: String
    }

    nonisolated enum Step: Equatable {
        /// 表单（本机配对码 + 发现列表 + 手动地址）。
        case form
        /// SAS 核对：等待用户确认或拒绝。
        case verify(Verify)
        /// 已确认，等待对端确认（对端有 60 秒窗口）。
        case finishing(Verify)
        /// 已配对（对端设备名）。
        case done(String)
        /// 对端拒绝。
        case rejected
    }

    nonisolated struct Options {
        /// `pairBegin` 请求完全控制对端（手机「添加主机」）。
        var requestControl = false
        /// 经 agent `discovery.set` 开关 Rust mDNS。iOS 本机主机没有 multicast entitlement，置 false，
        /// 由视图用 `LanBrowser`（NWBrowser）提供发现。
        var agentDiscovery = true
    }

    /// `completion` 的结果：配对成功后的收尾（取控制凭据、保存主机……）。
    nonisolated enum Completion {
        case done(name: String)
        case failed(message: String)
    }

    var address = ""
    var code = ""
    private(set) var step: Step = .form
    private(set) var connecting = false
    /// 核对页「拒绝 / 确认」提交中：防止重复点击。
    private(set) var submitting = false
    /// 表单 / 核对页的内联错误。
    private(set) var message: String?

    private(set) var own: LinkPairingCodeDto?
    /// 本机配对码的过期时刻（本机 Unix 毫秒）：收到 DTO 的本机时刻 + `ttlMs`（旧主机回退主机时钟的 `expiresAtUnixMs`）。
    private(set) var ownDeadlineMs: Int64 = 0
    /// 本机配对码的有效总时长（秒，取自拿到码时的剩余时间），供倒计时环使用。
    private(set) var ownTotal: TimeInterval = 120
    private(set) var ownError: String?
    private var ownRequests = 0
    private(set) var discoveryError: String?
    private(set) var scanning = false
    /// 发现开启 / 重新搜索后的 8 秒搜索窗口内（列表为空时显示「正在搜索…」）。
    private(set) var searching = false
    /// 本机局域网三态（由视图按 `agent.gateway` 分区同步；同步前按未就绪处理，不提前申请配对码 / 开发现）。
    private(set) var lan: LinkRules.LanAccess = .disabled

    var ownLoading: Bool { ownRequests > 0 }
    var usesAgentDiscovery: Bool { options.agentDiscovery }
    /// 发现列表是否仍在「搜索中」窗口内（agent 发现 / NWBrowser 各自计时）。
    var isSearching: Bool { options.agentDiscovery ? searching : browser.searching }
    /// iOS 本机主机的发现来源（`options.agentDiscovery == false` 时才会启动）：NWBrowser 浏览 `_fluxdown._tcp`。
    let browser = LanBrowser()

    @ObservationIgnored private unowned let container: AppContainer
    @ObservationIgnored private let api: (() -> AgentAPI)?
    @ObservationIgnored private let options: Options
    @ObservationIgnored private let completion: ((Verify, LinkPairFinishResponse) async -> Completion)?
    /// 配对码 / 发现是否正在运行（= 页签可见 && 局域网 active && 未关闭）。
    @ObservationIgnored private var active = false
    @ObservationIgnored private var visible = false
    /// 发起流程代次：放弃流程（离开页签 / 关闭 / 取消等待）时自增，迟到的响应据此丢弃。
    @ObservationIgnored private var flowGeneration = 0
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var pairingStarted = false
    /// 本模型最近一次拿到的配对码：`stopPairing` 只作废它（多客户端互不干扰）。
    @ObservationIgnored private var issuedCode: String?
    @ObservationIgnored private var closed = false
    @ObservationIgnored private var chain: Task<Void, Never>?
    /// 放弃会话的 `pairFinish(accept:false)` 串行链（拆除时可等它跑完再关会话）。
    @ObservationIgnored private var rejects: Task<Void, Never>?
    @ObservationIgnored private var expiryTask: Task<Void, Never>?
    private static let log = Logger(subsystem: "com.fluxdown.app", category: "pairing")

    /// 配对码到期后重新申请前的最短等待（防止主机时钟偏差造成刷新风暴）。
    private static let minRefreshDelayMs: Int64 = 3000

    /// `api` 缺省 = 当前主机的 agent（每次调用取最新会话）。
    init(
        container: AppContainer,
        api: (() -> AgentAPI)? = nil,
        options: Options = Options(),
        completion: ((Verify, LinkPairFinishResponse) async -> Completion)? = nil
    ) {
        self.container = container
        self.api = api
        self.options = options
        self.completion = completion
    }

    private var agent: AgentAPI { api?() ?? container.agent }

    // MARK: 生命周期

    /// 直连页签出现：开启发现并申请本机配对码。
    func start() {
        visible = true
        closed = false
        reconcile()
    }

    /// 视图同步本机局域网三态；变为 active 时补开发现与配对码，离开 active 时撤回。
    func setLan(_ access: LinkRules.LanAccess) {
        guard lan != access else { return }
        lan = access
        reconcile()
    }

    private func reconcile() {
        let shouldRun = visible && !closed
        if shouldRun, !active {
            active = true
            if options.agentDiscovery {
                enqueue { [self] in await setDiscovery(true, report: true) }
            } else {
                browser.start()
            }
            beginSearching()
        } else if !shouldRun, active {
            active = false
            searchTask?.cancel()
            searchTask = nil
            searching = false
            if options.agentDiscovery {
                enqueue { [self] in await setDiscovery(false, report: false) }
            } else {
                browser.stop()
            }
        }
        // 本机配对码（及其 mDNS 广播）只在局域网实际可达时申请；发现 / 发起配对不依赖它。
        if active, lan == .active {
            refreshOwnCode()
        } else if pairingStarted || own != nil || ownError != nil {
            expiryTask?.cancel()
            expiryTask = nil
            own = nil
            ownError = nil
            let stopCode = pairingStarted
            pairingStarted = false
            if stopCode { enqueue { [self] in await stopPairing() } }
        }
    }

    /// 直连页签消失：关闭发现与配对码广播（顺序在已排队的操作之后），并放弃进行中的发起流程。
    func stop() {
        visible = false
        reconcile()
        abandonFlow()
    }

    /// sheet 关闭：停止一切，并放弃仍在等待的会话。
    func dismissed() {
        closed = true
        stop()
    }

    /// `dismissed()` 之后等待全部收尾调用（关发现 / 停配对码 / 放弃会话）跑完；调用方随后才能安全关闭独立会话。
    func shutdown() async {
        dismissed()
        await chain?.value
        await rejects?.value
    }

    /// 放弃进行中的发起流程：核对 / 等待阶段立即释放会话，begin 在途的迟到响应由代次丢弃并释放。
    private func abandonFlow() {
        flowGeneration += 1
        submitting = false
        switch step {
        case let .verify(verify), let .finishing(verify):
            step = .form
            reject(token: verify.token)
        default:
            break
        }
    }

    /// 等待对方确认阶段点「取消」：回到表单，并向 agent 取消在途确认（`pairFinish accept=false`，对端随之收到取消）。
    func cancelFinishing() {
        guard case let .finishing(verify) = step else { return }
        flowGeneration += 1
        submitting = false
        step = .form
        message = nil
        reject(token: verify.token)
    }

    private func beginSearching() {
        searchTask?.cancel()
        searching = true
        searchTask = Task { [self] in
            do {
                try await Task.sleep(for: .seconds(Self.searchWindow))
            } catch {
                return
            }
            searching = false
        }
    }

    private static let searchWindow = 8

    // MARK: 本机配对码

    /// 申请（或换一个）本机配对码；已有申请在途时忽略。
    func refreshOwnCode() {
        guard active, ownRequests == 0 else { return }
        ownRequests += 1
        pairingStarted = true
        enqueue { [self] in await runRefreshOwn() }
    }

    private func runRefreshOwn() async {
        defer { ownRequests -= 1 }
        guard active else { return }
        do throws(HostError) {
            let result = try await agent.linkPairingCode()
            let receivedAt = AccountText.nowMs()
            issuedCode = result.code
            guard active else { return }
            own = result
            ownError = nil
            ownDeadlineMs = LinkRules.deadlineMs(ttlMs: result.ttlMs, expiresAtUnixMs: result.expiresAtUnixMs, receivedAtMs: receivedAt)
            let remaining = LinkRules.secondsUntil(expiresAtUnixMs: ownDeadlineMs, nowMs: receivedAt)
            ownTotal = max(1, TimeInterval(remaining))
            scheduleExpiry(ownDeadlineMs)
        } catch {
            ownError = AccountText.error(error, context: .pairing)
        }
    }

    /// 配对码到期自动换新。
    private func scheduleExpiry(_ deadlineMs: Int64) {
        expiryTask?.cancel()
        let wait = max(deadlineMs - AccountText.nowMs(), Self.minRefreshDelayMs)
        expiryTask = Task { [self] in
            do {
                try await Task.sleep(for: .milliseconds(wait))
            } catch {
                return
            }
            refreshOwnCode()
        }
    }

    // MARK: 发现

    private func setDiscovery(_ enabled: Bool, report: Bool) async {
        guard options.agentDiscovery else { return }
        do throws(HostError) {
            try await agent.linkDiscoverySet(enabled: enabled)
            if enabled { discoveryError = nil }
        } catch {
            if report {
                discoveryError = AccountText.error(error, context: .pairing)
            } else {
                Self.log.error("link discovery off failed: \(error.description, privacy: .public)")
            }
        }
    }

    /// 重新搜索：agent 发现 关 → 开；NWBrowser 发现重启浏览。
    func rescan() {
        guard active, !scanning else { return }
        if !options.agentDiscovery {
            browser.restart()
            return
        }
        scanning = true
        beginSearching()
        enqueue { [self] in
            await setDiscovery(false, report: true)
            await setDiscovery(true, report: true)
            scanning = false
        }
    }

    /// 只作废本模型自己拿到的码（带 `code`）：别的客户端刚换出来的新码不受影响。
    private func stopPairing() async {
        guard let code = issuedCode else { return }
        issuedCode = nil
        do throws(HostError) {
            try await agent.linkStopPairing(code: code)
        } catch {
            Self.log.error("link stopPairing failed: \(error.description, privacy: .public)")
        }
    }

    private func enqueue(_ operation: @escaping @MainActor () async -> Void) {
        let previous = chain
        chain = Task {
            await previous?.value
            await operation()
        }
    }

    // MARK: 配对

    /// 视图在发现解析失败等场景给表单一条内联错误。
    func showMessage(_ text: String) {
        message = text
        FluxHaptic.error.play()
    }

    /// 校验输入并发起配对（`agent.link.pairBegin`），成功进入 SAS 核对。
    func begin() async {
        guard !connecting, step == .form else { return }
        switch LinkRules.validate(address: address, code: code, requestControl: options.requestControl) {
        case let .failure(input):
            message = L(input.key)
        case let .success(params):
            connecting = true
            message = nil
            let generation = flowGeneration
            defer { connecting = false }
            do throws(HostError) {
                let response = try await agent.linkPairBegin(params)
                if closed || generation != flowGeneration {
                    // 对话框 / 页签已在等待期间离开：不留悬挂会话。
                    await rejectSilently(token: response.token)
                    return
                }
                step = .verify(Verify(
                    token: response.token,
                    sas: response.sas,
                    peerName: response.peerName,
                    peerFingerprint: response.peerFingerprint
                ))
            } catch {
                message = pairingErrorText(error)
                FluxHaptic.error.play()
            }
        }
    }

    /// SAS 核对后的决定：确认 → 等待对端；拒绝 → 回到表单。
    func finish(accept: Bool) async {
        guard case let .verify(verify) = step, !submitting else { return }
        message = nil
        submitting = true
        let generation = flowGeneration
        // 被取消 / 放弃的流程已在 `cancelFinishing` / `abandonFlow` 里复位 `submitting`，不能再动新流程的状态。
        defer { if generation == flowGeneration { submitting = false } }
        if accept { step = .finishing(verify) }
        do throws(HostError) {
            let response = try await agent.linkPairFinish(token: verify.token, accept: accept)
            // 等待期间被取消 / 离开：结果作废（配对若已成立，由已配对列表的推送体现）。
            guard generation == flowGeneration else { return }
            if !accept {
                step = .form
                code = ""
            } else if response.paired {
                await conclude(verify, response, generation: generation)
            } else {
                step = .rejected
                FluxHaptic.warning.play()
            }
        } catch {
            guard generation == flowGeneration else { return }
            step = .form
            message = pairingErrorText(error)
            FluxHaptic.error.play()
        }
    }

    /// 配对已成立：有 `completion` 时交给它收尾（添加主机取凭据并保存），否则直接报成功。
    private func conclude(_ verify: Verify, _ response: LinkPairFinishResponse, generation: Int) async {
        guard let completion else {
            let name = response.device?.name ?? verify.peerName
            step = .done(name)
            FluxHaptic.success.play()
            container.toasts.show(text: L("localPairingPaired", ["device": name]), tone: .success)
            return
        }
        let outcome = await completion(verify, response)
        guard generation == flowGeneration else { return }
        switch outcome {
        case let .done(name):
            step = .done(name)
            FluxHaptic.success.play()
        case let .failed(text):
            step = .form
            code = ""
            message = text
            FluxHaptic.error.play()
        }
    }

    private func pairingErrorText(_ error: HostError) -> String {
        if options.requestControl, LinkRules.isControlUnsupported(error) { return L("mobileAddHostControlUnsupported") }
        return AccountText.error(error, context: .pairing)
    }

    /// 免配对直接连接（已配对且可控制的主机）成功：直接进入完成态（不经 SAS）。
    func markDirectDone(name: String) {
        step = .done(name)
        FluxHaptic.success.play()
    }

    /// 拒绝页 / 完成后返回表单。
    func backToForm() {
        step = .form
        message = nil
        code = ""
    }

    /// 放弃会话（串行，拆除时 `shutdown()` 会等它们跑完）。
    private func reject(token: String) {
        let previous = rejects
        rejects = Task { [self] in
            await previous?.value
            await rejectSilently(token: token)
        }
    }

    private func rejectSilently(token: String) async {
        do throws(HostError) {
            _ = try await agent.linkPairFinish(token: token, accept: false)
        } catch {
            Self.log.error("link pairFinish(reject) failed: \(error.description, privacy: .public)")
        }
    }
}
