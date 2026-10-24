import FluxDomain
import FluxUI
import SwiftUI

/// 吸顶筛选区（02-downloads §3.1 / §3.2）：状态范围条（`GlassScopeBar`，唯一的玻璃件）+ 范围 / 分类芯片行（内容层，不上玻璃）。
/// 选中「状态 + 分类」= 交集；状态计数忽略分类，分类计数 = 当前状态 ∩ 该分类。分类芯片之后是「远程任务」芯片（与分类互斥，只看其他设备上的任务）。
/// 各部分的显隐由云同步偏好 `ui.show_sidebar_status|queues|category`（通用设置「下载页显示」）决定。
struct DownloadsFilterBar: View {
    let model: DownloadsModel
    @Environment(AppContainer.self) private var container
    @Environment(TaskActions.self) private var actions

    var body: some View {
        let visibility = FilterBarVisibility(container.store.state.preferences)
        Group {
            if visibility.isEmpty(hasCategories: !model.facets.categories.isEmpty || showsRemoteChip, hasScopeChip: showsScopeChip(visibility)) {
                EmptyView()
            } else {
                VStack(spacing: 8) {
                    if visibility.status {
                        GlassScopeBar(items: scopeItems, selection: folderBinding)
                            .padding(.horizontal)
                    }
                    if showsChips(visibility) {
                        ScrollView(.horizontal) {
                            HStack(spacing: 8) {
                                scopeChip(visibility)
                                if visibility.categories {
                                    ForEach(model.facets.categories) { pill in
                                        categoryChip(pill)
                                    }
                                    if showsRemoteChip { remoteChip }
                                }
                            }
                            .padding(.horizontal)
                        }
                        .scrollIndicators(.hidden)
                        .scrollBounceBehavior(.basedOnSize)
                    }
                }
                .padding(.bottom, 4)
            }
        }
        // 被隐藏的部分对应的筛选必须复位，否则列表会被一个看不见的筛选卡住。
        .onChange(of: visibility, initial: true) { _, now in
            if !now.status, model.filter.folder != .all { model.setFolder(.all) }
            if !now.queues, model.filter.queueId != nil { model.setQueue(nil) }
            if !now.categories {
                if model.filter.categoryId != nil { model.setCategory(nil) }
                if model.filter.remoteOnly { model.setRemoteOnly(false) }
            }
        }
    }

    private var folderBinding: Binding<StatusFolder> {
        Binding(get: { model.filter.folder }, set: { model.setFolder($0) })
    }

    private var scopeItems: [ScopeItem<StatusFolder>] {
        // 任务表分页载入中：计数是半份列表算出来的，不显示（nil）。
        let loaded = model.list.loaded
        return StatusFolder.visible.map { folder in
            let count = model.facets.count(folder)
            let tone: ScopeCountTone = switch folder {
            case .active: .accent
            case .failed: count > 0 ? .failure : .secondary
            default: .secondary
            }
            return ScopeItem(id: folder, title: folder.title, count: loaded ? count : nil, countTone: tone)
        }
    }

    private func showsChips(_ visibility: FilterBarVisibility) -> Bool {
        (visibility.categories && (!model.facets.categories.isEmpty || showsRemoteChip)) || showsScopeChip(visibility)
    }

    private func showsScopeChip(_ visibility: FilterBarVisibility) -> Bool {
        visibility.queues && (model.facets.queues.count > 1 || model.filter.queueId != nil)
    }

    // MARK: 范围芯片（队列）

    @ViewBuilder
    private func scopeChip(_ visibility: FilterBarVisibility) -> some View {
        if showsScopeChip(visibility) {
            let scoped = model.filter.queueId
            let name = scoped.flatMap { id in model.facets.queues.first { normalizedQueueId($0.queue.queueId) == id }?.queue.displayName } ?? scoped
            Menu {
                Picker(L("mobileViewScope"), selection: queueBinding) {
                    Text(L("mobileViewAllQueues")).tag(String?.none)
                    ForEach(model.facets.queues) { facet in
                        Label(queueMenuTitle(facet), systemImage: facet.queue.isRunning ? "play.circle" : "pause.circle")
                            .tag(Optional(normalizedQueueId(facet.queue.queueId)))
                    }
                }
                .pickerStyle(.inline)
                Section {
                    if let queue = scopedQueue(scoped) {
                        if !queue.isRunning || queue.canStop {
                            Button(
                                L(queue.isRunning ? "stopQueueAction" : "startQueueAction"),
                                systemImage: queue.isRunning ? FluxSymbol.pause : FluxSymbol.resume
                            ) { setRunning(!queue.isRunning, queue: queue) }
                        }
                    }
                    Button(L("manageQueueAction"), systemImage: FluxSymbol.queue) { container.router.sheet = .queues }
                }
            } label: {
                FilterChip(
                    title: name.map { L("mobileScopeQueue", ["name": $0]) } ?? L("mobileViewScope"),
                    systemImage: FluxSymbol.queue,
                    selected: scoped != nil
                )
            }
            .accessibilityLabel(name.map { L("mobileScopeQueue", ["name": $0]) } ?? L("mobileViewScope"))
        }
    }

    private var queueBinding: Binding<String?> {
        Binding(get: { model.filter.queueId }, set: { model.setQueue($0) })
    }

    /// 当前范围对应的队列（未限定范围 = nil）。
    private func scopedQueue(_ scoped: String?) -> TaskQueue? {
        guard let scoped else { return nil }
        return model.facets.queues.first { normalizedQueueId($0.queue.queueId) == scoped }?.queue
    }

    /// `daemon.queue.start/stop`：走 `TaskActions.run`（只读拦截 + 失败 Toast）；运行状态经快照回流。
    private func setRunning(_ running: Bool, queue: TaskQueue) {
        let queueId = queue.queueId
        let name = queue.displayName
        actions.run(onSuccess: { [toasts = container.toasts] in
            toasts.show(
                text: QueueText.runningToast(running, name: name),
                tone: .info,
                systemImage: running ? FluxSymbol.resume : FluxSymbol.pause
            )
        }) { session throws(HostError) in
            try await session.callVoid(
                running ? HostMethod.daemonQueueStart : HostMethod.daemonQueueStop,
                params: QueueIdParams(queueId: queueId)
            )
        }
    }

    private func queueMenuTitle(_ facet: QueueFacet) -> String {
        let state = L(facet.queue.isRunning ? "queueRunningBadge" : "queueStoppedBadge")
        // 任务表分页载入中：队列任务数是半份列表算出来的，不显示。
        guard model.list.loaded else { return "\(facet.queue.displayName) · \(state)" }
        return "\(facet.queue.displayName) · \(state) · \(L("nTasks", ["n": facet.count]))"
    }

    // MARK: 分类芯片

    private func categoryChip(_ pill: CategoryPill) -> some View {
        let selected = model.filter.categoryId == pill.category.id
        return Button {
            model.setCategory(selected ? nil : pill.category.id)
        } label: {
            FilterChip(title: pill.category.displayName, count: pill.count, dot: pill.category.tint, selected: selected)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .sensoryFeedback(.selection, trigger: selected)
    }

    // MARK: 远程任务芯片

    /// 其他设备上的远程任务：有远程任务（当前范围 / 状态内）或已选中时显示，跟在分类芯片后；与分类互斥。
    private var showsRemoteChip: Bool {
        model.facets.remoteCount > 0 || model.filter.remoteOnly
    }

    private var remoteChip: some View {
        let selected = model.filter.remoteOnly
        return Button {
            model.setRemoteOnly(!selected)
        } label: {
            FilterChip(title: L("remoteTasksGroup"), count: model.facets.remoteCount, systemImage: FluxSymbol.cloud, selected: selected)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .sensoryFeedback(.selection, trigger: selected)
    }
}

/// 筛选芯片（高 34）：选中 = 强调色 16% 底 + 1pt 描边；未选 = 内容层卡片色。
struct FilterChip: View {
    let title: String
    var count: Int?
    var systemImage: String?
    var dot: Color?
    let selected: Bool

    @Environment(\.fluxAccent) private var accent

    var body: some View {
        HStack(spacing: 6) {
            if let dot {
                Circle().fill(dot).frame(width: 8, height: 8).accessibilityHidden(true)
            }
            if let systemImage {
                Image(systemName: systemImage).imageScale(.small).accessibilityHidden(true)
            }
            Text(title)
            if let count, count > 0 {
                Text(count, format: .number)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
        }
        .font(.subheadline.weight(selected ? .semibold : .medium))
        .foregroundStyle(selected ? accent.text : Color.primary)
        .lineLimit(1)
        .padding(.horizontal, 12)
        .frame(minHeight: 34)
        .background(selected ? accent.color.opacity(0.16) : Color(uiColor: .secondarySystemGroupedBackground), in: .capsule)
        .overlay {
            if selected { Capsule().stroke(accent.color, lineWidth: 1) }
        }
        .padding(.vertical, 5)
        .contentShape(.rect)
    }
}
