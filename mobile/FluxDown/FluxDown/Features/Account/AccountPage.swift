import FluxDomain
import FluxUI
import SwiftUI
import UIKit

/// S2 · 账户（设置 → 账户）。随登录态呈现：未登录 Hero → 登录 / 注册 sheet；已登录资料卡 → 账号与安全 → 云功能。
/// 会话 / 同步 / 云连接状态全部来自 `agent.session` / `agent.sync` / `agent.cloudConnection` 分区（无本地副本）。
/// 断连只读：所有写操作置灰，顶部显示 `localServiceDisconnected`。**没有套餐购买 / 订单 / 推荐入口**（App Store 规则）；
/// 套餐徽标只读显示。
struct AccountPage: View {
    @Environment(AppContainer.self) private var container

    @State private var sections = AgentSections()
    @State private var sheet: AccountSheet?
    @State private var refreshing = false
    @State private var confirmingLogout = false
    @State private var loggingOut = false
    @State private var errorText: String?
    @State private var refreshCount = 0
    @State private var copyCount = 0

    @ScaledMetric(relativeTo: .largeTitle) private var heroSize: CGFloat = 88
    @ScaledMetric(relativeTo: .title) private var avatarSize: CGFloat = 64

    var body: some View {
        let state = container.store.state
        let session = sections.session(state)
        let readOnly = state.isReadOnly
        SettingsPage(title: L("settingsCatAccount"), showsReadOnlyBanner: true) {
            if let session {
                profileSection(session, readOnly: readOnly)
                securitySection(session, readOnly: readOnly)
            } else {
                heroSection(readOnly: readOnly)
            }
            AccountCloudFeatures(sections: sections, session: session)
            AccountEndpointSection(readOnly: readOnly)
            if let session {
                logoutSection(readOnly: readOnly)
                deleteAccountSection(session: session, readOnly: readOnly)
            }
        }
        .overlay {
            // 主机不支持账户能力（`agent.auth`）：整页不可用（设置首页本就隐藏入口，这里兜住从通知等处直达的情况）。
            if state.info != nil, !state.has(HostCapability.agentAuth) {
                ContentUnavailableView(
                    L("settingsCatAccount"),
                    systemImage: FluxSymbol.cloud,
                    description: Text(L("settingsUnsupportedOnPlatform"))
                )
                .background(Color(uiColor: .systemGroupedBackground))
            }
        }
        .toolbar {
            if session != nil {
                ToolbarItem(placement: .primaryAction) {
                    if refreshing {
                        ProgressView()
                    } else {
                        Button(L("accountCloudRefresh"), systemImage: FluxSymbol.retry, action: refresh)
                            .disabled(readOnly)
                    }
                }
            }
        }
        .sheet(item: $sheet) { item in
            switch item {
            case .login: LoginSheet()
            case .register: RegisterSheet()
            case let .nickname(current): NicknameSheet(current: current)
            case let .originId(current): OriginIdSheet(current: current)
            case let .email(userId, email): EmailChangeSheet(userId: userId, currentEmail: email)
            case let .password(userId, email, hasPassword): PasswordChangeSheet(userId: userId, email: email, hasPassword: hasPassword)
            case let .deleteAccount(userId, email, hasPassword): DeleteAccountSheet(userId: userId, email: email, hasPassword: hasPassword)
            }
        }
        .onChange(of: session == nil) { _, loggedOut in
            // 会话没了（退出 / 被撤销）：依赖会话的编辑 sheet 随之关闭。
            if loggedOut, sheet?.requiresSession == true { sheet = nil }
            errorText = nil
        }
        .sensoryFeedback(.success, trigger: refreshCount)
        .sensoryFeedback(.success, trigger: copyCount)
    }

    // MARK: 未登录 Hero

    private func heroSection(readOnly: Bool) -> some View {
        Group {
            Section {
                VStack(spacing: 12) {
                    Image(systemName: "person.crop.circle")
                        .font(.system(size: min(heroSize, 160) * 0.5))
                        .foregroundStyle(.tint)
                        .frame(width: min(heroSize, 160), height: min(heroSize, 160))
                        .background(Color.accentColor.opacity(0.12), in: .circle)
                        .accessibilityHidden(true)
                    Text(L("accountLoginDialogTitle"))
                        .font(.title3.weight(.semibold))
                        .multilineTextAlignment(.center)
                    Text(L("accountHeroSubtitle"))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 8)
            }
            .listRowBackground(Color.clear)

            Section {
                Button {
                    sheet = .login
                } label: {
                    Label(L("accountLogin"), systemImage: "person.crop.circle.badge.checkmark")
                }
                Button {
                    sheet = .register
                } label: {
                    Label(L("accountRegister"), systemImage: "person.crop.circle.badge.plus")
                }
            }
            .disabled(readOnly)

            Section {
                featureRow(symbol: FluxSymbol.syncing, title: L("accountFeatureConfigSync"), detail: L("accountFeatureConfigSyncDesc"))
                featureRow(symbol: "network", title: L("accountFeatureMultiDevice"), detail: L("accountFeatureMultiDeviceDesc"))
            }
        }
    }

    private func featureRow(symbol: String, title: String, detail: String) -> some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                Text(detail)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } icon: {
            Image(systemName: symbol).foregroundStyle(.tint)
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: 资料卡

    private func profileSection(_ session: AgentSessionDto, readOnly: Bool) -> some View {
        let user = session.user
        return Section {
            profileHeader(session)
            Button {
                sheet = .nickname(current: user.nickname)
            } label: {
                disclosureLabel(L("accountNicknameEditTitle"), symbol: FluxSymbol.edit)
            }
            .disabled(readOnly)
            if AccountRules.canEditOriginId(session) {
                Button {
                    sheet = .originId(current: user.originId)
                } label: {
                    disclosureLabel(L("accountOriginIdEditTitle"), symbol: "number")
                }
                .disabled(readOnly)
            }
        } footer: {
            if let errorText { AccountErrorLabel(text: errorText) }
        }
    }

    private func profileHeader(_ session: AgentSessionDto) -> some View {
        let user = session.user
        let side = min(avatarSize, 112)
        return HStack(alignment: .center, spacing: 16) {
            Text(AccountRules.avatarInitial(user) ?? "")
                .font(.system(size: side * 0.42, weight: .semibold, design: .rounded))
                .foregroundStyle(.tint)
                .frame(width: side, height: side)
                .background(Color.accentColor.opacity(0.12), in: .circle)
                .overlay {
                    if AccountRules.avatarInitial(user) == nil {
                        Image(systemName: "person.fill").font(.system(size: side * 0.4)).foregroundStyle(.tint)
                    }
                }
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 8) {
                        nameText(user)
                        planBadge(session)
                    }
                    VStack(alignment: .leading, spacing: 4) {
                        nameText(user)
                        planBadge(session)
                    }
                }
                originIdChip(user)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 4)
    }

    private func nameText(_ user: CloudUser) -> some View {
        Text(AccountRules.displayName(user))
            .font(.title3.weight(.semibold))
            .lineLimit(2)
            .truncationMode(.tail)
    }

    @ViewBuilder
    private func planBadge(_ session: AgentSessionDto) -> some View {
        if let plan = session.currentPlan {
            PlanBadgeView(plan: plan, ordinal: session.user.membershipOrdinal)
        }
    }

    /// Origin ID 胶囊：等宽数字，点按复制；无 ID 时灰色 `#—` 不可点。
    @ViewBuilder
    private func originIdChip(_ user: CloudUser) -> some View {
        if let originId = user.originId {
            Button {
                UIPasteboard.general.string = String(originId)
                copyCount += 1
                container.toasts.show(text: L("accountOriginIdCopied"), tone: .success)
            } label: {
                HStack(spacing: 4) {
                    Text(verbatim: "#\(originId)")
                        .font(.system(.footnote, design: .monospaced).weight(.medium))
                    Image(systemName: FluxSymbol.copy).imageScale(.small)
                }
                .foregroundStyle(.tint)
                .padding(.horizontal, 10)
                .frame(minHeight: 28)
                .background(Color.accentColor.opacity(0.12), in: .capsule)
            }
            .buttonStyle(.borderless)
            .accessibilityLabel(L("accountOriginIdCopied"))
            .accessibilityValue(String(originId))
        } else {
            Text(verbatim: "#—")
                .font(.system(.footnote, design: .monospaced).weight(.medium))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10)
                .frame(minHeight: 28)
                .background(Color(uiColor: .tertiarySystemFill), in: .capsule)
        }
    }

    // MARK: 账号与安全

    private func securitySection(_ session: AgentSessionDto, readOnly: Bool) -> some View {
        let user = session.user
        let hasPassword = user.hasPassword != false
        return Section {
            Button {
                sheet = .email(userId: user.id, email: user.email)
            } label: {
                valueRow(title: L("accountEmailPlaceholder"), detail: user.email)
            }
            .disabled(readOnly)
            .accessibilityHint(L("accountEmailChangeTitle"))
            Button {
                sheet = .password(userId: user.id, email: user.email, hasPassword: user.hasPassword)
            } label: {
                valueRow(
                    title: L("accountPasswordTitle"),
                    detail: hasPassword ? L("accountPasswordStatusSet") : L("accountPasswordStatusNotSet")
                )
            }
            .disabled(readOnly)
            .accessibilityHint(hasPassword ? L("accountPasswordChangeTitle") : L("accountPasswordSetTitle"))
        } header: {
            Text(L("accountSecurityGroup"))
        } footer: {
            Text(L("accountSecurityGroupDesc"))
        }
    }

    private func disclosureLabel(_ title: String, symbol: String) -> some View {
        HStack(spacing: 12) {
            Label(title, systemImage: symbol).foregroundStyle(.primary)
            Spacer(minLength: 8)
            chevron
        }
        .contentShape(.rect)
    }

    private func valueRow(title: String, detail: String) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).foregroundStyle(.primary)
                Text(detail)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            Spacer(minLength: 8)
            chevron
        }
        .contentShape(.rect)
    }

    private var chevron: some View {
        Image(systemName: "chevron.right")
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.tertiary)
            .accessibilityHidden(true)
    }

    // MARK: 退出登录

    private func logoutSection(readOnly: Bool) -> some View {
        Section {
            Button(role: .destructive) {
                confirmingLogout = true
            } label: {
                HStack(spacing: 8) {
                    Text(L("accountLogout"))
                    Spacer(minLength: 8)
                    if loggingOut { ProgressView() }
                }
            }
            .disabled(readOnly || loggingOut)
            // 退出登录确认 alert 由触发按钮挂载。
            .alert(L("accountLogoutConfirmTitle"), isPresented: $confirmingLogout) {
                Button(L("accountLogout"), role: .destructive, action: logout)
                Button(L("cancel"), role: .cancel) {}
            } message: {
                Text(L("accountLogoutConfirmMessage"))
            }
        }
    }

    // MARK: 注销账户

    private func deleteAccountSection(session: AgentSessionDto, readOnly: Bool) -> some View {
        Section {
            Button(role: .destructive) {
                sheet = .deleteAccount(userId: session.user.id, email: session.user.email, hasPassword: session.user.hasPassword)
            } label: {
                Text(L("accountDeleteRow"))
            }
            .disabled(readOnly)
        } footer: {
            Text(L("accountDeleteRowFooter"))
        }
    }

    // MARK: 动作

    private func refresh() {
        guard !refreshing else { return }
        refreshing = true
        errorText = nil
        let api = container.agent
        Task {
            defer { refreshing = false }
            do throws(HostError) {
                try await api.refreshProfile()
                _ = try await api.deviceList()
                refreshCount += 1
                container.toasts.show(text: L("accountCloudRefreshDone"), tone: .success)
            } catch {
                let text = AccountText.error(error)
                errorText = text
                container.toasts.show(text: text, tone: .error)
            }
        }
    }

    private func logout() {
        guard !loggingOut else { return }
        loggingOut = true
        errorText = nil
        let api = container.agent
        Task {
            defer { loggingOut = false }
            do throws(HostError) {
                try await api.logout()
            } catch {
                // 本机会话已清除（云端吊销失败）时不再显示错误：用户已经是退出状态。
                if sections.session(container.store.state) != nil {
                    errorText = AccountText.error(error)
                }
            }
        }
    }
}

// MARK: - Sheet 路由

private nonisolated enum AccountSheet: Identifiable {
    case login, register
    case nickname(current: String)
    case originId(current: Int64?)
    case email(userId: String, email: String)
    case password(userId: String, email: String, hasPassword: Bool?)
    case deleteAccount(userId: String, email: String, hasPassword: Bool?)

    var id: String {
        switch self {
        case .login: "login"
        case .register: "register"
        case .nickname: "nickname"
        case .originId: "originId"
        case .email: "email"
        case .password: "password"
        case .deleteAccount: "deleteAccount"
        }
    }

    /// 编辑类 sheet 依赖当前会话。
    var requiresSession: Bool {
        switch self {
        case .login, .register: false
        default: true
        }
    }
}

// MARK: - 服务地址（仅调试 / TestFlight 构建）

/// S2.8：FluxCloud 服务地址。正式构建 `editable == false`，整段不出现；可编辑时改地址立即生效。
private struct AccountEndpointSection: View {
    @Environment(AppContainer.self) private var container
    let readOnly: Bool

    @State private var endpoint: CloudEndpointDto?
    @State private var draft = ""
    @State private var busy = false
    @State private var errorText: String?

    var body: some View {
        Group {
            if let endpoint, endpoint.editable {
                Section {
                    TextField(endpoint.defaultBaseUrl, text: $draft)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.done)
                        .disabled(readOnly || busy)
                        .onSubmit { save(draft) }
                    Button(L("accountServerAddressReset")) { save("") }
                        .disabled(readOnly || busy || endpoint.baseUrl == endpoint.defaultBaseUrl)
                } header: {
                    Text(L("accountServerAddress"))
                } footer: {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(L("accountServerAddressDesc"))
                        if let errorText { AccountErrorLabel(text: errorText) }
                    }
                }
            }
        }
        .task(id: readOnly) { await load() }
    }

    private func load() async {
        guard !readOnly, container.store.state.has(HostCapability.agentAuth) else { return }
        do throws(HostError) {
            let value = try await container.agent.cloudEndpoint()
            endpoint = value
            draft = value.baseUrl
        } catch {
            // 旧主机没有该方法 / 暂时不可用：保持隐藏，不影响其它账户功能。
            endpoint = nil
        }
    }

    private func save(_ value: String) {
        let text = AccountRules.trimmed(value)
        guard !busy else { return }
        busy = true
        errorText = nil
        let api = container.agent
        Task {
            defer { busy = false }
            do throws(HostError) {
                try await api.setCloudEndpoint(baseUrl: text)
                let updated = try await api.cloudEndpoint()
                endpoint = updated
                draft = updated.baseUrl
                container.toasts.show(text: L("accountServerAddressSaved"), tone: .success)
            } catch {
                errorText = error.code == .invalidArgument ? L("accountServerAddressInvalid") : AccountText.error(error)
            }
        }
    }
}
