import FluxDomain
import FluxUI
import LocalAuthentication
import SwiftUI

/// S9 · API 服务（网关）：只在远端 `--server` 主机且具备 `agent.gateway` 能力时出现（设置首页已门控）。
///
/// 管理的是**该主机**的网关：地址只读展示（取当前连接的主机地址）、功能开关、访问令牌。
/// 令牌 = 访问密钥（gateway user token）：文本只经 `agent.gateway.revealToken` 按需读取（显示 / 复制前过设备认证），
/// 修改后立即把新密钥写入钥匙串并重连，保证会话不断。
struct ApiServicePage: View {
    @Environment(AppContainer.self) private var container

    var body: some View {
        ApiServiceContent(container: container)
    }
}

// MARK: - 模型

@MainActor
@Observable
final class ApiServiceModel {
    /// 开关的乐观值（等主机状态追上后丢弃）。
    private(set) var pending: [GatewayFeature: Bool] = [:]
    private(set) var pendingLan: Bool?
    private(set) var portBusy = false
    private(set) var token: String?
    private(set) var tokenBusy = false

    @ObservationIgnored private let container: AppContainer

    init(container: AppContainer) {
        self.container = container
    }

    private var session: any HostSession { container.session }

    /// 网关写入错误：端口占用 / 重启失败有专门文案，其余按通用规则。
    static func describe(_ error: HostError) -> String {
        switch error.reason {
        case "gatewayPortInUse": L("apiServicePortInUse")
        case "gatewayRestartFailed": L("apiServiceRestartFailed")
        default: ErrorText.describe(error)
        }
    }

    private func fail(_ error: HostError) {
        container.toasts.show(text: Self.describe(error), tone: .error)
    }

    // MARK: 开关

    func set(_ feature: GatewayFeature, _ on: Bool) async {
        pending[feature] = on
        do throws(HostError) {
            let _: GatewayStatusDto = try await session.call(HostMethod.agentGatewayPatch, params: feature.patch(on))
        } catch {
            pending[feature] = nil
            fail(error)
        }
    }

    func setLan(_ on: Bool) async {
        pendingLan = on
        do throws(HostError) {
            let _: GatewayStatusDto = try await session.call(HostMethod.agentGatewayPatch, params: GatewayPatchParams(lanEnabled: on))
        } catch {
            pendingLan = nil
            fail(error)
        }
    }

    /// 主机状态更新后丢弃已追上的乐观值。
    func reconcile(_ status: GatewayStatusDto) {
        for (feature, value) in pending where feature.isOn(status) == value { pending[feature] = nil }
        if pendingLan == status.lanEnabled { pendingLan = nil }
    }

    // MARK: 端口

    func setPort(_ port: Int) async {
        guard !portBusy else { return }
        portBusy = true
        do throws(HostError) {
            let status: GatewayStatusDto = try await session.call(HostMethod.agentGatewayPatch, params: GatewayPatchParams(port: port))
            container.toasts.show(text: L("apiServiceRestarted", ["port": status.port]), tone: .success)
        } catch {
            fail(error)
        }
        portBusy = false
    }

    // MARK: 令牌

    /// 设备认证（Face ID / Touch ID / 密码）：没有设置设备密码时无从认证，直接放行。
    /// 用户主动取消保持静默；锁定 / 不可用等其余错误用 Toast 告知。
    func authenticate(reason: String) async -> Bool {
        let context = LAContext()
        var probe: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &probe) else { return true }
        do {
            return try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
        } catch {
            if let laError = error as? LAError, [.userCancel, .appCancel, .systemCancel].contains(laError.code) {
                return false
            }
            container.toasts.show(text: error.localizedDescription, tone: .error)
            return false
        }
    }

    /// 认证后读取令牌明文（`agent.gateway.revealToken`）。
    func reveal() async -> Bool {
        guard !tokenBusy else { return false }
        guard await authenticate(reason: L("apiServiceTokenRevealAuth")) else { return false }
        return await loadToken()
    }

    @discardableResult
    func loadToken() async -> Bool {
        tokenBusy = true
        defer { tokenBusy = false }
        do throws(HostError) {
            let result: GatewayRevealTokenResult = try await session.call(HostMethod.agentGatewayRevealToken)
            token = result.userToken
            return true
        } catch {
            fail(error)
            return false
        }
    }

    func hideToken() { token = nil }

    /// 自定义令牌：写网关 → 同步钥匙串并重连。
    func saveToken(_ value: String) async {
        guard !tokenBusy else { return }
        tokenBusy = true
        defer { tokenBusy = false }
        do throws(HostError) {
            let _: GatewayStatusDto = try await session.call(HostMethod.agentGatewayPatch, params: GatewayPatchParams(userToken: value))
            token = value
        } catch {
            fail(error)
            return
        }
        await adoptAccessKey(value)
    }

    /// 重新生成：写网关 → 读新令牌 → 同步钥匙串并重连。
    func regenerate() async {
        guard !tokenBusy else { return }
        tokenBusy = true
        defer { tokenBusy = false }
        let fresh: String
        do throws(HostError) {
            let _: GatewayStatusDto = try await session.call(HostMethod.agentGatewayPatch, params: GatewayPatchParams(regenerateUserToken: true))
            let result: GatewayRevealTokenResult = try await session.call(HostMethod.agentGatewayRevealToken)
            fresh = result.userToken
        } catch {
            fail(error)
            return
        }
        token = fresh
        await adoptAccessKey(fresh)
    }

    /// 访问密钥已变：写入钥匙串并重开当前主机会话（旧会话还握着旧密钥，断线重连会被拒）。
    private func adoptAccessKey(_ key: String) async {
        if case let .failure(error) = await container.updateRemoteAccessKey(key) {
            fail(error)
        }
    }

    /// 忘记这台主机：删除已保存的主机与密钥并回到本机。
    func forgetHost() async {
        let id = container.host.id
        if case let .failure(error) = await container.removeHost(id: id) { fail(error) }
    }
}

// MARK: - 页面

private struct ApiServiceContent: View {
    @Environment(HostStore.self) private var store
    @Environment(AppContainer.self) private var container
    @State private var model: ApiServiceModel
    @State private var gateway = SectionMemo<GatewayStatusDto>(empty: GatewayStatusDto())
    @State private var confirmForget = false

    init(container: AppContainer) {
        _model = State(initialValue: ApiServiceModel(container: container))
    }

    private var endpoint: String {
        if case let .remote(_, _, endpoint) = container.host { return endpoint }
        return ""
    }

    var body: some View {
        let state = store.state
        let status = gateway.value(state.sections[HostSection.agentGateway])
        let loaded = state.sections[HostSection.agentGateway] != nil
        let readOnly = state.isReadOnly || !loaded
        SettingsPage(title: L("settingsCatApiService"), showsReadOnlyBanner: true) {
            hostSection
            addressSection(status)
            TokenSection(model: model, configured: status.userTokenConfigured, readOnly: readOnly)
            featuresSection(status, readOnly: readOnly)
            Section {
                Button(role: .destructive) { confirmForget = true } label: {
                    Label(L("mobileApiForgetHost"), systemImage: "rectangle.portrait.and.arrow.right")
                }
                .alert(L("mobileApiForgetHost"), isPresented: $confirmForget) {
                    Button(L("mobileApiForgetHost"), role: .destructive) { Task { await model.forgetHost() } }
                    Button(L("cancel"), role: .cancel) {}
                } message: {
                    Text(L("mobileApiForgetHostDesc"))
                }
            } footer: {
                Text(L("mobileApiForgetHostDesc"))
            }
        }
        .onChange(of: status) { _, new in model.reconcile(new) }
    }

    // MARK: 分组

    private var hostSection: some View {
        Section {
            LabeledContent(L("mobileHostSwitchTitle")) {
                Text(container.host.displayName).foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func addressSection(_ status: GatewayStatusDto) -> some View {
        let base = ApiAddress.base(endpoint)
        Section {
            if status.portEditable {
                PortRow(model: model, port: status.port, readOnly: store.state.isReadOnly)
            } else {
                LabeledContent(L("apiServicePort")) {
                    Text(verbatim: String(ApiAddress.port(endpoint) ?? status.port)).monospacedDigit()
                }
            }
            if let base {
                AddressRow(title: L("apiServiceAddress"), url: base)
                AddressRow(title: L("apiServiceJsonrpc"), url: base + "/jsonrpc")
                AddressRow(title: L("apiServiceMcp"), url: base + "/mcp")
                AddressRow(title: L("apiServiceApi"), url: base + "/api/v1")
            }
        } header: {
            Text(L("settingsCatApiService"))
        } footer: {
            Text(L(status.portEditable ? "apiServicePortRestartHint" : "webApiBindFixed"))
        }

        if status.portEditable {
            Section {
                Toggle(isOn: Binding(
                    get: { model.pendingLan ?? status.lanEnabled },
                    set: { on in Task { await model.setLan(on) } }
                )) {
                    SettingsText(title: L("apiServiceLanEnable"), detail: L("apiServiceLanEnableDesc"))
                }
                .tint(Color.fdToggleOn)
                .disabled(store.state.isReadOnly)
            } footer: {
                Text(L("apiServiceLanRestartHint"))
            }
        }
    }

    private func featuresSection(_ status: GatewayStatusDto, readOnly: Bool) -> some View {
        Section {
            ForEach(GatewayFeature.allCases, id: \.self) { feature in
                VStack(alignment: .leading, spacing: 6) {
                    Toggle(isOn: Binding(
                        get: { model.pending[feature] ?? feature.isOn(status) },
                        set: { on in Task { await model.set(feature, on) } }
                    )) {
                        SettingsText(title: L(feature.titleKey), detail: L(feature.detailKey))
                    }
                    .tint(Color.fdToggleOn)
                    if feature == .cors, model.pending[.cors] ?? status.corsEnabled {
                        Label(L("apiServiceCorsAllowAllHelp"), systemImage: FluxSymbol.warning)
                            .font(.footnote)
                            .foregroundStyle(Color.fdStatusWarningText)
                    }
                }
            }
        } header: {
            Text(L("apiServiceFeaturesTitle"))
        } footer: {
            Text(L("apiServiceFeaturesDesc"))
        }
        .disabled(readOnly)
    }
}

// MARK: - 地址

/// 主机地址派生（`http(s)://host:port`；`ws(s)` 视同）。
enum ApiAddress {
    /// 主机 HTTP 根：`http(s)://host:port`（`ws(s)` 视同，路径 / 查询丢弃）。
    static func baseURL(_ endpoint: String) -> URL? {
        guard var components = URLComponents(string: endpoint.trimmingCharacters(in: .whitespacesAndNewlines)) else { return nil }
        switch components.scheme?.lowercased() {
        case "ws": components.scheme = "http"
        case "wss": components.scheme = "https"
        case "http", "https": break
        default: return nil
        }
        components.path = ""
        components.query = nil
        components.fragment = nil
        return components.url
    }

    static func base(_ endpoint: String) -> String? {
        baseURL(endpoint)?.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }

    static func port(_ endpoint: String) -> Int? {
        baseURL(endpoint)?.port
    }
}

private struct AddressRow: View {
    let title: String
    let url: String

    @Environment(AppContainer.self) private var container
    @State private var copied = false
    @State private var copyTick = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.subheadline).foregroundStyle(.secondary)
            HStack(spacing: 8) {
                Text(url)
                    .font(.fluxMono)
                    .textSelection(.enabled)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button {
                    ExtensionsClipboard.copy(url)
                    copied = true
                    copyTick += 1
                    container.toasts.show(text: L("apiServiceCopied"), tone: .success)
                    Task {
                        do {
                            try await Task.sleep(for: .seconds(2))
                        } catch {
                            return
                        }
                        copied = false
                    }
                } label: {
                    Image(systemName: copied ? FluxSymbol.done : FluxSymbol.copy)
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(.rect)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel(copied ? L("apiServiceCopied") : L("apiServiceCopy"))
                ShareLink(item: url) {
                    Image(systemName: FluxSymbol.share)
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(.rect)
                }
                .buttonStyle(.borderless)
            }
        }
        .sensoryFeedback(FluxHaptic.success.sensoryFeedback, trigger: copyTick)
        .accessibilityElement(children: .contain)
    }
}

// MARK: - 端口

/// 端口（仅 `portEditable`）：回车 / 失焦提交，1024…65535；无效输入还原并提示。
private struct PortRow: View {
    let model: ApiServiceModel
    let port: Int
    let readOnly: Bool

    @Environment(AppContainer.self) private var container
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var draft = ""
    @State private var warnTick = 0
    @FocusState private var focused: Bool

    var body: some View {
        LabeledContent(L("apiServicePort")) {
            HStack(spacing: 8) {
                TextField(L("apiServicePort"), text: $draft)
                    .keyboardType(.numberPad)
                    .multilineTextAlignment(.trailing)
                    .monospacedDigit()
                    .focused($focused)
                    .frame(minWidth: 72, maxWidth: typeSize.isAccessibilitySize ? .infinity : 110, minHeight: 44)
                    .disabled(readOnly || model.portBusy)
                    .onSubmit(commit)
                if model.portBusy { ProgressView() }
            }
        }
        .onAppear { draft = String(port) }
        .onChange(of: port) { _, new in if !focused { draft = String(new) } }
        .onChange(of: focused) { _, isFocused in if !isFocused { commit() } }
        .sensoryFeedback(FluxHaptic.warning.sensoryFeedback, trigger: warnTick)
    }

    private func commit() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard text != String(port) else { return }
        guard let value = Int(text), GatewayStatusDto.portRange.contains(value) else {
            draft = String(port)
            warnTick += 1
            container.toasts.show(text: L("apiServicePortInvalid"), tone: .error)
            return
        }
        Task { await model.setPort(value) }
    }
}

// MARK: - 令牌

private struct TokenSection: View {
    let model: ApiServiceModel
    let configured: Bool
    let readOnly: Bool
    @Environment(AppContainer.self) private var container
    @State private var confirmRegenerate = false
    @State private var draft = ""
    @State private var issue: String?
    @State private var copied = false
    @FocusState private var focused: Bool

    var body: some View {
        let revealed = model.token
        let busy = model.tokenBusy
        let dirty = revealed.map { draft != $0 } ?? false
        Section {
            if let revealed {
                TextField(L("apiServiceToken"), text: $draft, prompt: Text(L("proxyNotConfigured")))
                    .font(.fluxMono)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focused)
                    .submitLabel(.done)
                    .onSubmit { Task { await save() } }
                    .disabled(readOnly || busy)
                    .onAppear { draft = revealed }
                    .onChange(of: revealed) { _, new in draft = new }
                if let issue {
                    Label(issue, systemImage: FluxSymbol.failure)
                        .font(.footnote)
                        .foregroundStyle(Color.fdStatusFailedText)
                }
                if dirty {
                    Button {
                        Task { await save() }
                    } label: {
                        HStack {
                            Text(L("webSetupSubmit"))
                            if busy {
                                Spacer()
                                ProgressView()
                            }
                        }
                    }
                    .disabled(readOnly || busy)
                }
            } else {
                HStack {
                    Text(configured ? "••••••••••••" : L("proxyNotConfigured"))
                        .font(.fluxMono)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel(configured ? L("apiServiceToken") : L("proxyNotConfigured"))
                    Spacer()
                    if busy { ProgressView() }
                }
            }
            Button {
                Task { await toggleReveal() }
            } label: {
                Label(L(revealed == nil ? "webShowKey" : "webHideKey"), systemImage: revealed == nil ? "eye" : "eye.slash")
            }
            .disabled(busy || (revealed == nil && !configured))
            Button {
                Task { await copy() }
            } label: {
                Label(copied ? L("apiServiceCopied") : L("apiServiceCopy"), systemImage: copied ? FluxSymbol.done : FluxSymbol.copy)
            }
            .disabled(busy || (revealed == nil && !configured))
            Button {
                confirmRegenerate = true
            } label: {
                Label(L("apiServiceTokenGenerate"), systemImage: FluxSymbol.syncing)
            }
            .disabled(readOnly || busy)
            .alert(L("apiServiceTokenGenerate"), isPresented: $confirmRegenerate) {
                Button(L("apiServiceTokenGenerate")) { Task { await model.regenerate() } }
                Button(L("cancel"), role: .cancel) {}
            } message: {
                Text(L("apiServiceTokenDesc"))
            }
        } header: {
            Text(L("apiServiceToken"))
        } footer: {
            Text(L("apiServiceTokenDesc"))
        }
        .onChange(of: configured) { _, _ in
            // 令牌被改动（如开启管理 API 时主机自动生成）：已显示的明文重新读取。
            if model.token != nil { Task { await model.loadToken() } }
        }
        .onDisappear { model.hideToken() }
    }

    private func toggleReveal() async {
        if model.token != nil {
            model.hideToken()
            issue = nil
        } else {
            _ = await model.reveal()
        }
    }

    private func copy() async {
        if model.token == nil, !(await model.reveal()) { return }
        guard let token = model.token, !token.isEmpty else { return }
        ExtensionsClipboard.copy(token)
        container.toasts.show(text: L("apiServiceCopied"), tone: .success)
        copied = true
        do {
            try await Task.sleep(for: .seconds(2))
        } catch {
            return
        }
        copied = false
    }

    /// 自定义令牌须过访问密钥策略（可见 ASCII、8–128 位、含字母与数字）。
    private func save() async {
        let value = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard value != model.token else { return }
        if let problem = HostInput.validateAccessKey(value) {
            issue = Self.issueText(problem)
            return
        }
        issue = nil
        focused = false
        await model.saveToken(value)
    }

    private static func issueText(_ issue: KeyIssue) -> String {
        switch issue {
        case .badChars: L("webKeyBadChars")
        case .tooShort: L("webKeyTooShort", ["min": HostInput.accessKeyMinLength])
        case .tooLong: L("webKeyTooLong", ["max": HostInput.accessKeyMaxLength])
        case .needsMix: L("webKeyNeedsMix")
        }
    }
}
