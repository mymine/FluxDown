import FluxDomain
import FluxUI
import SwiftUI

/// 队列管理 Sheet 内的导航目的地。编辑按 `queueId` 解析，保证主机回流后表单头部实时更新。
nonisolated enum QueueRoute: Hashable {
    case create
    case edit(String)
}

/// D7 队列管理（02-downloads §9.1）：系统 Sheet（medium / large）内的 `NavigationStack`。
/// 行 = 运行状态点 + 名称（内置带徽标）+ 摘要 + 启停开关；点按推入 D8 表单；非内置队列可左滑删除（确认）。
/// 队列自身的顺序主机不支持写入，因此不提供重排。
struct QueueManagerSheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var model = QueueManagerModel()
    @State private var path: [QueueRoute] = []

    var body: some View {
        let state = container.store.state
        let queues = state.queues.sorted { $0.position < $1.position }
        let counts = QueueText.taskCounts(state.tasks)
        NavigationStack(path: $path) {
            List {
                if state.isReadOnly || model.errorText != nil {
                    Section {
                        VStack(spacing: 8) {
                            if state.isReadOnly {
                                Banner(text: L("localServiceDisconnected"), tone: .warning, systemImage: FluxSymbol.offline, slim: true)
                            }
                            if let text = model.errorText {
                                Banner(text: text, tone: .error, slim: true)
                            }
                        }
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                    }
                }
                if !queues.isEmpty {
                    Section {
                        ForEach(queues) { queue in
                            // 任务表分页载入中数量未知（nil），不能显示成 0。
                            row(queue, taskCount: state.tasksLoading ? nil : counts[normalizedQueueId(queue.queueId)] ?? 0, readOnly: state.isReadOnly)
                        }
                    } footer: {
                        Text(L("queueTasksOrderHint"))
                    }
                }
            }
            .overlay {
                if queues.isEmpty { emptyState(readOnly: state.isReadOnly) }
            }
            .navigationTitle(L("manageQueueAction"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("close"), systemImage: FluxSymbol.close) { dismiss() }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button(L("createQueueAction"), systemImage: FluxSymbol.add) { path.append(.create) }
                        .disabled(state.isReadOnly)
                }
            }
            .navigationDestination(for: QueueRoute.self) { route in
                switch route {
                case .create: QueueEditorView(queueId: nil)
                case let .edit(id): QueueEditorView(queueId: id)
                }
            }
            .alert(
                L("deleteQueueAction"),
                isPresented: Binding(
                    get: { model.pendingDelete != nil },
                    set: { if !$0 { model.pendingDelete = nil } }
                ),
                presenting: model.pendingDelete
            ) { queue in
                Button(L("cancel"), role: .cancel) {}
                Button(L("delete"), role: .destructive) { model.delete(queue, container: container) }
            } message: { queue in
                Text(L("queueDeleteConfirmDesc", ["name": queue.displayName]))
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationSizing(.form)
    }

    // MARK: 行

    private func row(_ queue: TaskQueue, taskCount: Int?, readOnly: Bool) -> some View {
        let builtin = QueueIds.isBuiltin(queue.queueId)
        let busy = model.busy.contains(queue.queueId)
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 10))
            : AnyLayout(HStackLayout(alignment: .center, spacing: 12))
        return layout {
            Button {
                path.append(.edit(queue.queueId))
            } label: {
                info(queue, taskCount: taskCount)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .combine)
            .accessibilityHint(L("editQueue"))

            HStack(spacing: 8) {
                Toggle(
                    queue.displayName,
                    isOn: Binding(
                        get: { queue.isRunning },
                        set: { model.setRunning($0, queue: queue, container: container) }
                    )
                )
                .labelsHidden()
                .disabled(readOnly || busy || (queue.isRunning && !queue.canStop))
                .accessibilityValue(L(queue.isRunning ? "queueRunningBadge" : "queueStoppedBadge"))
                if !typeSize.isAccessibilitySize {
                    Button {
                        path.append(.edit(queue.queueId))
                    } label: {
                        Image(systemName: "chevron.right")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(.tertiary)
                            .frame(minWidth: 20, minHeight: 44)
                            .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .accessibilityHidden(true)
                }
            }
        }
        // 删除只弹确认框：不用 `role: .destructive`（会让 List 先行移除该行而数据未变，导致行数不一致崩溃）。
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if !builtin {
                Button(L("delete"), systemImage: FluxSymbol.delete) { model.pendingDelete = queue }
                    .tint(Color.fdStatusFailed)
                    .disabled(readOnly || busy)
            }
        }
        .contextMenu {
            Button(L("editQueue"), systemImage: FluxSymbol.edit) { path.append(.edit(queue.queueId)) }
            if !queue.isRunning || queue.canStop {
                Button(
                    L(queue.isRunning ? "stopQueueAction" : "startQueueAction"),
                    systemImage: queue.isRunning ? FluxSymbol.pause : FluxSymbol.resume
                ) { model.setRunning(!queue.isRunning, queue: queue, container: container) }
                    .disabled(readOnly || busy)
            }
            if !builtin {
                Button(L("deleteQueueAction"), systemImage: FluxSymbol.delete, role: .destructive) { model.pendingDelete = queue }
                    .disabled(readOnly || busy)
            }
        }
    }

    private func info(_ queue: TaskQueue, taskCount: Int?) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(queue.displayName)
                    .font(.body)
                    .foregroundStyle(.primary)
                    .fixedSize(horizontal: false, vertical: true)
                if QueueIds.isBuiltin(queue.queueId) {
                    Text(L("queueBuiltinBadge"))
                        .font(.caption2.weight(.medium))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(.fill.tertiary, in: .capsule)
                }
            }
            QueueStateBadge(running: queue.isRunning)
            Text(QueueText.summary(queue, taskCount: taskCount))
                .font(.footnote)
                .foregroundStyle(.secondary)
                .monospacedDigit()
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private func emptyState(readOnly: Bool) -> some View {
        if readOnly {
            ContentUnavailableView(
                L("manageQueueAction"),
                systemImage: FluxSymbol.offline,
                description: Text(L("localServiceDisconnected"))
            )
        } else {
            ProgressView(L("mobileLoading"))
        }
    }
}
