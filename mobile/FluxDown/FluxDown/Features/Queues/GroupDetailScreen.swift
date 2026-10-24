import FluxDomain
import FluxUI
import Observation
import SwiftUI

/// D6 任务组详情（02-downloads §8）。compact：下载栈推入页；regular：右栏根页（由宿主决定是否有返回钮）。
/// 数据全部来自实时 `HostState`：概览（进度 / 状态计数）、信息卡（来源 / 目录 / 创建时间）、成员行（与下载列表同款行与菜单）。
/// 组被删除（本端 / 远端 / 切换主机）→ 显示「任务组已不存在」，1.2 s 后自动返回。
struct GroupDetailScreen: View {
    let groupId: String

    var body: some View {
        // 以 groupId 为身份：右栏切换任务组时重建模型。
        GroupDetailLoader(groupId: groupId).id(groupId)
    }
}

// MARK: - 模型

/// 组详情的派生模型：成员行复用下载列表的 `DownloadsDeriver`（`groupId` 范围 + 智能排序，无分组）。
/// 与 `DownloadsModel` 同构：观察主机状态，派生结果相等时不发布，页面不随 10 Hz 的速度刷新整页重算。
@MainActor
@Observable
final class GroupDetailModel {
    let groupId: String
    private(set) var group: DownloadGroup?
    private(set) var summary = GroupSummary(members: [])
    private(set) var items: [TaskItem] = []
    /// 排队顺序：可见排队行按稳定键取名次（`QueueOrder.rank`），顺序变化不重建行模型。
    private(set) var queueOrder = QueueOrder.empty
    private(set) var isReadOnly = true
    /// 首次快照到达前（含尚未绑定）：不能判定「组不存在」。
    private(set) var isLoading = true
    /// 任务表分页载入中：成员 / 汇总是半份列表，不得显示成「没有成员」。
    private(set) var tasksLoading = false

    @ObservationIgnored private var store: HostStore?
    @ObservationIgnored private var deriver = DownloadsDeriver()

    init(groupId: String) {
        self.groupId = groupId
    }

    /// 绑定数据源并开始观察（幂等）。
    func bind(store: HostStore) {
        guard self.store == nil else { return }
        self.store = store
        recompute()
        track()
    }

    private func track() {
        guard let store else { return }
        withObservationTracking {
            _ = store.state
        } onChange: { [weak self] in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.recompute()
                self.track()
            }
        }
    }

    private func recompute() {
        guard let store else { return }
        let state = store.state

        let nextGroup = state.groups.first { $0.groupId == groupId }
        if nextGroup != group { group = nextGroup }

        let members = GroupSummary.members(of: groupId, in: state.tasks)
        let nextSummary = GroupSummary(members: members)
        if nextSummary != summary { summary = nextSummary }

        var filter = DownloadsFilter()
        filter.groupId = groupId
        let result = deriver.derive(
            DeriveInput(
                state: state,
                order: ViewOrder(groupBy: .none, sortKey: .smart, ascending: false),
                filter: filter,
                collapsed: []
            ),
            nowMs: Int64(Date().timeIntervalSince1970 * 1000),
            interactionMs: 0
        )
        let nextItems = result.list.sections.flatMap(\.rows).compactMap(\.item)
        if nextItems != items { items = nextItems }
        if state.queueOrder != queueOrder { queueOrder = state.queueOrder }

        if state.isReadOnly != isReadOnly { isReadOnly = state.isReadOnly }
        let loading = state.connection == .connecting && state.tasks.isEmpty && state.groups.isEmpty
        if loading != isLoading { isLoading = loading }
        if state.tasksLoading != tasksLoading { tasksLoading = state.tasksLoading }
    }
}

// MARK: - 加载 / 不存在

/// 读取实时模型并处理「加载中 / 已不存在」。
private struct GroupDetailLoader: View {
    let groupId: String

    @Environment(AppContainer.self) private var container
    @Environment(\.downloadsNavigation) private var navigation
    @State private var model: GroupDetailModel
    /// 曾经见过该组：只有「见过后消失」才自动返回（从未存在的 id 保留页面由用户返回）。
    @State private var seen = false

    init(groupId: String) {
        self.groupId = groupId
        _model = State(initialValue: GroupDetailModel(groupId: groupId))
    }

    var body: some View {
        let group = model.group
        let closing = seen && group == nil && !model.isLoading
        Group {
            if let group {
                GroupDetailPage(model: model, group: group)
            } else if model.isLoading {
                ProgressView(L("pluginCommonLoading"))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .navigationTitle(L("detail"))
                    .navigationBarTitleDisplayMode(.inline)
            } else {
                ContentUnavailableView {
                    Label(L("mobileGroupGone"), systemImage: "questionmark.folder")
                } description: {
                    Text(L("mobileGroupGoneSub"))
                }
                .navigationTitle(L("detail"))
                .navigationBarTitleDisplayMode(.inline)
            }
        }
        .onAppear { model.bind(store: container.store) }
        .onChange(of: group != nil, initial: true) { _, present in
            if present { seen = true }
        }
        .task(id: closing) {
            guard closing else { return }
            do {
                try await Task.sleep(for: .milliseconds(1200))
            } catch {
                return
            }
            navigation?.closeGroup(groupId)
        }
    }
}

// MARK: - 页面

private struct GroupDetailPage: View {
    let model: GroupDetailModel
    let group: DownloadGroup

    @Environment(AppContainer.self) private var container
    @Environment(TaskActions.self) private var actions
    @Environment(ToastCenter.self) private var toasts
    @Environment(\.downloadsNavigation) private var navigation
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.fluxAccent) private var accent
    @State private var pendingDelete: GroupDeleteRequest?

    private var summary: GroupSummary { model.summary }
    private var title: String { group.name.isEmpty ? group.groupId : group.name }
    private var commands: GroupCommands { GroupCommands(actions: actions, toasts: toasts) }
    private var canShowInFiles: Bool { container.isLocalHost && !group.saveDir.isEmpty }

    var body: some View {
        let style = RowStyle(density: container.viewPrefs.density, fields: Set(container.viewPrefs.fields))
        List {
            Section {
                GroupOverviewCard(title: title, summary: summary, loading: model.tasksLoading)
                    .listRowInsets(EdgeInsets(top: 18, leading: 18, bottom: 18, trailing: 18))
            }
            Section {
                infoRows
            }
            Section {
                if model.tasksLoading {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text(L("mobileTasksLoading"))
                    }
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .accessibilityElement(children: .combine)
                } else if model.items.isEmpty {
                    Text(L("groupDetailNoMembers"))
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .multilineTextAlignment(.center)
                } else {
                    ForEach(model.items) { item in
                        row(item, style: style)
                    }
                }
            } header: {
                HStack {
                    Text(L("groupDetailMembersTab"))
                    Spacer(minLength: 0)
                    if !model.tasksLoading { Text(model.items.count, format: .number) }
                }
                .font(.subheadline.weight(.semibold))
                .monospacedDigit()
                .textCase(nil)
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.isHeader)
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarVisibility(sizeClass == .compact ? .hidden : .automatic, for: .tabBar)
        .toolbar { toolbarContent }
        .suppressesBottomAccessory("groupDetail")
    }

    // MARK: 信息卡

    @ViewBuilder
    private var infoRows: some View {
        KeyValueRow(
            key: L("groupDetailSource"),
            value: group.sourceUrl.isEmpty ? Format.dash : group.sourceUrl,
            monospaced: true,
            copyable: !group.sourceUrl.isEmpty,
            copyLabel: L("webCopy"),
            onCopy: { toasts.show(text: L("urlCopied"), tone: .success, systemImage: FluxSymbol.copy) }
        )
        KeyValueRow(
            key: L("groupDetailSaveDir"),
            value: group.saveDir.isEmpty ? Format.dash : group.saveDir,
            monospaced: true,
            copyable: !group.saveDir.isEmpty,
            copyLabel: L("webCopy"),
            onCopy: { toasts.show(text: L("mobilePathCopied"), tone: .success, systemImage: FluxSymbol.copy) }
        )
        if group.createdAt > 0 {
            KeyValueRow(key: L("groupDetailCreatedAt"), value: TaskDetailFormat.dateTime(group.createdAt), monospaced: true)
        }
    }

    // MARK: 成员行（与下载列表同款）

    private func row(_ item: TaskItem, style: RowStyle) -> some View {
        let queuePosition = item.visual == .queued ? model.queueOrder.rank(of: item.id) : 0
        return DownloadRowView(
            item: item,
            style: style,
            isLocalHost: container.isLocalHost,
            queuePosition: queuePosition,
            showsRing: true,
            zoom: nil,
            onOpen: { navigation?.open(.task(item.id)) }
        )
        .equatable()
        .taskRowSwipeActions(item, readOnly: model.isReadOnly)
        .contextMenu {
            TaskMenuItems(task: item.task, boosted: item.boosted)
        } preview: {
            DownloadRowView(
                item: item, style: style, isLocalHost: container.isLocalHost, queuePosition: queuePosition, showsRing: false
            )
                .padding(.horizontal)
                .frame(minWidth: 320)
                .environment(actions)
                .environment(\.fluxAccent, accent)
        }
        .taskDeleteHost()
    }

    // MARK: 工具栏

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Button(L("groupPauseAll"), systemImage: FluxSymbol.pause) { commands.pauseAll(group.groupId) }
                    .disabled(!summary.canPauseAll || model.isReadOnly)
                Button(L("groupResumeAll"), systemImage: FluxSymbol.resume) { commands.resumeAll(group.groupId) }
                    .disabled(!summary.canResumeAll || model.isReadOnly)
                if summary.canRetryFailed {
                    Button(L("groupRetryFailed"), systemImage: FluxSymbol.retry) { commands.retryFailed(summary.failedIds) }
                        .disabled(model.isReadOnly)
                }
                if canShowInFiles || !group.sourceUrl.isEmpty {
                    Divider()
                    if canShowInFiles {
                        Button(L("mobileShowInFiles"), systemImage: FluxSymbol.folder) { commands.showInFiles(group) }
                    }
                    if !group.sourceUrl.isEmpty {
                        Button(L("groupCopySourceLink"), systemImage: FluxSymbol.copy) { commands.copySourceLink(group) }
                    }
                }
                Divider()
                Button(L("groupDelete"), systemImage: FluxSymbol.delete, role: .destructive) {
                    pendingDelete = GroupDeleteRequest(groupId: group.groupId, name: title, withFiles: false)
                }
                .disabled(model.isReadOnly)
                Button(L("groupDeleteWithFiles"), systemImage: "trash.fill", role: .destructive) {
                    pendingDelete = GroupDeleteRequest(groupId: group.groupId, name: title, withFiles: true)
                }
                .disabled(model.isReadOnly)
            } label: {
                Label(L("moreActions"), systemImage: FluxSymbol.more)
            }
            .groupDeleteConfirmation($pendingDelete, groupId: group.groupId) { navigation?.closeGroup(group.groupId) }
        }
        ToolbarItem(placement: .bottomBar) {
            Button(L("groupPauseAll"), systemImage: FluxSymbol.pause) {
                FluxHaptic.light.play()
                commands.pauseAll(group.groupId)
            }
            .disabled(!summary.canPauseAll || model.isReadOnly)
        }
        ToolbarItem(placement: .bottomBar) {
            Button(L("groupRetryFailed"), systemImage: FluxSymbol.retry) {
                FluxHaptic.light.play()
                commands.retryFailed(summary.failedIds)
            }
            .disabled(!summary.canRetryFailed || model.isReadOnly)
        }
        ToolbarSpacer(.flexible, placement: .bottomBar)
        ToolbarItem(placement: .bottomBar) {
            Button(L("groupResumeAll"), systemImage: FluxSymbol.resume) {
                FluxHaptic.light.play()
                commands.resumeAll(group.groupId)
            }
            .labelStyle(.titleAndIcon)
            .buttonStyle(.borderedProminent)
            .disabled(!summary.canResumeAll || model.isReadOnly)
        }
    }
}

// MARK: - 概览卡

/// 概览：图标 + 组名 + 「完成数/总数 · 百分比」，确定进度条，状态计数徽标（颜色 + 图形冗余）。
private struct GroupOverviewCard: View {
    let title: String
    let summary: GroupSummary
    let loading: Bool

    @Environment(\.dynamicTypeSize) private var typeSize
    @Environment(\.fluxAccent) private var accent
    @ScaledMetric(relativeTo: .title2) private var iconSize = 28

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            if let progress = summary.progress {
                ProgressView(value: progress)
                    .tint(summary.isFinished ? Color.fdStatusSeeding : accent.color)
                    .accessibilityLabel(L("groupDetailMembersTab"))
                    .accessibilityValue(Format.percent(progress))
            }
            if summary.total > 0 {
                FlowLayout(spacing: 6) {
                    badges
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder
    private var header: some View {
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 8))
            : AnyLayout(HStackLayout(alignment: .center, spacing: 12))
        layout {
            Image(systemName: "square.stack.3d.down.right")
                .font(.system(size: iconSize))
                .foregroundStyle(accent.color)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.headline)
                    .textSelection(.enabled)
                Text(statusLine)
                    .font(.subheadline)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var statusLine: String {
        guard !loading else { return L("mobileTasksLoading") }
        guard summary.total > 0 else { return L("groupDetailNoMembers") }
        let done = L("groupDoneOfTotal", ["done": summary.completed, "total": summary.total])
        return summary.progress.map { done + " · " + Format.percent($0) } ?? done
    }

    @ViewBuilder
    private var badges: some View {
        if summary.completed > 0 {
            StatusBadge(text: L("groupDoneCount", ["n": summary.completed]), tone: .success, systemImage: FluxSymbol.success)
        }
        if summary.downloading > 0 {
            StatusBadge(text: L("groupDownloadingCount", ["n": summary.downloading]), tone: .accent, systemImage: "arrow.down.circle.fill")
        }
        if summary.pending > 0 {
            StatusBadge(text: L("groupPendingCount", ["n": summary.pending]), tone: .neutral, systemImage: "clock.fill")
        }
        if summary.paused > 0 {
            StatusBadge(text: L("groupPausedCount", ["n": summary.paused]), tone: .warning, systemImage: "pause.circle.fill")
        }
        if summary.failed > 0 {
            StatusBadge(text: L("groupFailedCount", ["n": summary.failed]), tone: .failure, systemImage: FluxSymbol.warning)
        }
    }
}
