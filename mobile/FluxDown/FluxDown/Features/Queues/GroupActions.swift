import FluxDomain
import FluxUI
import SwiftUI
import UIKit

// 任务组（D6）的共享动作与导航胶水：组头菜单（下载列表）与组详情页共用同一套命令 / 删除确认。

// MARK: - 命令

/// 组命令（`daemon.group.*`）：只读拦截 / 错误 Toast 由 `TaskActions.run` 统一处理。
@MainActor
struct GroupCommands {
    let actions: TaskActions
    let toasts: ToastCenter

    func pauseAll(_ groupId: String) {
        actions.run { session throws(HostError) in
            try await session.callVoid(HostMethod.daemonGroupPause, params: GroupIdParams(groupId: groupId))
        }
    }

    func resumeAll(_ groupId: String) {
        actions.run { session throws(HostError) in
            try await session.callVoid(HostMethod.daemonGroupResume, params: GroupIdParams(groupId: groupId))
        }
    }

    /// 重试失败成员：逐个 `resume`（与 Web `retryFailedInGroup` 同）。
    func retryFailed(_ ids: [String]) {
        guard !ids.isEmpty else { return }
        actions.resume(ids)
    }

    /// 删除任务组；成功后 Toast 并回调（详情页据此返回）。
    func delete(_ request: GroupDeleteRequest, onDone: (@MainActor () -> Void)? = nil) {
        actions.run(onSuccess: { [toasts] in
            toasts.show(text: L("mobileGroupDeleted"), tone: .success, systemImage: FluxSymbol.delete)
            onDone?()
        }) { session throws(HostError) in
            try await session.callVoid(
                HostMethod.daemonGroupDelete,
                params: GroupDeleteParams(groupId: request.groupId, deleteFiles: request.withFiles)
            )
        }
    }

    /// 在「文件」App 中显示组保存目录（仅本机主机）。
    func showInFiles(_ group: DownloadGroup) {
        guard !group.saveDir.isEmpty, let url = LocalPaths.filesAppURL(forDirectory: group.saveDir) else {
            toasts.show(text: L("mobileFileNotFound"), tone: .warning, systemImage: "exclamationmark.triangle")
            return
        }
        UIApplication.shared.open(url)
    }

    func copySourceLink(_ group: DownloadGroup) {
        guard !group.sourceUrl.isEmpty else { return }
        UIPasteboard.general.string = group.sourceUrl
        FluxHaptic.success.play()
        toasts.show(text: L("urlCopied"), tone: .success, systemImage: FluxSymbol.copy)
    }
}

// MARK: - 删除确认

/// 待确认的组删除：保留文件 / 连同文件各自一档确认。
nonisolated struct GroupDeleteRequest: Identifiable, Equatable {
    let groupId: String
    /// 组名；为空回退组 id。
    let name: String
    let withFiles: Bool

    var id: String { groupId + (withFiles ? ":files" : "") }
}

extension View {
    /// 组删除确认对话框，挂在触发它的视图上：
    /// `request` 非 nil 且指向 `groupId` 时呈现，确认后执行删除并回调 `onDeleted`。
    /// 同一个 `request` 可被多个组的触发视图共用——各自只响应自己的 `groupId`；`groupId` 为 nil（非真实任务组）恒不呈现。
    func groupDeleteConfirmation(
        _ request: Binding<GroupDeleteRequest?>,
        groupId: String?,
        onDeleted: (@MainActor () -> Void)? = nil
    ) -> some View {
        modifier(GroupDeleteConfirmation(request: request, groupId: groupId, onDeleted: onDeleted))
    }
}

private struct GroupDeleteConfirmation: ViewModifier {
    @Binding var request: GroupDeleteRequest?
    let groupId: String?
    let onDeleted: (@MainActor () -> Void)?

    @Environment(TaskActions.self) private var actions
    @Environment(ToastCenter.self) private var toasts

    func body(content: Content) -> some View {
        content.alert(
            request.map { $0.withFiles ? L("groupDeleteWithFiles") : L("groupDelete") } ?? "",
            isPresented: Binding(
                get: { groupId != nil && request?.groupId == groupId },
                set: { if !$0 { request = nil } }
            ),
            presenting: request
        ) { pending in
            Button(pending.withFiles ? L("groupDeleteWithFiles") : L("groupDelete"), role: .destructive) {
                GroupCommands(actions: actions, toasts: toasts).delete(pending, onDone: onDeleted)
            }
            Button(L("cancel"), role: .cancel) {}
        } message: { pending in
            Text(
                pending.withFiles
                    ? L("deleteConfirmDescWithFile", ["fileName": pending.name])
                    : L("deleteConfirmDescKeepFile", ["fileName": pending.name])
            )
        }
    }
}

// MARK: - 下载列表分区 ↔ 任务组

extension DownloadsSection {
    /// 「按任务组」分组下的真实任务组 id；`group:none`（未分组）与其它分组方式的分区返回 nil。
    /// 调用方须同时确认当前 `groupBy == .group`（`.group` 分区种类也用于状态 / 日期等分组）。
    var taskGroupId: String? {
        let prefix = "group:"
        guard kind == .group, id.hasPrefix(prefix), id != "group:none" else { return nil }
        let value = String(id.dropFirst(prefix.count))
        return value.isEmpty ? nil : value
    }
}

// MARK: - 下载栈导航（组详情内部跳转）

/// 宿主布局（compact 栈 / regular 右栏）提供给组详情的导航能力：成员行推入任务详情、组消失时关闭自身。
struct DownloadsNavigation {
    var open: @MainActor (DownloadsRoute) -> Void
    var closeGroup: @MainActor (String) -> Void
    var closeTask: @MainActor (String) -> Void
}

extension EnvironmentValues {
    @Entry var downloadsNavigation: DownloadsNavigation?
}

// MARK: - 行滑动（下载列表与组详情共用）

/// 任务行滑动动作（02-downloads §1.3 状态矩阵）：左滑 暂停 / 继续 / 重试 + 优先，右滑 删除 + 复制链接。
private struct TaskRowSwipeActions: ViewModifier {
    let item: TaskItem
    let readOnly: Bool

    @Environment(TaskActions.self) private var actions
    @Environment(\.confirmTaskDelete) private var confirmDelete
    @Environment(\.fluxAccent) private var accent

    func body(content: Content) -> some View {
        content
            .swipeActions(edge: .leading, allowsFullSwipe: true) {
                if !readOnly { leading }
            }
            .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                if !readOnly, item.task.status != .unknown { trailing }
            }
    }

    @ViewBuilder
    private var leading: some View {
        switch item.task.status {
        case .downloading, .pending:
            Button(L("pause"), systemImage: FluxSymbol.pause) { actions.pause([item.id]) }
                .tint(Color.fdStatusWarning)
            Button(item.boosted ? L("cancelBoost") : L("boostDownload"), systemImage: FluxSymbol.boost) {
                actions.boost(item.task, boosted: item.boosted)
            }
            .tint(Color.fdBoost)
        case .paused:
            Button(L("resume"), systemImage: FluxSymbol.resume) { actions.resume([item.id]) }
                .tint(accent.color)
        case .failed:
            Button(L("mobileRetry"), systemImage: FluxSymbol.retry) { actions.resume([item.id]) }
                .tint(accent.color)
        case .completed:
            if item.task.presentsSeeding {
                Button(L("pause"), systemImage: FluxSymbol.pause) { actions.pause([item.id]) }
                    .tint(Color.fdStatusWarning)
            } else if item.task.presentsResumeSeeding {
                Button(L("resume"), systemImage: FluxSymbol.resume) { actions.resume([item.id]) }
                    .tint(accent.color)
            }
        case .preparing, .unknown:
            EmptyView()
        }
    }

    @ViewBuilder
    private var trailing: some View {
        // 删除 → 对话框（保留文件 / 连同文件）；全滑同样走对话框，不会无确认删除。
        Button(L("delete"), systemImage: FluxSymbol.delete) { confirmDelete?.confirm([item.task]) }
            .tint(Color.fdStatusFailed)
        Button(L("mobileSwipeCopyLink"), systemImage: FluxSymbol.copy) { actions.copyLink(item.task) }
            .tint(Color.fdStatusPaused)
    }
}

extension View {
    func taskRowSwipeActions(_ item: TaskItem, readOnly: Bool) -> some View {
        modifier(TaskRowSwipeActions(item: item, readOnly: readOnly))
    }
}
