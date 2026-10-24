import FluxDomain
import FluxUI
import Foundation
import Observation
import SwiftUI
import UIKit

// 账户表单的共用构件：验证码倒计时、密码输入、错误行、sheet 工具栏。全部是系统控件在 `Form` 里的内容层拼装。

// MARK: - 验证码倒计时

/// 一次发码的倒计时：有效期（`ttl` 秒）与重发冷却（发码后前 60 秒，见 `AccountRules.resendCooldown`）。
/// 不持有计时器：视图用 `TimelineView` 按 `now` 求值，因此重建视图不会重置，也不会泄漏任务。
nonisolated struct CodeClock: Equatable, Sendable {
    let sentAt: Date
    let ttl: Int64

    init(ttl: Int64, sentAt: Date = .now) {
        self.ttl = ttl
        self.sentAt = sentAt
    }

    init(ttlSeconds: UInt64, sentAt: Date = .now) {
        self.init(ttl: Int64(clamping: ttlSeconds), sentAt: sentAt)
    }

    /// 验证码剩余有效秒数（≥ 0）。
    func remaining(at now: Date) -> Int64 {
        max(0, Int64((Double(ttl) - now.timeIntervalSince(sentAt)).rounded(.up)))
    }

    /// 重发冷却剩余秒数（0 = 可重发）。
    func cooldown(at now: Date) -> Int64 {
        AccountRules.resendCooldown(remaining: remaining(at: now), ttlSeconds: ttl)
    }
}

/// 跨对话框实例记住已发出的验证码：关掉再打开对话框时恢复倒计时，不重复发码（云端 60s 限频、码仍有效）。
@MainActor
enum SentCodeStore {
    private static var records: [String: AccountRules.SentCode] = [:]

    /// 修改邮箱：原邮箱验证码。
    static func emailOldKey(userId: String) -> String { "email-old:\(userId)" }
    /// 修改 / 设置密码：绑定邮箱验证码。
    static func passwordKey(userId: String) -> String { "password:\(userId)" }
    /// 注销账户：绑定邮箱验证码（与改密码的码用途不同，分开记）。
    static func deleteKey(userId: String) -> String { "delete:\(userId)" }

    static func remember(_ key: String, ttlSeconds: UInt64) {
        records[key] = AccountRules.SentCode(sentAtMs: AccountText.nowMs(), ttlSeconds: Int64(clamping: ttlSeconds))
    }

    /// 取出仍可恢复的倒计时；已失效的顺手清掉。
    static func recall(_ key: String) -> CodeClock? {
        guard let record = records[key] else { return nil }
        guard AccountRules.restore(record, nowMs: AccountText.nowMs()) != nil else {
            records[key] = nil
            return nil
        }
        return CodeClock(ttl: record.ttlSeconds, sentAt: Date(timeIntervalSince1970: Double(record.sentAtMs) / 1000))
    }

    static func forget(_ key: String) { records[key] = nil }
}

/// 验证码输入行 + 有效期 / 重发行（`Form` 分区内容）。
struct VerificationCodeRows: View {
    @Binding var code: String
    let clock: CodeClock?
    /// 正在发码（只让发码按钮转圈，其余输入仍可用）。
    let sending: Bool
    /// 整个表单被主操作锁定。
    var locked = false
    /// 尚未发过码时发码按钮的文案键。
    var firstSendTitle = L("accountSendCode")
    /// 发码前置条件（如邮箱格式合法）。
    var canSend = true
    var fieldTitle = L("accountCodePlaceholder")
    let onSend: () -> Void

    var body: some View {
        TextField(fieldTitle, text: $code)
            .keyboardType(.numberPad)
            .textContentType(.oneTimeCode)
            .disabled(locked)
            .accessibilityLabel(L("accountFieldCode"))
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let now = context.date
            let remaining = clock?.remaining(at: now) ?? 0
            let cooldown = clock?.cooldown(at: now) ?? 0
            // 倒计时文字每秒变化：固定左右排，避免在两种版式间来回跳。
            LeadingTrailingRow {
                expiry(remaining)
            } trailing: {
                sendButton(cooldown: cooldown)
            }
        }
    }

    @ViewBuilder
    private func expiry(_ remaining: Int64) -> some View {
        if clock != nil {
            Text(remaining > 0 ? L("accountCodeExpireIn", ["seconds": remaining]) : L("accountCodeExpired"))
                .font(.footnote)
                .monospacedDigit()
                .foregroundStyle(remaining > 0 ? Color.secondary : Color.fdStatusWarningText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func sendButton(cooldown: Int64) -> some View {
        Button(action: onSend) {
            HStack(spacing: 6) {
                if sending { ProgressView() }
                Text(sendTitle(cooldown: cooldown)).monospacedDigit()
            }
        }
        .buttonStyle(.borderless)
        .disabled(sending || locked || cooldown > 0 || !canSend)
    }

    private func sendTitle(cooldown: Int64) -> String {
        if cooldown > 0 { return L("accountResendCodeIn", ["seconds": cooldown]) }
        return clock == nil ? firstSendTitle : L("accountResendCode")
    }
}

// MARK: - 输入 / 错误

/// 密码输入：`SecureField` + 显示 / 隐藏切换。
struct AccountPasswordField: View {
    let title: String
    @Binding var text: String
    var contentType: UITextContentType = .password
    var locked = false

    @State private var reveal = false

    var body: some View {
        HStack(spacing: 8) {
            Group {
                if reveal {
                    TextField(title, text: $text)
                } else {
                    SecureField(title, text: $text)
                }
            }
            .textContentType(contentType)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .disabled(locked)
            Button {
                reveal.toggle()
            } label: {
                Image(systemName: reveal ? "eye.slash" : "eye")
                    .frame(minWidth: 28, minHeight: 28)
            }
            .buttonStyle(.borderless)
            .accessibilityLabel(L(reveal ? "mobilePasswordHide" : "mobilePasswordShow"))
        }
    }
}

/// 表单内联错误（红色，图标 + 文字）。
struct AccountErrorLabel: View {
    let text: String

    var body: some View {
        Label(text, systemImage: FluxSymbol.failure)
            .font(.footnote)
            .foregroundStyle(Color.fdStatusFailedText)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityAddTraits(.updatesFrequently)
    }
}

extension View {
    /// 账户 sheet 的标准导航栏：取消 / 主操作（运行中显示菊花并锁住取消与交互式关闭）。
    func accountSheetToolbar(
        title: String,
        confirmTitle: String,
        canConfirm: Bool,
        busy: Bool,
        onCancel: @escaping () -> Void,
        onConfirm: @escaping () -> Void
    ) -> some View {
        navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .interactiveDismissDisabled(busy)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("cancel"), action: onCancel).disabled(busy)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if busy {
                        ProgressView()
                    } else {
                        Button(confirmTitle, action: onConfirm).disabled(!canConfirm)
                    }
                }
            }
    }
}

// MARK: - 乐观开关

/// 开关的乐观显示：点按后立即显示目标值，请求成功后再保持一小段时间等状态推送追上（响应与事件不保证先后，
/// 立即放手会让开关先弹回旧值再跳到新值），失败立即回滚。状态推送到达（`settle`）也会放手。
@MainActor
@Observable
final class OptimisticValues<Key: Hashable & Sendable, Value: Equatable & Sendable> {
    private(set) var values: [Key: Value] = [:]
    @ObservationIgnored private var tokens: [Key: Int] = [:]

    func value(_ key: Key, actual: Value) -> Value { values[key] ?? actual }

    func isPending(_ key: Key) -> Bool { values[key] != nil }

    /// - Returns: 请求失败时的错误（已回滚）；成功为 nil。
    func apply(
        _ key: Key,
        _ value: Value,
        hold: Duration = .seconds(2),
        work: () async throws(HostError) -> Void
    ) async -> HostError? {
        let token = (tokens[key] ?? 0) + 1
        tokens[key] = token
        values[key] = value
        do throws(HostError) {
            try await work()
        } catch {
            if tokens[key] == token { release(key) }
            return error
        }
        try? await Task.sleep(for: hold)
        if tokens[key] == token { release(key) }
        return nil
    }

    /// 真实状态已推送到：放手。
    func settle(_ key: Key) { release(key) }

    private func release(_ key: Key) {
        values[key] = nil
        tokens[key] = nil
    }
}
