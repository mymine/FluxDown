import FluxDomain
import FluxUI
import SwiftUI

/// V2 添加远程主机（`fluxdown-agent --server` / 开启局域网访问的桌面 agent），两种方式：
/// - 局域网配对（默认）：发现 / 手输地址 → 主机上显示的 6 位配对码 → 双方核对 SAS → 主机侧批准「完全控制」→ 得到设备专属
///   控制凭据，保存并切换。配对始终对**本机主机的独立会话**进行（`HostPairingFlow`），与当前连着哪台主机无关；
/// - 访问密钥：名称、地址、密钥 → 连接测试 → 保存并切换（原表单）。
/// 系统表单 Sheet（玻璃由系统提供）；表单每次打开重建，访问密钥只存在于本 View 的 `@State`，
/// 持久化加密由 `AppContainer.addRemoteHost`（钥匙串）负责。
struct AddHostSheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss

    private nonisolated enum Field: Hashable { case name, address, key, pairAddress, pairCode }
    private nonisolated enum Mode: Hashable { case pair, key }

    @State private var mode: Mode = .pair
    @State private var flow: HostPairingFlow?

    @State private var name = ""
    @State private var address = ""
    @State private var key = ""
    @State private var revealKey = false
    /// 点过「连接」后才显示本地校验错误，避免输入中途报错。
    @State private var attempted = false
    @State private var busy = false
    /// 主机拒绝了这把密钥（Unauthorized）；改动密钥后清除。
    @State private var keyRejected = false
    /// 连接失败横幅文案（不可达 / 超时 / 版本不兼容 / 其它）。
    @State private var failure: String?
    @State private var failureCount = 0
    @FocusState private var focus: Field?

    var body: some View {
        let pairModel = flow?.model
        let pairStep = pairModel?.step ?? .form
        let showsPicker = mode == .key || pairStep == .form
        NavigationStack {
            Form {
                if showsPicker {
                    Section {
                        Picker(
                            L("mobileHostAddTitle"),
                            selection: $mode
                        ) {
                            Text(L("mobileAddHostModePair")).tag(Mode.pair)
                            Text(L("mobileAddHostModeKey")).tag(Mode.key)
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                        .disabled(busy)
                    }
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
                }
                switch mode {
                case .pair:
                    pairContent()
                case .key:
                    keyContent()
                }
            }
            .disabled(busy)
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(L("mobileHostAddTitle"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("cancel"), role: .cancel) { dismiss() }
                        .disabled(busy)
                }
                ToolbarItem(placement: .confirmationAction) {
                    confirmItem(pairModel: pairModel, pairStep: pairStep)
                }
            }
            .safeAreaInset(edge: .bottom) {
                if mode == .pair, let pairModel { PairingVerifyBar(model: pairModel) }
            }
            .interactiveDismissDisabled(busy || pairModel?.connecting == true)
            .sensoryFeedback(.error, trigger: failureCount)
            .task { await prepare() }
            .onChange(of: mode) { _, new in
                if new == .pair { flow?.model?.start() } else { flow?.model?.stop() }
            }
            .onChange(of: flow?.model?.step) { _, step in
                // 配对 / 直接连接成功：主机已保存并切换，收起表单（切换结果由 Toast 回执）。
                if case .done? = step { dismiss() }
            }
            .onDisappear {
                key = ""
                let flow = flow
                Task { await flow?.close() }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    // MARK: 工具栏

    @ViewBuilder
    private func confirmItem(pairModel: PairingModel?, pairStep: PairingModel.Step) -> some View {
        switch mode {
        case .key:
            if busy {
                ProgressView().accessibilityLabel(L("mobileHostConnect"))
            } else {
                Button(L("mobileHostConnect"), action: submit)
            }
        case .pair:
            if let pairModel, pairStep == .form {
                if pairModel.connecting {
                    ProgressView().accessibilityLabel(L("mobileHostConnect"))
                } else {
                    Button(L("mobileHostConnect")) { beginPairing(pairModel) }
                }
            }
        }
    }

    // MARK: 局域网配对

    /// 打开本机会话、建好状态机（面板出现时一次性消费「重新配对」预填地址），然后开始发现。
    private func prepare() async {
        guard flow == nil else { return }
        let created = HostPairingFlow(container: container)
        flow = created
        let prefill = container.router.addHostRepairAddress
        container.router.addHostRepairAddress = nil
        await created.open(prefill: prefill)
        if mode == .pair { created.model?.start() }
    }

    @ViewBuilder
    private func pairContent() -> some View {
        if let flow {
            switch flow.phase {
            case .opening:
                Section {
                    HStack(spacing: 12) {
                        ProgressView()
                        Text(L("mobileHostConnConnecting"))
                            .foregroundStyle(.secondary)
                    }
                    .accessibilityElement(children: .combine)
                }
            case let .failed(text):
                Section {
                    Banner(text: text, tone: .error, systemImage: FluxSymbol.failure, slim: true)
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets())
                    Button(L("mobileRetry"), systemImage: FluxSymbol.retry) {
                        Task {
                            await flow.retry(prefill: nil)
                            if mode == .pair { flow.model?.start() }
                        }
                    }
                }
            case .ready:
                if let model = flow.model { readyContent(flow: flow, model: model) }
            }
        }
    }

    @ViewBuilder
    private func readyContent(flow: HostPairingFlow, model: PairingModel) -> some View {
        switch model.step {
        case .form:
            pairForm(flow: flow, model: model)
        case let .verify(verify), let .finishing(verify):
            PairingVerifySection(
                verify: verify,
                finishing: model.step != .verify(verify),
                message: model.message,
                waitingText: L("mobileAddHostWaitingApproval"),
                hint: L("mobileAddHostVerifyHint")
            )
        case let .done(name):
            PairingDoneSection(text: L("mobileHostSwitched", ["name": name])) { dismiss() }
        case .rejected:
            PairingRejectedSection { model.backToForm() }
        }
    }

    @ViewBuilder
    private func pairForm(flow: HostPairingFlow, model: PairingModel) -> some View {
        @Bindable var model = model
        let entries = LanDiscovery.entries(model.browser.adverts, paired: flow.paired)
        Section {
            Text(L("mobileAddHostPairIntro"))
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 0, leading: 4, bottom: 0, trailing: 4))
        }

        Section {
            if entries.isEmpty {
                if model.isSearching {
                    HStack(spacing: 12) {
                        ProgressView()
                        Text(L("mobileAddHostSearching"))
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                    .accessibilityElement(children: .combine)
                } else {
                    Text(L("mobileAddHostNoneFound"))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }
            ForEach(entries) { entry in
                let resolved = model.browser.resolved[entry.id]
                Button {
                    Task { await pick(entry, flow: flow, model: model) }
                } label: {
                    PairingPeerRow(
                        name: entry.name,
                        platform: entry.platform,
                        detail: resolved,
                        paired: entry.controllable,
                        selected: resolved != nil && model.address == resolved,
                        busy: model.browser.resolvingId == entry.id || (entry.fingerprint != nil && flow.connectingId == entry.fingerprint)
                    )
                }
                .disabled(model.browser.resolvingId != nil || flow.connectingId != nil)
            }
            Button {
                model.rescan()
            } label: {
                Label(L("localPairingRetryScan"), systemImage: FluxSymbol.retry)
            }
            if let failure = model.browser.failure {
                Label(failure, systemImage: FluxSymbol.failure)
                    .font(.footnote)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
        } header: {
            Text(L("mobileAddHostDiscovered"))
        }

        Section {
            TextField(L("mobileHostAddress"), text: $model.address, prompt: Text(verbatim: "192.168.1.20:17800"))
                .font(.body.monospaced())
                .keyboardType(.URL)
                .textContentType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.next)
                .focused($focus, equals: .pairAddress)
                .onSubmit { focus = .pairCode }
        } header: {
            Text(L("mobileAddHostManualAddress"))
        } footer: {
            Text(L("mobileHostAddressHint"))
        }

        Section {
            TextField(
                L("localPairingCodeLabel"),
                text: Binding(
                    get: { model.code },
                    set: { model.code = LinkRules.normalizePairingCode($0) }
                ),
                prompt: Text(L("localPairingCodePlaceholder"))
            )
            .font(.body.monospacedDigit())
            .keyboardType(.numberPad)
            .textContentType(.oneTimeCode)
            .focused($focus, equals: .pairCode)
            if let message = model.message {
                Label(message, systemImage: FluxSymbol.failure)
                    .font(.footnote)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
            Button {
                beginPairing(model)
            } label: {
                HStack {
                    Text(L("mobileHostConnect"))
                    if model.connecting {
                        Spacer()
                        ProgressView()
                    }
                }
            }
            .disabled(model.connecting)
        } header: {
            Text(L("localPairingCodeLabel"))
        } footer: {
            Text(L("localPairingCodeHint"))
        }
    }

    private func beginPairing(_ model: PairingModel) {
        focus = nil
        Task { await model.begin() }
    }

    /// 点选发现列表里的一台：先解析成 IPv4 `host:port`；已配对且可控制的直接连接，否则填入地址等输配对码。
    private func pick(_ entry: LanDiscovery.Entry, flow: HostPairingFlow, model: PairingModel) async {
        guard let resolved = await model.browser.resolve(entry) else {
            model.showMessage(L("errReasonPairingPeerUnreachable"))
            return
        }
        model.address = resolved
        guard entry.controllable, let fingerprint = entry.fingerprint else {
            focus = .pairCode
            return
        }
        if let failure = await flow.connectDirect(fingerprint: fingerprint, name: entry.name, address: resolved) {
            // 授权已被主机侧撤销 / 连不上：留在表单，给出原因；撤销时转到配对码输入（重新配对）。
            model.showMessage(failure.message)
            if failure.repair { focus = .pairCode }
        }
    }

    // MARK: 访问密钥

    @ViewBuilder
    private func keyContent() -> some View {
        let parsed = HostInput.parseEndpoint(address)
        let keyIssue = HostInput.validateAccessKey(key.trimmingCharacters(in: .whitespacesAndNewlines))
        let cleartext: Bool = if case .ok(_, _, true) = parsed { true } else { false }
        Section {
            Text(L("mobileHostAddSub"))
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 0, leading: 4, bottom: 0, trailing: 4))
        }

        Section {
            TextField(L("mobileHostName"), text: $name, prompt: Text(L("mobileHostNamePlaceholder")))
                .textInputAutocapitalization(.words)
                .submitLabel(.next)
                .focused($focus, equals: .name)
                .onSubmit { focus = .address }
        }

        Section {
            TextField(L("mobileHostAddress"), text: $address, prompt: Text(verbatim: "192.168.1.20:17800"))
                .font(.body.monospaced())
                .keyboardType(.URL)
                .textContentType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.next)
                .focused($focus, equals: .address)
                .onSubmit { focus = .key }
                .onChange(of: address) { failure = nil }
            if cleartext {
                Banner(text: L("mobileHostCleartext"), tone: .warning, systemImage: FluxSymbol.warning, slim: true)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }
        } header: {
            Text(L("mobileHostAddress"))
        } footer: {
            FieldFooter(hint: L("mobileHostAddressHint"), error: attempted ? addressError(parsed) : nil)
        }

        Section {
            HStack {
                Group {
                    if revealKey {
                        TextField(L("webAccessKey"), text: $key, prompt: Text(L("webAccessKeyPlaceholder")))
                            .font(.body.monospaced())
                    } else {
                        SecureField(L("webAccessKey"), text: $key, prompt: Text(L("webAccessKeyPlaceholder")))
                    }
                }
                .textContentType(.password)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.go)
                .focused($focus, equals: .key)
                .onSubmit(submit)
                .onChange(of: key) {
                    keyRejected = false
                    failure = nil
                }
                Button {
                    revealKey.toggle()
                } label: {
                    Image(systemName: revealKey ? "eye.slash" : "eye")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel(L(revealKey ? "webHideKey" : "webShowKey"))
            }
        } header: {
            Text(L("webAccessKey"))
        } footer: {
            FieldFooter(
                hint: L("mobileHostKeyHint", ["min": HostInput.accessKeyMinLength, "max": HostInput.accessKeyMaxLength]),
                error: keyError(keyIssue)
            )
        }

        if let failure {
            Section {
                Banner(text: failure, tone: .error, systemImage: FluxSymbol.failure, slim: true)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }
        }
    }

    // MARK: 校验文案

    private func addressError(_ parsed: EndpointParse) -> String? {
        switch parsed {
        case .empty: L("localPairingHostRequired")
        case .badAddress: L("localPairingAddressInvalid")
        case .badPort: L("localPairingPortInvalid")
        case .ok: nil
        }
    }

    private func keyError(_ issue: KeyIssue?) -> String? {
        if keyRejected { return L("webLoginInvalidKey") }
        guard attempted, let issue else { return nil }
        switch issue {
        case .badChars: return L("webKeyBadChars")
        case .tooShort: return L("webKeyTooShort", ["min": HostInput.accessKeyMinLength])
        case .tooLong: return L("webKeyTooLong", ["max": HostInput.accessKeyMaxLength])
        case .needsMix: return L("webKeyNeedsMix")
        }
    }

    // MARK: 提交（访问密钥）

    private func submit() {
        guard !busy else { return }
        attempted = true
        let accessKey = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard case let .ok(endpoint, host, _) = HostInput.parseEndpoint(address),
              HostInput.validateAccessKey(accessKey) == nil
        else {
            failureCount += 1
            return
        }
        busy = true
        failure = nil
        keyRejected = false
        focus = nil
        let displayName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        Task {
            let result = await container.addRemoteHost(
                name: displayName.isEmpty ? host : displayName,
                endpoint: endpoint,
                accessKey: accessKey
            )
            switch result {
            case let .success(ref):
                // 主机已保存；无论切换是否成功都收起表单（切换结果由 Toast 回执）。
                await HostFlow.switchTo(ref, container: container)
                busy = false
                dismiss()
            case let .failure(error):
                failureCount += 1
                if error.code == .unauthorized {
                    keyRejected = true
                } else {
                    failure = HostFlow.errorText(error)
                }
                busy = false
            }
        }
    }
}

/// 字段下方的说明 + 校验错误（错误用「图标 + 文字」，不只靠颜色）。
private struct FieldFooter: View {
    let hint: String
    let error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(hint)
            if let error {
                Label(error, systemImage: FluxSymbol.failure)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
        }
    }
}
