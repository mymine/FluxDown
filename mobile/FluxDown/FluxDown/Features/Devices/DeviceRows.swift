import FluxDomain
import FluxUI
import SwiftUI

// V1 / V1a / V3 共用的设备行与云端设备对话框。内容层，不上玻璃。

// MARK: - 行

/// 云端已信任设备行：在线点 + 名称 + 「状态 · 平台 · 版本」+ 当前设备 / 远程任务小计徽标。
/// presence 未知时（云端连接未建立）状态词显示「状态未知」，不冒充在线 / 离线。
struct CloudDeviceRow: View {
    let record: CloudDeviceRecord
    let presenceKnown: Bool
    var remoteCount = 0

    var body: some View {
        let online = presenceKnown && record.isOnline
        let status = L(CloudPresence.deviceKey(isOnline: record.isOnline, presenceKnown: presenceKnown))
        HStack(spacing: 14) {
            GlyphTile(
                systemImage: DevicePresentation.symbol(platform: record.platform),
                tint: online ? .primary : .secondary,
                size: 40
            )
            .overlay(alignment: .bottomTrailing) {
                PresenceMark(online: online).offset(x: 4, y: 4)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(record.name)
                    .font(.body)
                    .lineLimit(2)
                    .truncationMode(.middle)
                Text(DevicePresentation.subtitle(status: status, platform: record.platform, version: record.appVersion))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                DeviceBadges(isCurrent: record.isCurrent, remoteCount: remoteCount)
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }
}

/// 局域网已配对设备行：在线点 + 名称 +「在线 / 离线 · 平台」+ tag（对方已解除 / 同一账号）。
struct LinkDeviceRow: View {
    let name: String
    let platform: String?
    let online: Bool
    var revoked = false
    var sameAccount = false
    var grantsControl = false

    var body: some View {
        HStack(spacing: 14) {
            GlyphTile(
                systemImage: DevicePresentation.symbol(platform: platform),
                tint: online ? .primary : .secondary,
                size: 40
            )
            .overlay(alignment: .bottomTrailing) {
                PresenceMark(online: online).offset(x: 4, y: 4)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(name)
                    .font(.body)
                    .lineLimit(2)
                    .truncationMode(.middle)
                Text(DevicePresentation.subtitle(online: online && !revoked, platform: platform, version: nil))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                if revoked {
                    StatusBadge(text: L("linkedDeviceRevokedTag"), tone: .warning)
                } else if sameAccount {
                    StatusBadge(text: L("linkedDeviceSameAccountTag"), tone: .neutral)
                }
                if grantsControl, !revoked {
                    // 这台设备已获批完全控制本机（手机经「添加主机 → 局域网配对」授权的）。
                    StatusBadge(text: L("linkControlGrantedTag"), tone: .warning)
                }
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }
}

/// 「当前设备」与「远程任务 N」徽标：放不下时纵排。
private struct DeviceBadges: View {
    let isCurrent: Bool
    let remoteCount: Int

    var body: some View {
        if isCurrent || remoteCount > 0 {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 6) { badges }
                VStack(alignment: .leading, spacing: 4) { badges }
            }
        }
    }

    @ViewBuilder
    private var badges: some View {
        if isCurrent {
            StatusBadge(text: L("accountDeviceCurrent"), tone: .accent)
        }
        if remoteCount > 0 {
            StatusBadge(text: L("mobileBadgeRemoteTasksCount", ["count": remoteCount]), tone: .neutral)
        }
    }
}

/// 在线 = 绿色实心点；离线 / 未知 = 空心环（永远伴随文字状态）。底色圈把它与图标块分开。
struct PresenceMark: View {
    let online: Bool

    var body: some View {
        Circle()
            .fill(Color(uiColor: .secondarySystemGroupedBackground))
            .frame(width: 14, height: 14)
            .overlay {
                if online {
                    Circle().fill(Color.fdStatusSeeding).frame(width: 8, height: 8)
                } else {
                    Circle().strokeBorder(Color.fdStatusPaused, lineWidth: 1.5).frame(width: 8, height: 8)
                }
            }
            .accessibilityHidden(true)
    }
}

// MARK: - 行操作

extension View {
    /// 云端设备行的滑动 / 长按操作：重命名、删除（只读时不提供）。删除确认框由本函数挂在该行上；重命名 `alert` 由 `cloudDeviceRenameAlert` 承接。
    func cloudDeviceActions(
        _ record: CloudDeviceRecord,
        enabled: Bool,
        renaming: Binding<CloudDeviceRecord?>,
        deleting: Binding<CloudDeviceRecord?>,
        onDeleted: @escaping (CloudDeviceRecord) -> Void = { _ in }
    ) -> some View {
        // 删除只弹确认框：不用 `role: .destructive`（会让 List 先行移除该行而数据未变，导致行数不一致崩溃）。
        swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if enabled {
                Button(L("accountDeviceDeleteAction"), systemImage: FluxSymbol.delete) {
                    deleting.wrappedValue = record
                }
                .tint(Color.fdStatusFailed)
                Button(L("renameTask"), systemImage: FluxSymbol.edit) {
                    renaming.wrappedValue = record
                }
                .tint(.accentColor)
            }
        }
        .contextMenu {
            if enabled {
                Button(L("accountDeviceRenameTitle"), systemImage: FluxSymbol.edit) { renaming.wrappedValue = record }
                Button(L("accountDeviceDeleteAction"), systemImage: FluxSymbol.delete, role: .destructive) {
                    deleting.wrappedValue = record
                }
            }
        }
        .cloudDeviceDeleteConfirmation(record, deleting: deleting, onDeleted: onDeleted)
    }

    /// 云端设备的重命名 `alert`（`accountDeviceRenameTitle` / `accountFieldDeviceName`，1–64 字符校验）。
    func cloudDeviceRenameAlert(renaming: Binding<CloudDeviceRecord?>) -> some View {
        modifier(CloudDeviceRenameAlert(renaming: renaming))
    }

    /// 云端设备删除确认框（删除本机追加 `accountDeviceDeleteCurrentWarning`，并在成功后提示已退出登录）。
    /// 必须挂在触发删除的行 / 按钮上；`onDeleted` 在删除成功后回调。
    func cloudDeviceDeleteConfirmation(
        _ record: CloudDeviceRecord,
        deleting: Binding<CloudDeviceRecord?>,
        onDeleted: @escaping (CloudDeviceRecord) -> Void = { _ in }
    ) -> some View {
        modifier(CloudDeviceDeleteConfirmation(record: record, deleting: deleting, onDeleted: onDeleted))
    }
}

/// 直连设备重命名目标：只带指纹与当前备注名（预填输入框；留空 = 清除备注名）。
struct LinkRenameTarget: Equatable, Identifiable {
    var fingerprint: String
    var customName: String
    var id: String { fingerprint }
}

extension View {
    /// 直连设备的重命名 `alert`（`accountDeviceRenameTitle` / `linkedDeviceRenameHint`，留空恢复对方设备名，非空 1–64 字符）。
    func linkDeviceRenameAlert(renaming: Binding<LinkRenameTarget?>) -> some View {
        modifier(LinkDeviceRenameAlert(renaming: renaming))
    }
}

private struct LinkDeviceRenameAlert: ViewModifier {
    @Environment(AppContainer.self) private var container
    @Environment(DevicesModel.self) private var model
    @Binding var renaming: LinkRenameTarget?

    @State private var draft: String?
    /// 对话框关闭时 `renaming` 会先被清空：保留最近一次的目标供确认按钮使用。
    @State private var target: LinkRenameTarget?

    func body(content: Content) -> some View {
        let text = draft ?? renaming?.customName ?? ""
        let valid = LinkRules.isValidRename(text)
        content
            .onChange(of: renaming) { _, new in
                if let new {
                    target = new
                    draft = nil
                }
            }
            .alert(
                L("accountDeviceRenameTitle"),
                isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })
            ) {
                TextField(
                    L("accountFieldDeviceName"),
                    text: Binding(get: { text }, set: { draft = $0 })
                )
                Button(L("cancel"), role: .cancel) { draft = nil }
                Button(L("confirm")) { submit(text) }
                    .disabled(!valid)
            } message: {
                Text(valid ? L("linkedDeviceRenameHint") : L("accountDeviceRenameInvalid"))
            }
    }

    private func submit(_ text: String) {
        guard let target else { return }
        draft = nil
        guard LinkRules.isValidRename(text) else { return }
        let name = LinkRules.renameValue(text)
        Task {
            if let error = await model.renameLink(fingerprint: target.fingerprint, name: name) {
                container.toasts.show(text: AccountText.error(error, context: .pairing), tone: .error)
            }
        }
    }
}

private struct CloudDeviceRenameAlert: ViewModifier {
    @Environment(AppContainer.self) private var container
    @Environment(DevicesModel.self) private var model
    @Binding var renaming: CloudDeviceRecord?

    /// 编辑中的名称，按设备 id 归属（别的设备的残留草稿不会串到本次输入框）。
    private struct Draft {
        var id: String
        var text: String
    }

    @State private var draft: Draft?
    /// 对话框关闭时 `renaming` 会先被清空：保留最近一次的目标供确认按钮使用。
    @State private var renameTarget: CloudDeviceRecord?

    func body(content: Content) -> some View {
        let valid = AccountRules.isValidDeviceName(text(for: renaming))
        content
            .onChange(of: renaming) { _, new in
                if let new { renameTarget = new }
            }
            .alert(
                L("accountDeviceRenameTitle"),
                isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })
            ) {
                TextField(
                    L("accountFieldDeviceName"),
                    text: Binding(
                        get: { text(for: renaming) },
                        set: { value in
                            if let id = renaming?.id { draft = Draft(id: id, text: value) }
                        }
                    )
                )
                Button(L("cancel"), role: .cancel) { draft = nil }
                Button(L("confirm")) { submitRename() }
                    .disabled(!valid)
            } message: {
                if !valid { Text(L("accountDeviceRenameInvalid")) }
            }
    }

    private func text(for record: CloudDeviceRecord?) -> String {
        if let draft, draft.id == record?.id { return draft.text }
        return record?.name ?? ""
    }


    private func submitRename() {
        guard let record = renameTarget else { return }
        let name = AccountRules.trimmed(text(for: record))
        draft = nil
        guard AccountRules.isValidDeviceName(name), name != record.name else { return }
        Task {
            if let error = await model.rename(record, to: name) {
                container.toasts.show(text: AccountText.error(error), tone: .error)
            }
        }
    }

}

/// 删除确认 alert：挂在触发它的行 / 按钮上。
private struct CloudDeviceDeleteConfirmation: ViewModifier {
    @Environment(AppContainer.self) private var container
    @Environment(DevicesModel.self) private var model
    let record: CloudDeviceRecord
    @Binding var deleting: CloudDeviceRecord?
    let onDeleted: (CloudDeviceRecord) -> Void

    func body(content: Content) -> some View {
        content
            .alert(
                L("accountDeviceDeleteConfirmTitle"),
                isPresented: Binding(get: { deleting?.id == record.id }, set: { if !$0 { deleting = nil } })
            ) {
                Button(L("accountDeviceDeleteAction"), role: .destructive) { performDelete(record) }
                Button(L("cancel"), role: .cancel) {}
            } message: {
                Text(deleteMessage(record))
            }
    }

    private func deleteMessage(_ record: CloudDeviceRecord) -> String {
        let base = L("accountDeviceDeleteConfirmDesc")
        return record.isCurrent ? base + "\n" + L("accountDeviceDeleteCurrentWarning") : base
    }

    private func performDelete(_ record: CloudDeviceRecord) {
        Task {
            if let error = await model.delete(record) {
                container.toasts.show(text: AccountText.error(error), tone: .error)
                return
            }
            if record.isCurrent {
                container.toasts.show(text: L("mobileDeviceDeletedSignedOut"), tone: .info)
            }
            onDeleted(record)
        }
    }
}
