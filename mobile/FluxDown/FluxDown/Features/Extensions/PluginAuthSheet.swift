import FluxDomain
import FluxUI
import SwiftUI
#if FULL_FLAVOR

/// S11.5 · 插件平台登录。流程与 GPUI `plugin_auth.rs` / Web `PluginAuthDialog` 一致：
/// 打开即 `status` → `begin` →（二维码挑战每 2s 自动 `poll`，仅前台；其它挑战手动「检查状态」）→ success；
/// 已登录可 `logout`；任何方式关闭都向引擎 `cancel` 当前会话。
@MainActor
@Observable
final class PluginAuthModel {
    nonisolated enum Action: String {
        case begin, poll, logout, status
    }

    private(set) var auth = PluginAuthState()
    private(set) var busy = true
    var site = ""
    var input = ""

    @ObservationIgnored private let identity: String
    @ObservationIgnored private let container: AppContainer
    @ObservationIgnored private var queriedSite = ""
    @ObservationIgnored private var alive = true
    @ObservationIgnored private var polling = false

    init(identity: String, container: AppContainer) {
        self.identity = identity
        self.container = container
    }

    var qrPolling: Bool { auth.sessionPending && auth.isQrChallenge }

    func start() async {
        alive = true
        busy = false
        await run(.status, site: "")
    }

    /// 站点失焦 / 回车：按新站点刷新登录状态（同一站点只查一次）。
    func querySite() async {
        guard !busy, site != queriedSite else { return }
        queriedSite = site
        await run(.status, site: site)
    }

    func run(_ action: Action, site siteOverride: String? = nil, notifySuccess: Bool = false) async {
        guard !busy else { return }
        // 轮询是后台动作：不置 `busy`（否则取消 / 输入框每 2s 闪一次禁用），仅防重入。
        let isPoll = action == .poll
        if isPoll {
            guard !polling else { return }
            polling = true
        } else {
            guard !polling else { return }
            busy = true
        }
        defer { if isPoll { polling = false } }
        if action != .poll { auth.message = nil }
        let request = PluginAuthRequest(
            identity: identity,
            action: action.rawValue,
            site: siteOverride ?? site,
            authRef: action == .status ? "" : auth.authRef,
            sessionId: action == .status ? "" : auth.sessionId,
            input: action == .status ? "" : input
        )
        do throws(HostError) {
            let response: PluginAuthResponse = try await container.session.call(HostMethod.daemonPluginAuth, params: request)
            guard alive else { return }
            auth = PluginAuth.apply(auth, response: response, wasLogout: action == .logout)
            if notifySuccess, response.status == "success" {
                container.toasts.show(text: L("pluginAuthSuccess"), tone: .success)
            }
        } catch {
            guard alive else { return }
            auth.status = "error"
            auth.message = L("pluginAuthFailed", ["message": ExtensionErrorText.describe(error)])
        }
        if alive, !isPoll { busy = false }
    }

    /// 关闭（取消按钮 / 下拉 / 返回）：向引擎释放悬空会话（fire-and-forget，与在途 poll 并存无害）。
    func close() {
        alive = false
        guard !auth.sessionId.isEmpty else { return }
        let request = PluginAuthRequest(identity: identity, action: "cancel", site: site, authRef: auth.authRef, sessionId: auth.sessionId)
        let session = container.session
        Task {
            do throws(HostError) {
                try await session.callVoid(HostMethod.daemonPluginAuth, params: request)
            } catch {
                // 会话已过期 / 主机已断开：引擎侧本就会回收悬空会话，无需打扰用户。
            }
        }
    }
}

struct PluginAuthSheet: View {
    let plugin: PluginDto

    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @State private var model: PluginAuthModel?
    @FocusState private var siteFocused: Bool

    var body: some View {
        NavigationStack {
            Group {
                if let model {
                    content(model)
                } else {
                    Color.clear
                }
            }
            .navigationTitle(L("pluginAuthDialogTitle", ["name": plugin.name]))
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.large])
        .task {
            let created = PluginAuthModel(identity: plugin.identity, container: container)
            model = created
            await created.start()
        }
        .onDisappear { model?.close() }
    }

    @ViewBuilder
    private func content(_ model: PluginAuthModel) -> some View {
        @Bindable var model = model
        let auth = model.auth
        Form {
            Section {
                TextField(L("pluginAuthSiteLabel"), text: $model.site, prompt: Text(L("pluginAuthSitePlaceholder")))
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($siteFocused)
                    .submitLabel(.done)
                    .onSubmit { Task { await model.querySite() } }
                    .disabled(model.busy)
                TextField(L("pluginAuthInputLabel"), text: $model.input, prompt: Text(L("pluginAuthInputPlaceholder")))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .disabled(model.busy)
            } header: {
                Text(L("pluginAuthDescription")).textCase(nil)
            }

            if auth.loggedIn {
                Section {
                    Label(L("pluginAuthSuccess"), systemImage: FluxSymbol.success)
                        .foregroundStyle(Color.fdStatusSeedingText)
                }
            } else if auth.sessionPending {
                Section {
                    HStack(spacing: 10) {
                        ProgressView()
                        Text(L("pluginAuthPending"))
                    }
                    .accessibilityElement(children: .combine)
                }
            }

            if let challenge = auth.challenge {
                PluginChallengeView(value: challenge, type: auth.challengeType ?? "")
            }

            if let message = auth.message {
                Section {
                    if auth.status == "error" {
                        Label(message, systemImage: FluxSymbol.failure)
                            .foregroundStyle(Color.fdStatusFailedText)
                    } else {
                        Text(message)
                    }
                }
            }
        }
        .scrollDismissesKeyboard(.interactively)
        .onChange(of: siteFocused) { _, focused in
            if !focused { Task { await model.querySite() } }
        }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button(L("cancel")) { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                primaryButton(model, auth)
            }
        }
        // 二维码挑战：前台每 2s 自动 poll；回到前台立即补一次。
        .task(id: model.qrPolling) {
            guard model.qrPolling else { return }
            while !Task.isCancelled {
                do {
                    try await Task.sleep(for: PluginAuth.pollInterval)
                } catch {
                    return
                }
                if scenePhase == .active { await model.run(.poll, notifySuccess: true) }
            }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active, model.qrPolling { Task { await model.run(.poll, notifySuccess: true) } }
        }
    }

    @ViewBuilder
    private func primaryButton(_ model: PluginAuthModel, _ auth: PluginAuthState) -> some View {
        if model.busy {
            ProgressView().accessibilityLabel(L("pluginProcessing"))
        } else if auth.loggedIn {
            Button(L("pluginAuthLogout"), role: .destructive) { Task { await model.run(.logout) } }
        } else if auth.sessionPending, !auth.isQrChallenge {
            // 非二维码挑战没有自动轮询，提供手动「检查状态」。
            Button(L("pluginAuthPoll")) { Task { await model.run(.poll, notifySuccess: true) } }
                .fontWeight(.semibold)
        } else if !auth.sessionPending {
            Button(L("pluginAuthBegin")) { Task { await model.run(.begin, notifySuccess: true) } }
                .fontWeight(.semibold)
        }
    }
}

/// 挑战渲染：只使用安全的 `data:image` 或本地编码的二维码，绝不把挑战 URL 当图片请求；
/// 其余退化为截断文本 + 复制。
private struct PluginChallengeView: View {
    let value: String
    let type: String

    @Environment(AppContainer.self) private var container
    @State private var image: UIImage?

    var body: some View {
        Section {
        VStack(alignment: .leading, spacing: 12) {
            if !type.isEmpty {
                Text(type).font(.footnote).foregroundStyle(.secondary)
            }
            if let image {
                Image(uiImage: image)
                    .interpolation(.none)
                    .resizable()
                    .scaledToFit()
                    .frame(width: 240, height: 240)
                    .padding(16)
                    .background(.white, in: .rect(cornerRadius: 20, style: .continuous))
                    .frame(maxWidth: .infinity)
                    .accessibilityLabel(type.isEmpty ? L("pluginAuthQr") : type)
                    .accessibilityAddTraits(.isImage)
            } else {
                Text(PluginAuth.truncate(value))
                    .font(.footnote)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        }
        Section { actions }
            .task(id: value) { image = renderImage() }
    }

    @ViewBuilder
    private var actions: some View {
        Button {
            ExtensionsClipboard.copy(value)
            container.toasts.show(text: L("apiServiceCopied"), tone: .success)
        } label: {
            Label(L("apiServiceCopy"), systemImage: FluxSymbol.copy)
        }
        if let image {
            ShareLink(
                item: Image(uiImage: image),
                preview: SharePreview(type.isEmpty ? L("pluginAuthQr") : type, image: Image(uiImage: image))
            ) {
                Label(L("mobileNotifActionShare"), systemImage: FluxSymbol.share)
            }
        }
        if let url = PluginAuth.safeHTTPURL(value) {
            Link(destination: url) {
                Label(L("mobilePluginAuthOpenLink"), systemImage: FluxSymbol.openFile)
            }
        }
    }

    private func renderImage() -> UIImage? {
        if let bytes = PluginAuth.dataImageBytes(value) { return UIImage(data: bytes) }
        // data: 载荷无法解码为图片时不当二维码文本编码（避免把整段 base64 画成二维码）。
        if value.lowercased().hasPrefix("data:") { return nil }
        guard PluginAuth.isQrcode(type) else { return nil }
        return QRCodeImage.make(value)
    }
}
#endif
