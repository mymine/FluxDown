import FluxDomain
import FluxUI
import SwiftUI
import UIKit

/// V4 添加设备：账户自动（同账号登录即出现）/ 直连配对（免账号，局域网发现 + 配对码 + SAS 核对）。
/// 页面自己应用 `.linkPairingPrompts()`：入站配对请求的全屏确认页会盖在本 sheet 之上，根层实例保持静默。
///
/// 配对状态机 `PairingModel` 由呈现方持有（每次呈现新建一个），并在 sheet 的 `onDismiss` 里调用 `dismissed()` 收尾：
/// 不能靠视图的 `onDisappear`——入站确认的全屏 cover 盖上来时下面的视图也会触发它。
struct AddDeviceSheet: View {
    let model: PairingModel
    /// 未登录时点「登录」：由呈现方关闭本 sheet 后再呈现登录 sheet。
    let requestLogin: () -> Void
    /// 配对完成（让设备页刷新已配对列表）。
    let onPaired: () -> Void

    var body: some View {
        AddDeviceContent(model: model, requestLogin: requestLogin, onPaired: onPaired)
    }
}

private struct AddDeviceContent: View {
    nonisolated enum Tab: Hashable { case account, local }
    nonisolated enum Field: Hashable { case address, code }

    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    let model: PairingModel
    @State private var sections = AgentSections()
    @State private var tab: Tab?
    @State private var celebrate = false
    @ScaledMetric(relativeTo: .largeTitle) private var doneIcon: CGFloat = 56
    @FocusState private var focus: Field?

    let requestLogin: () -> Void
    let onPaired: () -> Void

    var body: some View {
        @Bindable var model = model
        let state = container.store.state
        let accountCap = state.has(HostCapability.agentAuth)
        let linkCap = state.has(HostCapability.agentDeviceLink)
        let session = accountCap ? sections.session(state) : nil
        let defaultTab: Tab = (session != nil || !linkCap) ? .account : .local
        let current: Tab = linkCap ? (accountCap ? (tab ?? defaultTab) : .local) : .account
        let requestIds = sections.pairingRequests(state).map(\.sessionId)
        NavigationStack {
            List {
                if accountCap, linkCap, model.step == .form {
                    Section {
                        Picker(
                            L("addDeviceEntry"),
                            selection: Binding(get: { current }, set: { tab = $0 })
                        ) {
                            Text(L("addDeviceTabAccount")).tag(Tab.account)
                            Text(L("addDeviceTabLocal")).tag(Tab.local)
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                    }
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
                }
                switch current {
                case .account:
                    accountContent(session: session)
                case .local:
                    localContent(model: $model, state: state)
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle(L("addDeviceEntry"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("close")) { dismiss() }
                }
                ToolbarItemGroup(placement: .keyboard) {
                    Spacer()
                    if focus == .code {
                        Button(L("localPairingConnect")) { connect() }
                    }
                }
            }
            .safeAreaInset(edge: .bottom) { verifyBar }
            .scrollDismissesKeyboard(.interactively)
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .linkPairingPrompts()
        .onChange(of: sections.lanAccess(state), initial: true) { _, access in
            model.setLan(access)
        }
        .onChange(of: current == .local, initial: true) { _, isLocal in
            if isLocal { model.start() } else { model.stop() }
        }
        .onChange(of: requestIds) { old, new in
            // 有新的入站请求：本机配对码已被对端使用的可能性高，换一个。
            if current == .local, new.contains(where: { !old.contains($0) }) { model.refreshOwnCode() }
        }
        .onChange(of: model.step) { _, step in
            if case .done = step { onPaired() }
        }
    }

    // MARK: 账户页签

    @ViewBuilder
    private func accountContent(session: AgentSessionDto?) -> some View {
        Section {
            if let session {
                Label(L("addDeviceAccountSynced", ["account": session.user.email]), systemImage: FluxSymbol.success)
                    .foregroundStyle(.primary)
            } else {
                Label(L("addDeviceLoginRequired"), systemImage: "person.crop.circle.badge.exclamationmark")
                Button(L("accountLogin")) { requestLogin() }
            }
        } footer: {
            Text(L("addDeviceAccountFooter"))
        }
        Section {
            Text(L("addDeviceHint"))
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
    }

    // MARK: 直连页签

    @ViewBuilder
    private func localContent(model: Bindable<PairingModel>, state: HostState) -> some View {
        switch model.wrappedValue.step {
        case .form:
            formContent(model: model, state: state)
        case let .verify(verify), let .finishing(verify):
            verifyContent(verify, finishing: model.wrappedValue.step != .verify(verify), message: model.wrappedValue.message)
        case let .done(name):
            Section {
                VStack(spacing: 12) {
                    Image(systemName: FluxSymbol.success)
                        .font(.system(size: doneIcon))
                        .foregroundStyle(Color.fdStatusSeeding)
                        .symbolEffect(.bounce, value: celebrate)
                        .accessibilityHidden(true)
                    Text(L("localPairingPaired", ["device": name]))
                        .font(.headline)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
                .onAppear { celebrate = true }
            }
            Section {
                Button(L("close")) { dismiss() }
            }
        case .rejected:
            Section {
                Banner(text: L("errReasonPairingRejected"), tone: .warning, systemImage: "hand.raised.fill", slim: true)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }
            Section {
                Button(L("back")) { model.wrappedValue.backToForm() }
            }
        }
    }

    @ViewBuilder
    private func formContent(model: Bindable<PairingModel>, state: HostState) -> some View {
        let paired = state.linkDevices.filter { !$0.revoked }.map {
            LinkDeviceInfo(fingerprint: $0.fingerprint, name: $0.name, platform: $0.platform, online: $0.online)
        }
        let lan = sections.lanAccess(state)
        let entries = LinkRules.discoveredEntries(sections.discovered(state), paired: paired)
        let readOnly = state.isReadOnly
        Section {
            Text(L("localPairingHint"))
                .font(.subheadline)
                .foregroundStyle(.secondary)
            if readOnly {
                Banner(text: L("localServiceDisconnected"), tone: .warning, systemImage: FluxSymbol.offline, slim: true)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }
            if lan != .active {
                // 本机局域网未生效：仍允许发起，但只能单向（对端无法回连本机）。
                Banner(text: L("localPairingOneWayHint"), tone: .warning, systemImage: FluxSymbol.offline, slim: true)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }
        }
        OwnCodeSection(model: model.wrappedValue, lan: lan, openLanSettings: openLanSettings)
        discoveredSection(model: model, entries: entries)
        manualSection(model: model, readOnly: readOnly)
        Section {} footer: {
            Text(L("mobileAddDeviceForeground"))
        }
    }

    /// 收起本 sheet 并跳到「设置 → API 服务」（设置页只在远端主机且具备网关能力时有该入口）。
    private func openLanSettings() {
        dismiss()
        container.router.showSettings(.api)
    }

    private func discoveredSection(model: Bindable<PairingModel>, entries: [LinkRules.DiscoveredEntry]) -> some View {
        let searching = entries.isEmpty && model.wrappedValue.searching
        return Section {
            if searching {
                HStack(spacing: 12) {
                    ProgressView()
                    Text(L("localPairingDiscovering"))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            } else if entries.isEmpty {
                Text(L("localPairingNoDevices"))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            ForEach(entries) { entry in
                Button {
                    model.wrappedValue.address = entry.address
                    focus = .code
                } label: {
                    DiscoveredPeerRow(entry: entry, selected: model.wrappedValue.address == entry.address)
                }
                .disabled(entry.paired)
            }
            Button {
                model.wrappedValue.rescan()
            } label: {
                HStack {
                    Label(L("localPairingRetryScan"), systemImage: FluxSymbol.retry)
                    if model.wrappedValue.scanning {
                        Spacer()
                        ProgressView()
                    }
                }
            }
            .disabled(model.wrappedValue.scanning)
            if let error = model.wrappedValue.discoveryError {
                Label(error, systemImage: FluxSymbol.failure)
                    .font(.footnote)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
        } header: {
            if !entries.isEmpty { Text(L("localPairingNearbyTitle")) }
        }
    }

    @ViewBuilder
    private func manualSection(model: Bindable<PairingModel>, readOnly: Bool) -> some View {
        Section {
            TextField(L("localPairingAddressLabel"), text: model.address, prompt: Text(verbatim: "192.168.1.20:17800"))
                .font(.body.monospaced())
                .keyboardType(.URL)
                .textContentType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.next)
                .focused($focus, equals: .address)
                .onSubmit { focus = .code }
        } header: {
            Text(L("localPairingManualAddress"))
        } footer: {
            Text(L("localPairingAddressHint"))
        }
        Section {
            TextField(
                L("localPairingCodeLabel"),
                text: Binding(
                    get: { model.wrappedValue.code },
                    set: { model.wrappedValue.code = LinkRules.normalizePairingCode($0) }
                ),
                prompt: Text(L("localPairingCodePlaceholder"))
            )
            .font(.body.monospacedDigit())
            .keyboardType(.numberPad)
            .textContentType(.oneTimeCode)
            .focused($focus, equals: .code)
            if let message = model.wrappedValue.message {
                Label(message, systemImage: FluxSymbol.failure)
                    .font(.footnote)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
            Button {
                connect()
            } label: {
                HStack {
                    Text(L("localPairingConnect"))
                    if model.wrappedValue.connecting {
                        Spacer()
                        ProgressView()
                    }
                }
            }
            .disabled(model.wrappedValue.connecting || readOnly)
        } header: {
            Text(L("localPairingCodeLabel"))
        } footer: {
            Text(L("localPairingCodeHint"))
        }
    }

    private func connect() {
        focus = nil
        Task { await model.begin() }
    }

    // MARK: SAS 核对

    @ViewBuilder
    private func verifyContent(_ verify: PairingModel.Verify, finishing: Bool, message: String?) -> some View {
        Section {
            VStack(spacing: 16) {
                Text(L("localPairingSasTitle"))
                    .font(.headline)
                SasDigitsView(sas: verify.sas, size: 48)
                Text(verify.peerName)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                if finishing {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text(L("localPairingWaitingPeer"))
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
        } footer: {
            Text(L("localPairingSasHint"))
        }
        if let message {
            Section {
                Banner(text: message, tone: .error, systemImage: FluxSymbol.failure, slim: true)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }
        }
    }

    /// 核对页的底部操作（浮动功能控件，系统玻璃按钮）：拒绝 / 确认配对。
    @ViewBuilder
    private var verifyBar: some View {
        let step = model.step
        let pending: Bool = switch step {
        case .verify, .finishing: true
        default: false
        }
        if pending {
            let finishing: Bool = if case .finishing = step { true } else { false }
            if finishing {
                Button {
                    model.cancelFinishing()
                } label: {
                    Text(L("cancel"))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.glass)
                .controlSize(.large)
                .padding(.horizontal, 20)
                .padding(.vertical, 8)
            } else {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 12) { rejectButton(disabled: false); confirmButton(disabled: false) }
                    VStack(spacing: 12) { confirmButton(disabled: false); rejectButton(disabled: false) }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 8)
            }
        }
    }

    private func rejectButton(disabled: Bool) -> some View {
        Button(role: .destructive) {
            Task { await model.finish(accept: false) }
        } label: {
            Text(L("localPairingReject"))
                .frame(maxWidth: .infinity)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(.glass)
        .controlSize(.large)
        .disabled(disabled)
    }

    private func confirmButton(disabled: Bool) -> some View {
        Button {
            Task { await model.finish(accept: true) }
        } label: {
            Text(L("localPairingConfirm"))
                .frame(maxWidth: .infinity)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(.glassProminent)
        .controlSize(.large)
        .disabled(disabled)
    }
}

// MARK: - 发现的设备行

private struct DiscoveredPeerRow: View {
    let entry: LinkRules.DiscoveredEntry
    let selected: Bool

    var body: some View {
        HStack(spacing: 12) {
            GlyphTile(systemImage: DevicePresentation.symbol(platform: entry.platform), tint: .secondary, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.name)
                    .font(.body)
                    .foregroundStyle(.primary)
                    .lineLimit(2)
                Text([DevicePresentation.label(platform: entry.platform), entry.address].compactMap { $0 }.joined(separator: " · "))
                    .font(.footnote.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if entry.paired {
                StatusBadge(text: L("localDevicePairedTag"), tone: .neutral)
            } else if selected {
                Image(systemName: FluxSymbol.done)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.tint)
                    .accessibilityHidden(true)
            }
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

// MARK: - 本机配对码

/// 本机配对码卡：大号分组数字 + 倒计时 + 复制 / 换一个 + 可连接地址。展示宿主返回的真实结果；
/// 内嵌的本机宿主通常没有 TCP 网关（无地址），此时如实说明，申请失败则显示宿主错误。
private struct OwnCodeSection: View {
    @Environment(AppContainer.self) private var container
    let model: PairingModel
    let lan: LinkRules.LanAccess
    let openLanSettings: () -> Void

    @ScaledMetric(relativeTo: .largeTitle) private var codeSize: CGFloat = 44
    @State private var copyTick = 0

    var body: some View {
        Section {
            if lan != .active {
                lanHint
            } else if let own = model.own {
                codeBlock(own)
                addresses(own)
            } else if model.ownLoading {
                HStack(spacing: 12) {
                    ProgressView()
                    Text(L("mobileAddDeviceCodeLoading"))
                        .foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            }
            if lan == .active, let error = model.ownError {
                Label(error, systemImage: FluxSymbol.failure)
                    .font(.footnote)
                    .foregroundStyle(Color.fdStatusFailedText)
                if model.own == nil {
                    Button(L("localPairingMyCodeShow")) { model.refreshOwnCode() }
                        .disabled(model.ownLoading)
                }
            }
        } header: {
            Text(L("localPairingMyCodeTitle"))
        } footer: {
            Text(L("localPairingMyCodeHint"))
        }
    }

    /// 未开启 / 未生效局域网：不显示码，只给出原因；`disabled` 且设置页可达时附「前往设置」。
    @ViewBuilder
    private var lanHint: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(
                L(lan == .disabled ? "localPairingLanDisabledHint" : "localPairingLanRestartHint"),
                systemImage: FluxSymbol.offline
            )
            .font(.subheadline)
            .fixedSize(horizontal: false, vertical: true)
            if lan == .disabled, !container.isLocalHost, container.store.state.has(HostCapability.agentGateway) {
                Button(L("localPairingOpenLanSettings")) { openLanSettings() }
            }
        }
    }

    private func codeBlock(_ own: LinkPairingCodeDto) -> some View {
        VStack(spacing: 12) {
            Text(LinkRules.groupCode(own.code))
                .font(.system(size: min(codeSize, 72), weight: .semibold, design: .rounded))
                .monospacedDigit()
                .tracking(2)
                .lineLimit(1)
                .minimumScaleFactor(0.5)
                .contentTransition(.numericText())
                .fluxAnimation(.smooth, value: own.code)
                .textSelection(.enabled)
                .accessibilityLabel(L("localPairingMyCodeTitle"))
                .accessibilityValue(own.code.map(String.init).joined(separator: " "))
            countdown(own)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 20) { copyButton(own); refreshButton }
                VStack(alignment: .leading, spacing: 8) { copyButton(own); refreshButton }
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 4)
        .sensoryFeedback(.success, trigger: copyTick)
    }

    private func countdown(_ own: LinkPairingCodeDto) -> some View {
        TimelineView(.periodic(from: .now, by: 1)) { _ in
            let seconds = LinkRules.secondsUntil(expiresAtUnixMs: own.expiresAtUnixMs, nowMs: AccountText.nowMs())
            HStack(spacing: 8) {
                CountdownRing(
                    deadline: Date(timeIntervalSince1970: Double(own.expiresAtUnixMs) / 1000),
                    total: model.ownTotal,
                    label: { L("localDeviceCodeRemaining", ["seconds": $0]) }
                )
                Text(seconds > 0 ? L("localDeviceCodeRemaining", ["seconds": seconds]) : L("localPairingMyCodeExpired"))
                    .font(.subheadline)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func copyButton(_ own: LinkPairingCodeDto) -> some View {
        Button {
            UIPasteboard.general.string = own.code
            copyTick += 1
            container.toasts.show(text: L("localDeviceCodeCopied"), tone: .success)
        } label: {
            Label(L("localDeviceCodeCopy"), systemImage: FluxSymbol.copy)
        }
        .buttonStyle(.borderless)
    }

    private var refreshButton: some View {
        Button {
            model.refreshOwnCode()
        } label: {
            Label(L("localPairingMyCodeRefresh"), systemImage: FluxSymbol.retry)
        }
        .buttonStyle(.borderless)
        .disabled(model.ownLoading)
    }

    @ViewBuilder
    private func addresses(_ own: LinkPairingCodeDto) -> some View {
        if own.addresses.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Label(L("localPairingNoAddress"), systemImage: FluxSymbol.offline)
                    .font(.subheadline)
                if container.isLocalHost {
                    Text(L("mobileAddDeviceNoGatewayHint"))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 4) {
                Text(L("localPairingMyAddresses"))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                ForEach(own.addresses, id: \.self) { address in
                    Text(address)
                        .font(.footnote.monospaced())
                        .textSelection(.enabled)
                }
            }
        }
    }
}
