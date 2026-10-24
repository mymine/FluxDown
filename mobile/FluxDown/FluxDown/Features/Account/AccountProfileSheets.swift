import FluxDomain
import FluxUI
import SwiftUI

// S2 资料 / 安全编辑 sheet：昵称、Origin ID（一次性）、修改邮箱（两步验证码）、修改 / 设置密码。

// MARK: - 昵称

struct NicknameSheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    let current: String

    @State private var value: String
    @State private var busy = false
    @State private var errorText: String?
    @State private var successCount = 0

    init(current: String) {
        self.current = current
        _value = State(initialValue: current)
    }

    private var valid: Bool { AccountRules.isValidNickname(value) }
    private var unchanged: Bool { AccountRules.trimmed(value) == AccountRules.trimmed(current) }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(L("accountFieldNickname"), text: $value)
                        .textContentType(.nickname)
                        .submitLabel(.done)
                        .disabled(busy)
                } footer: {
                    if !valid, !value.isEmpty {
                        AccountErrorLabel(text: L("accountNicknameEditInvalid"))
                    } else if let errorText {
                        AccountErrorLabel(text: errorText)
                    }
                }
            }
            .accountSheetToolbar(
                title: L("accountNicknameEditTitle"),
                confirmTitle: L("confirm"),
                canConfirm: valid && !unchanged,
                busy: busy,
                onCancel: { dismiss() },
                onConfirm: save
            )
            .onSubmit { if valid, !unchanged { save() } }
            .onChange(of: value) { errorText = nil }
        }
        .presentationDetents([.medium])
        .sensoryFeedback(.success, trigger: successCount)
    }

    private func save() {
        guard !busy, valid, !unchanged else { return }
        let nickname = AccountRules.trimmed(value)
        busy = true
        errorText = nil
        Task {
            defer { busy = false }
            do throws(HostError) {
                try await container.agent.changeNickname(nickname)
                successCount += 1
                container.toasts.show(text: L("accountNicknameEditSuccess"), tone: .success)
                dismiss()
            } catch {
                errorText = AccountText.error(error)
            }
        }
    }
}

// MARK: - Origin ID

struct OriginIdSheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @State private var editor: OriginIdEditor
    @State private var sections = AgentSections()

    init(current: Int64?) {
        _editor = State(initialValue: OriginIdEditor(current: current))
    }

    private var allowed: Bool {
        guard let session = sections.session(container.store.state) else { return false }
        return editor.acceptsSession(session)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    HStack(spacing: 8) {
                        TextField(L("accountOriginIdEditPlaceholder"), text: Binding(get: { editor.value }, set: { editor.setValue($0) }))
                            .keyboardType(.numberPad)
                            .disabled(editor.busy)
                        Button(L("accountOriginIdEditRoll")) {
                            Task { await editor.roll(container.agent) }
                        }
                        .buttonStyle(.borderless)
                        .disabled(editor.busy)
                    }
                    availability
                } header: {
                    Text(L("accountOriginIdEditDesc")).textCase(nil)
                }
                Section {
                    Toggle(isOn: Binding(get: { editor.confirmed }, set: { editor.setConfirmed($0) })) {
                        Text(L("accountOriginIdEditWarning"))
                            .foregroundStyle(Color.fdStatusWarningText)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .disabled(editor.busy)
                } footer: {
                    if let text = editor.errorText { AccountErrorLabel(text: text) }
                }
            }
            .accountSheetToolbar(
                title: L("accountOriginIdEditTitle"),
                confirmTitle: L("accountOriginIdEditConfirm"),
                canConfirm: editor.canSubmit,
                busy: editor.busy,
                onCancel: { dismiss() },
                onConfirm: save
            )
            .task(id: editor.value) {
                // 防抖：停止输入 400ms 后再检查可用性。
                do {
                    try await Task.sleep(for: .milliseconds(400))
                } catch {
                    return
                }
                await editor.runCheck(container.agent)
            }
            .onChange(of: allowed) { _, now in
                if !now {
                    editor.invalidate()
                    dismiss()
                }
            }
            .onDisappear { editor.invalidate() }
        }
        .presentationDetents([.medium, .large])
        .sensoryFeedback(.success, trigger: editor.successCount)
    }

    @ViewBuilder
    private var availability: some View {
        switch editor.check {
        case .idle:
            EmptyView()
        case .checking:
            HStack(spacing: 8) {
                ProgressView()
                Text(L("mobileOriginIdChecking")).font(.footnote).foregroundStyle(.secondary)
            }
        case .available:
            if let parsed = editor.parsed {
                Label("#\(parsed)", systemImage: FluxSymbol.success)
                    .font(.footnote.weight(.medium))
                    .foregroundStyle(Color.fdStatusSeedingText)
            }
        case .taken:
            AccountErrorLabel(text: L("accountOriginIdErrorTaken"))
        case .invalid:
            AccountErrorLabel(text: L("accountOriginIdInvalid"))
        }
    }

    private func save() {
        Task {
            if await editor.submit(container.agent) {
                container.toasts.show(text: L("accountOriginIdEditSuccess"), tone: .success)
                dismiss()
            }
        }
    }
}

// MARK: - 修改邮箱

struct EmailChangeSheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @State private var flow: EmailChangeFlow

    init(userId: String, currentEmail: String) {
        _flow = State(initialValue: EmailChangeFlow(userId: userId, currentEmail: currentEmail))
    }

    var body: some View {
        NavigationStack {
            Form {
                switch flow.step {
                case .old: oldStep
                case .new: newStep
                }
            }
            .navigationTitle(L("accountEmailChangeTitle"))
            .navigationBarTitleDisplayMode(.inline)
            .interactiveDismissDisabled(flow.submitting)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    if flow.step == .new {
                        Button(L("back")) { flow.back() }.disabled(flow.submitting)
                    } else {
                        Button(L("cancel")) { dismiss() }
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if flow.submitting {
                        ProgressView()
                    } else if flow.step == .old {
                        Button(L("accountEmailChangeSendNewCode")) { advance() }.disabled(!flow.canNext)
                    } else {
                        Button(L("confirm")) { confirm() }.disabled(!flow.canConfirm)
                    }
                }
            }
            .task { await flow.start(container.agent) }
        }
        .presentationDetents([.large])
        .sensoryFeedback(.success, trigger: flow.successCount)
    }

    @ViewBuilder
    private var oldStep: some View {
        Section {
            VerificationCodeRows(
                code: $flow.oldCode,
                clock: flow.oldClock,
                sending: flow.sending,
                locked: flow.submitting,
                fieldTitle: L("accountEmailChangeOldCodePlaceholder"),
                onSend: { Task { await flow.sendOldCode(container.agent) } }
            )
        } header: {
            Text(flow.oldClock != nil ? L("accountEmailChangeOldSubtitle", ["email": flow.currentEmail]) : L("accountEmailChangeOldCodeHint"))
                .textCase(nil)
        } footer: {
            Text(L("accountEmailChangeOldCodeHint"))
        }
        Section {
            TextField(L("accountEmailChangeNewPlaceholder"), text: $flow.newEmail)
                .textContentType(.emailAddress)
                .keyboardType(.emailAddress)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .disabled(flow.submitting)
        } footer: {
            VStack(alignment: .leading, spacing: 6) {
                if let error = flow.emailError { AccountErrorLabel(text: error) }
                if let text = flow.errorText { AccountErrorLabel(text: text) }
            }
        }
    }

    @ViewBuilder
    private var newStep: some View {
        Section {
            VerificationCodeRows(
                code: $flow.newCode,
                clock: flow.activeNewClock,
                sending: flow.sending,
                locked: flow.submitting,
                canSend: flow.canNext,
                onSend: { Task { await flow.sendNewCode(container.agent, locking: false) } }
            )
        } header: {
            Text(L("accountEmailChangeCodeSubtitle", ["email": flow.sentEmail ?? flow.email])).textCase(nil)
        } footer: {
            if let text = flow.errorText { AccountErrorLabel(text: text) }
        }
    }

    private func advance() {
        Task { await flow.advance(container.agent) }
    }

    private func confirm() {
        Task {
            if await flow.confirm(container.agent) {
                container.toasts.show(text: L("accountEmailChangeSuccess"), tone: .success)
                dismiss()
            }
        }
    }
}

// MARK: - 修改 / 设置密码

struct PasswordChangeSheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @State private var flow: PasswordChangeFlow

    init(userId: String, email: String, hasPassword: Bool?) {
        _flow = State(initialValue: PasswordChangeFlow(userId: userId, email: email, hasPassword: hasPassword))
    }

    var body: some View {
        NavigationStack {
            Form {
                if flow.byCode {
                    Section {
                        VerificationCodeRows(code: $flow.code, clock: flow.clock, sending: flow.sending, locked: flow.submitting, onSend: sendCode)
                    } header: {
                        Text(
                            flow.clock != nil
                                ? L("accountPasswordCodeSubtitle", ["email": flow.email])
                                : L("accountPasswordCodeHint", ["email": flow.email])
                        )
                        .textCase(nil)
                    }
                } else {
                    Section {
                        AccountPasswordField(title: L("accountPasswordCurrentPlaceholder"), text: $flow.current, contentType: .password, locked: flow.submitting)
                    }
                }
                Section {
                    AccountPasswordField(title: L("accountPasswordNewPlaceholder"), text: $flow.newPassword, contentType: .newPassword, locked: flow.submitting)
                    AccountPasswordField(title: L("accountPasswordConfirmPlaceholder"), text: $flow.confirm, contentType: .newPassword, locked: flow.submitting)
                } footer: {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(L("accountPasswordHint"))
                        if let key = flow.visibleRuleKey { AccountErrorLabel(text: L(key)) }
                        if let text = flow.errorText { AccountErrorLabel(text: text) }
                    }
                }
                modeSwitch
            }
            .accountSheetToolbar(
                title: flow.canUsePassword ? L("accountPasswordChangeTitle") : L("accountPasswordSetTitle"),
                confirmTitle: L("confirm"),
                canConfirm: flow.canConfirm,
                busy: flow.submitting,
                onCancel: { dismiss() },
                onConfirm: confirm
            )
            .onAppear { flow.restoreIfNeeded() }
        }
        .presentationDetents([.large])
        .sensoryFeedback(.success, trigger: flow.successCount)
    }

    /// 切换验证方式：已设置密码的账号可在「当前密码」与「邮箱验证码」之间切换；未设置密码只有验证码。
    @ViewBuilder
    private var modeSwitch: some View {
        if flow.canUsePassword {
            Section {
                if flow.byCode {
                    Button(L("accountPasswordUseCurrentPassword")) { flow.setMode(.password) }
                } else {
                    Button(L("accountPasswordUseEmailCode")) { flow.setMode(.code) }
                }
            }
            .disabled(flow.submitting)
        }
    }

    private func sendCode() {
        Task { await flow.sendCode(container.agent) }
    }

    private func confirm() {
        Task {
            if await flow.submit(container.agent) {
                container.toasts.show(text: L("accountPasswordChangeSuccess"), tone: .success)
                dismiss()
            }
        }
    }
}

// MARK: - 注销账户

struct DeleteAccountSheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @State private var flow: DeleteAccountFlow
    @State private var confirming = false

    init(userId: String, email: String, hasPassword: Bool?) {
        _flow = State(initialValue: DeleteAccountFlow(userId: userId, email: email, hasPassword: hasPassword))
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text(L("accountDeleteExplain"))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if flow.byCode {
                    Section {
                        VerificationCodeRows(code: $flow.code, clock: flow.clock, sending: flow.sending, locked: flow.submitting, onSend: sendCode)
                    } header: {
                        Text(
                            flow.clock != nil
                                ? L("accountDeleteCodeSubtitle", ["email": flow.email])
                                : L("accountDeleteCodeHint", ["email": flow.email])
                        )
                        .textCase(nil)
                    } footer: {
                        if let text = flow.errorText { AccountErrorLabel(text: text) }
                    }
                } else {
                    Section {
                        AccountPasswordField(title: L("accountPasswordCurrentPlaceholder"), text: $flow.current, contentType: .password, locked: flow.submitting)
                    } footer: {
                        if let text = flow.errorText { AccountErrorLabel(text: text) }
                    }
                }
                modeSwitch
            }
            .accountSheetToolbar(
                title: L("accountDeleteTitle"),
                confirmTitle: L("accountDeleteConfirm"),
                canConfirm: flow.canConfirm,
                busy: flow.submitting,
                onCancel: { dismiss() },
                onConfirm: { confirming = true }
            )
            .alert(L("accountDeleteAlertTitle"), isPresented: $confirming) {
                Button(L("accountDeleteConfirm"), role: .destructive, action: confirm)
                Button(L("cancel"), role: .cancel) {}
            } message: {
                Text(L("accountDeleteAlertMessage"))
            }
            .onAppear { flow.restoreIfNeeded() }
        }
        .presentationDetents([.large])
    }

    @ViewBuilder
    private var modeSwitch: some View {
        if flow.canUsePassword {
            Section {
                if flow.byCode {
                    Button(L("accountPasswordUseCurrentPassword")) { flow.setMode(.password) }
                } else {
                    Button(L("accountPasswordUseEmailCode")) { flow.setMode(.code) }
                }
            }
            .disabled(flow.submitting)
        }
    }

    private func sendCode() {
        Task { await flow.sendCode(container.agent) }
    }

    private func confirm() {
        Task {
            if await flow.submit(container.agent) {
                container.toasts.show(text: L("accountDeleteSuccess"), tone: .success)
                dismiss()
            }
        }
    }
}
