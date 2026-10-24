import FluxDomain
import Foundation
import Observation

// 资料 / 安全编辑的状态机：Origin ID（一次性）、修改邮箱（原邮箱 → 新邮箱两步验证码）、修改 / 设置密码。
// 镜像 Web `account/profileEditing.ts::OriginIdEditor`、`SecurityCard.tsx::EmailChangeDialog`、
// `PasswordDialogs.tsx::ChangePasswordDialog`；纯规则在 `AccountRules`。

private func trimmed(_ value: String) -> String { AccountRules.trimmed(value) }

// MARK: - Origin ID

/// 一个 `revision` 拥有全部异步结果（含随机建议与保存）：任何输入变化都会让在途的检查 / 随机结果作废。
@MainActor
@Observable
final class OriginIdEditor {
    nonisolated enum Check: Sendable, Hashable { case idle, checking, available, taken, invalid }

    let current: Int64?

    private(set) var value = ""
    private(set) var check: Check = .idle
    private(set) var busy = false
    private(set) var errorText: String?
    private(set) var confirmed = false
    private(set) var successCount = 0
    private var revision = 0
    private var submittingId: Int64?

    init(current: Int64?) { self.current = current }

    var parsed: Int64? { AccountRules.parseOriginId(value) }

    var canSubmit: Bool {
        !busy && confirmed && check == .available && parsed != nil && parsed != current
    }

    /// 界面离开 / 会话变化时让所有在途结果作废。
    func invalidate() { revision += 1 }

    func setValue(_ newValue: String) {
        guard !busy else { return }
        revision += 1
        value = newValue
        confirmed = false
        errorText = nil
        let parsed = AccountRules.parseOriginId(newValue)
        if AccountRules.trimmed(newValue).isEmpty || parsed == current {
            check = .idle
        } else {
            check = parsed == nil ? .invalid : .checking
        }
    }

    func setConfirmed(_ newValue: Bool) {
        if !busy { confirmed = newValue }
    }

    /// 保存成功后会话里 `originIdChanged` 先于结果到达：只有「正在提交的那个 id」才允许会话已不可改。
    func acceptsSession(_ session: AgentSessionDto) -> Bool {
        guard session.entitlements["originIdEdit"]?.boolValue == true else { return false }
        if !session.user.originIdChanged { return true }
        return submittingId != nil && submittingId == session.user.originId
    }

    /// 防抖后由视图调用：只检查仍处于 `.checking` 的当前值。
    func runCheck(_ api: AgentAPI) async {
        guard !busy, check == .checking, let parsed, parsed != current else { return }
        revision += 1
        let mine = revision
        errorText = nil
        do throws(HostError) {
            let result = try await api.checkOriginId(parsed)
            guard mine == revision else { return }
            check = result.available ? .available : (result.reason == "invalid" ? .invalid : .taken)
        } catch {
            guard mine == revision else { return }
            check = .idle
            errorText = AccountText.error(error)
        }
    }

    func roll(_ api: AgentAPI) async {
        guard !busy else { return }
        revision += 1
        let mine = revision
        busy = true
        confirmed = false
        check = .idle
        errorText = nil
        do throws(HostError) {
            let random = try await api.randomOriginId()
            guard mine == revision else { return }
            busy = false
            setValue(String(random.originId))
        } catch {
            guard mine == revision else { return }
            busy = false
            errorText = AccountText.error(error)
        }
    }

    /// - Returns: 保存成功。
    func submit(_ api: AgentAPI) async -> Bool {
        guard canSubmit, let parsed else { return false }
        revision += 1
        let mine = revision
        busy = true
        submittingId = parsed
        errorText = nil
        do throws(HostError) {
            try await api.changeOriginId(parsed)
            guard mine == revision else { return false }
            successCount += 1
            return true
        } catch {
            guard mine == revision else { return false }
            busy = false
            submittingId = nil
            check = .idle
            confirmed = false
            errorText = AccountText.error(error)
            return false
        }
    }
}

// MARK: - 修改邮箱

@MainActor
@Observable
final class EmailChangeFlow {
    nonisolated enum Step: Sendable, Hashable { case old, new }

    let userId: String
    let currentEmail: String

    var oldCode = ""
    var newEmail = ""
    var newCode = ""

    private(set) var step: Step = .old
    private(set) var oldClock: CodeClock?
    private(set) var newClock: CodeClock?
    /// 新邮箱验证码发往的地址：改了地址后旧码与冷却不再适用。
    private(set) var sentEmail: String?
    private(set) var sending = false
    private(set) var submitting = false
    private(set) var errorText: String?
    private(set) var successCount = 0
    private var started = false

    init(userId: String, currentEmail: String) {
        self.userId = userId
        self.currentEmail = currentEmail
    }

    private var oldKey: String { SentCodeStore.emailOldKey(userId: userId) }
    var email: String { trimmed(newEmail) }

    var emailError: String? {
        if email.isEmpty { return nil }
        if !AccountRules.isValidEmail(email) { return L("accountEmailChangeInvalid") }
        if email.lowercased() == currentEmail.lowercased() { return L("accountEmailChangeSame") }
        return nil
    }

    /// 当前新邮箱对应的验证码倒计时。
    var activeNewClock: CodeClock? { sentEmail == email ? newClock : nil }

    var canNext: Bool {
        !sending && oldClock != nil && !trimmed(oldCode).isEmpty && !email.isEmpty && emailError == nil
    }

    var canConfirm: Bool {
        canNext && activeNewClock != nil && !trimmed(newCode).isEmpty
    }

    /// 打开时：此前发出的原邮箱验证码仍在有效期 / 冷却内则恢复倒计时，否则自动发码。
    func start(_ api: AgentAPI) async {
        guard !started else { return }
        started = true
        if let clock = SentCodeStore.recall(oldKey) {
            oldClock = clock
            return
        }
        await sendOldCode(api, force: true)
    }

    func sendOldCode(_ api: AgentAPI, force: Bool = false) async {
        guard !sending, !submitting else { return }
        if !force, let oldClock, oldClock.cooldown(at: .now) > 0 { return }
        sending = true
        errorText = nil
        defer { sending = false }
        do throws(HostError) {
            let ttl = try await api.sendEmailCode()
            SentCodeStore.remember(oldKey, ttlSeconds: ttl.ttlSeconds)
            oldClock = CodeClock(ttlSeconds: ttl.ttlSeconds)
            oldCode = ""
            newCode = ""
        } catch {
            errorText = AccountText.error(error, context: .code)
        }
    }

    /// 第一步主按钮：新邮箱已发过且仍有效就直接进入第二步，否则向新邮箱发码（锁表单）。
    func advance(_ api: AgentAPI) async {
        guard canNext, !submitting else { return }
        if let clock = activeNewClock, clock.remaining(at: .now) > 0 {
            step = .new
            errorText = nil
            return
        }
        await sendNewCode(api, locking: true)
    }

    /// 向新邮箱发码；第一步走 `locking`（锁表单，主按钮转圈），第二步「重新发送」只让发码按钮转圈。
    func sendNewCode(_ api: AgentAPI, locking: Bool) async {
        guard canNext, !sending, !submitting else { return }
        let target = email
        if locking { submitting = true } else { sending = true }
        errorText = nil
        defer {
            submitting = false
            sending = false
        }
        do throws(HostError) {
            let ttl = try await api.sendNewEmailCode(SendNewEmailCodeParams(email: target, code: trimmed(oldCode)))
            newClock = CodeClock(ttlSeconds: ttl.ttlSeconds)
            sentEmail = target
            newCode = ""
            step = .new
        } catch {
            errorText = AccountText.error(error, context: .code)
        }
    }

    func back() {
        step = .old
        newCode = ""
        errorText = nil
    }

    /// - Returns: 修改成功。
    func confirm(_ api: AgentAPI) async -> Bool {
        guard canConfirm, !submitting else { return false }
        submitting = true
        errorText = nil
        defer { submitting = false }
        do throws(HostError) {
            try await api.changeEmail(ChangeEmailParams(email: email, oldCode: trimmed(oldCode), newCode: trimmed(newCode)))
            SentCodeStore.forget(oldKey)
            successCount += 1
            return true
        } catch {
            errorText = AccountText.error(error, context: .code)
            return false
        }
    }
}

// MARK: - 修改 / 设置密码

@MainActor
@Observable
final class PasswordChangeFlow {
    nonisolated enum Mode: Sendable, Hashable { case password, code }

    let userId: String
    let email: String
    /// `nil`（旧云端未知）按已设置处理。
    let hasPassword: Bool?

    var mode: Mode
    var current = ""
    var code = ""
    var newPassword = ""
    var confirm = ""

    private(set) var clock: CodeClock?
    private(set) var sending = false
    private(set) var submitting = false
    private(set) var errorText: String?
    private(set) var successCount = 0
    private var restored = false

    init(userId: String, email: String, hasPassword: Bool?) {
        self.userId = userId
        self.email = email
        self.hasPassword = hasPassword
        mode = hasPassword == false ? .code : .password
    }

    private var key: String { SentCodeStore.passwordKey(userId: userId) }
    var canUsePassword: Bool { hasPassword != false }
    var byCode: Bool { mode == .code }

    var ruleKey: String? {
        AccountRules.newPasswordErrorKey(new: newPassword, confirm: confirm, current: byCode ? nil : current)
    }

    /// 规则提示只在用户已输入且（已填确认或已太短）时出现。
    var visibleRuleKey: String? {
        guard let ruleKey, !newPassword.isEmpty else { return nil }
        return (!confirm.isEmpty || ruleKey == "accountErrorPasswordTooShort") ? ruleKey : nil
    }

    var canConfirm: Bool {
        guard !sending, !submitting, ruleKey == nil else { return false }
        return byCode ? (clock != nil && !trimmed(code).isEmpty) : !current.isEmpty
    }

    func setMode(_ next: Mode) {
        guard !submitting else { return }
        mode = next
        errorText = nil
    }

    /// 重开对话框时恢复此前发出的验证码倒计时。
    func restoreIfNeeded() {
        guard !restored else { return }
        restored = true
        clock = SentCodeStore.recall(key)
    }

    func sendCode(_ api: AgentAPI) async {
        guard !sending, !submitting else { return }
        sending = true
        errorText = nil
        defer { sending = false }
        do throws(HostError) {
            let ttl = try await api.sendPasswordCode()
            SentCodeStore.remember(key, ttlSeconds: ttl.ttlSeconds)
            clock = CodeClock(ttlSeconds: ttl.ttlSeconds)
            code = ""
        } catch {
            errorText = AccountText.error(error, context: .code)
        }
    }

    /// - Returns: 修改成功（其它设备已退出）。
    func submit(_ api: AgentAPI) async -> Bool {
        guard canConfirm else { return false }
        submitting = true
        errorText = nil
        defer { submitting = false }
        let params = byCode
            ? ChangePasswordParams(newPassword: newPassword, code: trimmed(code))
            : ChangePasswordParams(newPassword: newPassword, currentPassword: current)
        do throws(HostError) {
            try await api.changePassword(params)
            SentCodeStore.forget(key)
            successCount += 1
            return true
        } catch {
            errorText = AccountText.error(error, context: .code)
            return false
        }
    }
}

// MARK: - 注销账户

@MainActor
@Observable
final class DeleteAccountFlow {
    nonisolated enum Mode: Sendable, Hashable { case password, code }

    let userId: String
    let email: String
    /// `nil`（旧云端未知）按已设置处理。
    let hasPassword: Bool?

    var mode: Mode
    var current = ""
    var code = ""

    private(set) var clock: CodeClock?
    private(set) var sending = false
    private(set) var submitting = false
    private(set) var errorText: String?
    private var restored = false

    init(userId: String, email: String, hasPassword: Bool?) {
        self.userId = userId
        self.email = email
        self.hasPassword = hasPassword
        mode = hasPassword == false ? .code : .password
    }

    private var key: String { SentCodeStore.deleteKey(userId: userId) }
    var canUsePassword: Bool { hasPassword != false }
    var byCode: Bool { mode == .code }

    var canConfirm: Bool {
        guard !sending, !submitting else { return false }
        return byCode ? (clock != nil && !trimmed(code).isEmpty) : !current.isEmpty
    }

    func setMode(_ next: Mode) {
        guard !submitting else { return }
        mode = next
        errorText = nil
    }

    func restoreIfNeeded() {
        guard !restored else { return }
        restored = true
        clock = SentCodeStore.recall(key)
    }

    func sendCode(_ api: AgentAPI) async {
        guard !sending, !submitting else { return }
        sending = true
        errorText = nil
        defer { sending = false }
        do throws(HostError) {
            let ttl = try await api.sendDeleteCode()
            SentCodeStore.remember(key, ttlSeconds: ttl.ttlSeconds)
            clock = CodeClock(ttlSeconds: ttl.ttlSeconds)
            code = ""
        } catch {
            errorText = AccountText.error(error, context: .code)
        }
    }

    /// - Returns: 账户已注销（本机会话随之清除）。
    func submit(_ api: AgentAPI) async -> Bool {
        guard canConfirm else { return false }
        submitting = true
        errorText = nil
        defer { submitting = false }
        let params = byCode
            ? DeleteAccountParams(code: trimmed(code))
            : DeleteAccountParams(currentPassword: current)
        do throws(HostError) {
            try await api.deleteAccount(params)
            SentCodeStore.forget(key)
            return true
        } catch {
            errorText = AccountText.error(error, context: .code)
            return false
        }
    }
}
