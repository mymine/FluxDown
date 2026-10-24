import FluxDomain
import FluxUI
import Observation
import SwiftUI

// V5 入站配对请求：对端输入了本机配对码，本机必须核对 SAS 后明确确认 / 拒绝。
// 事实来源 = 分区 `agent.linkPairingRequests`（按到期排序）；同一时刻只呈现一个，其余以提示计数。

// MARK: - 呈现闸门

extension EnvironmentValues {
    /// 当前 `.linkPairingPrompts()` 实例的嵌套深度（根 = 0，每经过一层实例 +1）。
    @Entry var pairingPromptDepth = 0
}

/// 同一时刻只有一个 `.linkPairingPrompts()` 实例负责呈现全屏确认页：iOS 无法同时呈现两个 sheet / cover，
/// 而入站请求恰好常在 V4（一个 sheet）里到达。实例在视图存活期间登记在本闸门上，最深（同深度取最新登记）的存活实例呈现：
/// 根层实例注入闸门，`AddDeviceSheet` 等嵌套实例读取同一个闸门，于是 sheet 里的实例盖在 sheet 之上，根层实例静默；
/// sheet 关闭（视图状态销毁）后自动回到根层实例。
///
/// 登记按视图**存活**而不是 `onAppear` / `onDisappear`：全屏 cover 盖上来会让下面的视图触发 `onDisappear`，
/// 若据此注销，cover 一出现就会失去呈现权而被收起。
@MainActor
@Observable
final class PairingPromptGate {
    private struct Entry {
        var id: UUID
        var depth: Int
        var seq: Int
    }

    /// 当前负责呈现的实例。
    private(set) var presenter: UUID?
    /// 请求首次出现时算好的过期时刻（sessionId → 本机 Unix 毫秒）；只在视图 body 里读写，不参与观察。
    @ObservationIgnored private var receivedDeadlines: [String: Int64] = [:]
    /// 已由用户 / 到期处理过、等待分区移除的请求（乐观关闭，避免等主机回写前重复呈现）。
    private(set) var handled: Set<String> = []
    @ObservationIgnored private var entries: [Entry] = []
    @ObservationIgnored private var nextSeq = 0
    @ObservationIgnored private var announced: Set<String> = []
    /// 已批准、等待 `LinkedDevicesChanged` 出现该指纹才提示「已配对」：指纹 → (对端名, 批准时刻)。
    @ObservationIgnored private var awaitingPaired: [String: (name: String, at: Date, pairedAt: Int64?)] = [:]
    private static let awaitingWindow: TimeInterval = 120

    /// `pairedAt` = 批准时该指纹已有记录的配对时间（重新配对时以其变化为成功信号）；新指纹为 nil。
    func awaitPaired(fingerprint: String, name: String, pairedAt: Int64?) {
        awaitingPaired[fingerprint] = (name, Date(), pairedAt)
    }

    func cancelAwaitPaired(fingerprint: String) {
        awaitingPaired[fingerprint] = nil
    }

    /// 取走已出现在已配对列表里的待确认项（各嵌套实例共用，只会被第一个消费者取到）。
    func takePaired(in devices: [LinkDevice]) -> [String] {
        let now = Date()
        awaitingPaired = awaitingPaired.filter { now.timeIntervalSince($0.value.at) < Self.awaitingWindow }
        let current = Dictionary(devices.map { ($0.fingerprint, $0.pairedAt) }, uniquingKeysWith: { a, _ in a })
        let hit = awaitingPaired.filter { key, value in
            guard let now = current[key] else { return false }
            return value.pairedAt.map { $0 != now } ?? true
        }
        for key in hit.keys { awaitingPaired[key] = nil }
        return hit.values.map(\.name)
    }

    /// 登记一个实例（幂等）。
    func register(_ instance: UUID, depth: Int) {
        guard !entries.contains(where: { $0.id == instance }) else { return }
        nextSeq += 1
        entries.append(Entry(id: instance, depth: depth, seq: nextSeq))
        recompute()
    }

    func unregister(_ instance: UUID) {
        entries.removeAll { $0.id == instance }
        recompute()
    }

    private func recompute() {
        presenter = entries.max { ($0.depth, $0.seq) < ($1.depth, $1.seq) }?.id
    }

    func isPresenter(_ instance: UUID) -> Bool { presenter == instance }

    /// 标记已处理；已处理过返回 false（保证同一请求只响应一次）。
    func markHandled(_ sessionId: String) -> Bool {
        handled.insert(sessionId).inserted
    }

    func unmarkHandled(_ sessionId: String) {
        handled.remove(sessionId)
    }

    /// 请求已从分区消失：清掉对应的记录。
    func prune(keeping live: Set<String>) {
        let stale = handled.subtracting(live)
        if !stale.isEmpty { handled.subtract(stale) }
        announced.formIntersection(live)
        receivedDeadlines = receivedDeadlines.filter { live.contains($0.key) }
    }

    /// 请求的过期时刻（本机 Unix 毫秒）：**第一次见到该请求的本机时刻** + `ttlMs`（旧主机缺 `ttlMs` 才回退主机时钟的
    /// `expiresAtUnixMs`），之后固定不变——倒计时 / 自动拒绝都不受主机与本机时钟偏差影响。
    func deadlineMs(for request: LinkPairingRequestDto) -> Int64 {
        if let known = receivedDeadlines[request.sessionId] { return known }
        let value = LinkRules.deadlineMs(
            ttlMs: request.ttlMs,
            expiresAtUnixMs: request.expiresAtUnixMs,
            receivedAtMs: AccountText.nowMs()
        )
        receivedDeadlines[request.sessionId] = value
        return value
    }

    /// 首次提示返回 true（同一请求只弹一次「有配对请求」Toast）。
    func markAnnounced(_ sessionId: String) -> Bool {
        announced.insert(sessionId).inserted
    }
}

/// 实例的登记凭证：随修饰器的视图状态存活，销毁（sheet 关闭 / 视图移除）时从闸门注销。
@MainActor
final class PairingPromptClaim {
    let id = UUID()
    private var gate: PairingPromptGate?

    func register(in gate: PairingPromptGate, depth: Int) {
        self.gate = gate
        gate.register(id, depth: depth)
    }

    deinit {
        let gate = gate
        let id = id
        Task { @MainActor in gate?.unregister(id) }
    }
}

// MARK: - 修饰器

extension View {
    /// 在本视图层级之上呈现入站配对请求的全屏确认页。
    /// 根壳应用一次（`RootView` 的 shell）；会盖住自己 sheet 的页面（如 `AddDeviceSheet`）再应用一次。
    func linkPairingPrompts() -> some View {
        modifier(PairingPromptModifier())
    }
}

private struct PairingPromptModifier: ViewModifier {
    @Environment(AppContainer.self) private var container
    @Environment(PairingPromptGate.self) private var inherited: PairingPromptGate?
    @State private var ownGate = PairingPromptGate()
    @State private var claim = PairingPromptClaim()
    @State private var sections = AgentSections()
    @Environment(\.pairingPromptDepth) private var depth
    /// 路由 sheet 刚关闭的收尾期（UIKit 退场动画期间立刻再呈现会被丢弃）。
    @State private var sheetSettling = false

    func body(content: Content) -> some View {
        let gate = inherited ?? ownGate
        let isRoot = inherited == nil
        let state = container.store.state
        let pending = sections.pairingRequests(state)
        let pendingIds = Set(pending.map(\.sessionId))
        let queue = pending.filter { !gate.handled.contains($0.sessionId) }
        let isPresenter = gate.isPresenter(claim.id)
        // 根层实例不能可靠地盖在路由 sheet 之上：等它关闭（含退场动画）后再呈现（期间只弹一次 Toast 提示）。
        let routerSheetOpen = container.router.sheet != nil
        let blocked = isRoot && (routerSheetOpen || sheetSettling)
        let shown: LinkPairingRequestDto? = (isPresenter && !blocked) ? queue.first : nil
        let heldBack: LinkPairingRequestDto? = (isPresenter && blocked) ? queue.first : nil
        content
            .environment(gate)
            .environment(\.pairingPromptDepth, depth + 1)
            .onAppear { claim.register(in: gate, depth: depth) }
            .task(id: routerSheetOpen) {
                if routerSheetOpen {
                    sheetSettling = true
                    return
                }
                do {
                    try await Task.sleep(for: .milliseconds(600))
                } catch {
                    return
                }
                sheetSettling = false
            }
            .fullScreenCover(item: Binding(get: { shown }, set: { _ in })) { request in
                InboundPairingPrompt(
                    request: request,
                    deadlineMs: gate.deadlineMs(for: request),
                    queued: max(queue.count - 1, 0),
                    respond: { accept, automatic in respond(to: request, accept: accept, automatic: automatic, gate: gate) }
                )
            }
            .onChange(of: heldBack?.sessionId, initial: true) { _, id in
                guard let held = heldBack, held.sessionId == id, gate.markAnnounced(held.sessionId) else { return }
                container.toasts.show(
                    text: "\(L("localPairingIncomingTitle")) · \(held.peerName)",
                    tone: .info,
                    systemImage: "link.badge.plus"
                )
            }
            .onChange(of: pendingIds) { _, live in gate.prune(keeping: live) }
            // 批准后不提前报成功：以已配对列表里出现该指纹为准再提示。
            .onChange(of: state.linkDevices.map { "\($0.fingerprint)#\($0.pairedAt)" }) { _, _ in
                for name in gate.takePaired(in: container.store.state.linkDevices) {
                    FluxHaptic.success.play()
                    container.toasts.show(text: L("localPairingPaired", ["device": name]), tone: .success)
                }
            }
            .onChange(of: shown?.sessionId) { old, new in
                guard isPresenter else { return }
                if let old, old != new, !gate.handled.contains(old), !pendingIds.contains(old) {
                    // 请求在用户处理之前消失：对端取消 / 已过期 / 在别处处理。
                    container.toasts.show(text: L("mobileIncomingPairingCancelled"), tone: .info, systemImage: "link.badge.plus")
                }
            }
    }

    /// 向 agent 回复请求。先乐观标记已处理（确认页立即收起）；用户操作失败时撤销标记并 Toast，
    /// 到期自动拒绝失败不再重试（请求本身已过期，由 agent 清理）。
    private func respond(to request: LinkPairingRequestDto, accept: Bool, automatic: Bool, gate: PairingPromptGate) {
        guard gate.markHandled(request.sessionId) else { return }
        if accept {
            let existing = container.store.state.linkDevices.first { $0.fingerprint == request.peerFingerprint }
            gate.awaitPaired(fingerprint: request.peerFingerprint, name: request.peerName, pairedAt: existing?.pairedAt)
        }
        Task {
            do throws(HostError) {
                try await container.agent.linkApprove(sessionId: request.sessionId, accept: accept)
            } catch {
                gate.cancelAwaitPaired(fingerprint: request.peerFingerprint)
                if !automatic { gate.unmarkHandled(request.sessionId) }
                container.toasts.show(text: AccountText.error(error, context: .pairing), tone: .error)
            }
        }
    }
}

// MARK: - 确认页

/// 入站配对确认页（全屏、不可下拉关闭）：来源设备、SAS 大数字、倒计时、等宽的 拒绝 / 确认配对。
/// 到期自动拒绝恰好一次；触感：出现 `.warning`，确认 `.success`，拒绝 `.rigid`。
struct InboundPairingPrompt: View {
    let request: LinkPairingRequestDto
    /// 过期时刻（本机 Unix 毫秒）：收到请求的本机时刻 + `ttlMs`（见 ``PairingPromptGate/deadlineMs(for:)``）。
    let deadlineMs: Int64
    /// 排在后面等待的请求数。
    let queued: Int
    /// `(accept, automatic)`：`automatic` = 到期自动拒绝。
    let respond: (Bool, Bool) -> Void

    @State private var total: TimeInterval

    init(request: LinkPairingRequestDto, deadlineMs: Int64, queued: Int, respond: @escaping (Bool, Bool) -> Void) {
        self.request = request
        self.deadlineMs = deadlineMs
        self.queued = queued
        self.respond = respond
        let remaining = LinkRules.secondsUntil(expiresAtUnixMs: deadlineMs, nowMs: AccountText.nowMs())
        // 对端有 60 秒确认窗口；先看到时剩余更长就以剩余为准。
        _total = State(initialValue: max(60, TimeInterval(remaining)))
    }

    private var deadline: Date { Date(timeIntervalSince1970: Double(deadlineMs) / 1000) }

    var body: some View {
        ScrollView {
            VStack(spacing: 24) {
                if queued > 0 {
                    Label(L("mobileIncomingPairingMore", ["n": queued]), systemImage: "tray.full")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                GlyphTile(
                    systemImage: DevicePresentation.symbol(platform: request.peerPlatform),
                    tint: .accentColor,
                    size: 72
                )
                VStack(spacing: 8) {
                    Text(L("incomingPairingTitle"))
                        .font(.title2.weight(.bold))
                        .multilineTextAlignment(.center)
                        .accessibilityAddTraits(.isHeader)
                    Text(L("incomingPairingFrom", ["device": request.peerName]))
                        .font(.headline)
                        .multilineTextAlignment(.center)
                    Text(L("incomingPairingHint"))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if request.requestsControl {
                    // 对端请求完全控制本机：批准后它能看到并管理全部下载、改设置——必须醒目地警示。
                    VStack(spacing: 8) {
                        StatusBadge(text: L("linkControlRequestTag"), tone: .warning)
                        Text(L("linkControlRequestWarning"))
                            .font(.subheadline.weight(.medium))
                            .multilineTextAlignment(.center)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(14)
                    .frame(maxWidth: .infinity)
                    .background(Color.fdStatusWarning.opacity(0.15), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .accessibilityElement(children: .combine)
                }
                SasDigitsView(sas: request.sas, size: 64)
                countdown
            }
            .padding(.horizontal, 24)
            .padding(.top, 32)
            .padding(.bottom, 16)
            .frame(maxWidth: 560)
            .frame(maxWidth: .infinity)
        }
        .safeAreaInset(edge: .bottom) { buttons }
        .interactiveDismissDisabled(true)
        .task(id: request.sessionId) { await watchExpiry() }
    }

    // MARK: 倒计时

    private var countdown: some View {
        VStack(spacing: 8) {
            CountdownRing(deadline: deadline, total: total) { L("incomingPairingCountdown", ["seconds": $0]) }
            TimelineView(.periodic(from: .now, by: 1)) { _ in
                let seconds = Int(LinkRules.secondsUntil(expiresAtUnixMs: deadlineMs, nowMs: AccountText.nowMs()))
                Text(L("incomingPairingCountdown", ["seconds": seconds]))
                    .font(.subheadline.weight(.medium))
                    .monospacedDigit()
                    .contentTransition(.numericText(countsDown: true))
                    .foregroundStyle(countdownStyle(seconds))
                    .fluxAnimation(.smooth, value: seconds)
            }
        }
    }

    /// 最后 10 秒变橙、5 秒变红（文字始终在，不只靠颜色）。
    private func countdownStyle(_ seconds: Int) -> Color {
        if seconds <= 5 { return .fdStatusFailedText }
        if seconds <= 10 { return .fdStatusWarningText }
        return .secondary
    }

    /// 出现时 `.warning` 触感；到期时自动拒绝一次（任务随确认页消失被取消，不会误拒别的请求）。
    private func watchExpiry() async {
        FluxHaptic.warning.play()
        let remaining = deadlineMs - AccountText.nowMs()
        if remaining > 0 {
            do {
                try await Task.sleep(for: .milliseconds(remaining))
            } catch {
                return
            }
        }
        respond(false, true)
    }

    // MARK: 按钮

    private var buttons: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) { rejectButton; acceptButton }
            VStack(spacing: 12) { acceptButton; rejectButton }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 8)
        .frame(maxWidth: 560)
        .frame(maxWidth: .infinity)
    }

    private var rejectButton: some View {
        Button(role: .destructive) {
            FluxHaptic.rigid.play()
            respond(false, false)
        } label: {
            Text(L("incomingPairingReject"))
                .frame(maxWidth: .infinity)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(.glass)
        .controlSize(.large)
    }

    private var acceptButton: some View {
        Button {
            FluxHaptic.success.play()
            respond(true, false)
        } label: {
            Text(L(request.requestsControl ? "linkControlApprove" : "incomingPairingAccept"))
                .frame(maxWidth: .infinity)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(.glassProminent)
        .controlSize(.large)
    }
}
