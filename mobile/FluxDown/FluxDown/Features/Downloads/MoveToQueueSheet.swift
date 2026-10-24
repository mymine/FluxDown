import FluxDomain
import FluxUI
import SwiftUI

/// D5#7 移动到队列：单选列表，点选即提交（`TaskActions.moveToQueueNow`）。
/// 副标题 = 运行状态 · 任务数；所选任务都在同一队列时，该队列打勾。
/// 系统 Sheet（`presentationDetents`），不设 `presentationBackground`，保持系统玻璃。
struct MoveToQueueSheet: View {
    let taskIds: [String]

    @Environment(AppContainer.self) private var container
    @Environment(TaskActions.self) private var actions
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let state = container.store.state
        let queues = state.queues.sorted { $0.position < $1.position }
        let current = Self.currentQueue(of: taskIds, in: state.tasks)
        let counts = Self.counts(of: state.tasks)
        NavigationStack {
            List {
                Section {
                    ForEach(queues) { queue in
                        let key = normalizedQueueId(queue.queueId)
                        Button {
                            if key != current {
                                actions.moveToQueueNow(taskIds, queueId: queue.queueId, queueName: queue.displayName)
                            }
                            dismiss()
                        } label: {
                            HStack(spacing: 12) {
                                Image(systemName: FluxSymbol.queue)
                                    .foregroundStyle(.secondary)
                                    .frame(width: 28)
                                    .accessibilityHidden(true)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(queue.displayName).foregroundStyle(.primary)
                                    Text(subtitle(queue, count: state.tasksLoading ? nil : counts[key] ?? 0))
                                        .font(.footnote)
                                        .foregroundStyle(.secondary)
                                        .monospacedDigit()
                                }
                                Spacer(minLength: 0)
                                if key == current {
                                    Image(systemName: FluxSymbol.done)
                                        .fontWeight(.semibold)
                                        .foregroundStyle(.tint)
                                        .accessibilityHidden(true)
                                }
                            }
                            .contentShape(.rect)
                        }
                        .accessibilityAddTraits(key == current ? .isSelected : [])
                    }
                } header: {
                    Text(L("mobileSelectQueue"))
                }
            }
            .navigationTitle(L("moveToQueueAction"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("close"), systemImage: FluxSymbol.close) { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private func subtitle(_ queue: TaskQueue, count: Int?) -> String {
        let state = L(queue.isRunning ? "queueRunningBadge" : "queueStoppedBadge")
        // 任务表分页载入中数量未知：只显示运行状态。
        guard let count else { return state }
        return state + " · " + L("nTasks", ["n": count])
    }

    /// 所选任务当前所在队列（全部相同才有值）。
    static func currentQueue(of ids: [String], in tasks: [DownloadTask]) -> String? {
        let selected = Set(ids)
        let queues = Set(tasks.lazy.filter { selected.contains($0.taskId) }.map { normalizedQueueId($0.queueId) })
        return queues.count == 1 ? queues.first : nil
    }

    /// 各队列任务数（"" 与 `main` 同一队列）。
    static func counts(of tasks: [DownloadTask]) -> [String: Int] {
        var counts: [String: Int] = [:]
        for task in tasks { counts[normalizedQueueId(task.queueId), default: 0] += 1 }
        return counts
    }
}
