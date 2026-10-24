import FluxDomain
import FluxUI
import SwiftUI

/// D8 待处理任务顺序（`daemon.queue.reorder`）：队列启动时按此顺序开始下载。
/// 每行有上移 / 下移按钮（VoiceOver 友好），并支持「编辑」模式下拖动手柄（`.onMove`）。
/// 编辑模式由外层 `Form` 的 `editMode` 环境值驱动（本段不改其他行的外观）。
struct QueuePendingOrderSection: View {
    /// 主机侧顺序（`QueueOrdering.pending`）。
    let serverTasks: [DownloadTask]
    /// 本地提交后到主机回流前的顺序。
    let override: [String]?
    let readOnly: Bool
    /// 任务表分页载入中：`serverTasks` 是半份列表，不能显示成「没有待处理任务」。
    var loading = false
    @Binding var editMode: EditMode
    let onReorder: ([String]) -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var tasks: [DownloadTask] {
        guard let override else { return serverTasks }
        let byId = Dictionary(serverTasks.map { ($0.taskId, $0) }, uniquingKeysWith: { first, _ in first })
        let ordered = override.compactMap { byId[$0] }
        let known = Set(override)
        return ordered + serverTasks.filter { !known.contains($0.taskId) }
    }

    var body: some View {
        let tasks = tasks
        let ids = tasks.map(\.taskId)
        Section {
            if loading {
                HStack(spacing: 8) {
                    ProgressView()
                    Text(L("mobileTasksLoading"))
                }
                .foregroundStyle(.secondary)
                .accessibilityElement(children: .combine)
            } else if tasks.isEmpty {
                Text(L("queueNoPendingTasks"))
                    .foregroundStyle(.secondary)
            } else {
                ForEach(Array(tasks.enumerated()), id: \.element.taskId) { index, task in
                    row(task, index: index, ids: ids)
                }
                .onMove(perform: moveHandler(ids))
            }
        } header: {
            header(count: tasks.count)
        } footer: {
            Text(L("queueTasksOrderHint"))
        }
        .fluxAnimation(.smooth, value: ids)
    }

    /// 只读时不提供拖动（返回 nil → 无手柄）。
    private func moveHandler(_ ids: [String]) -> ((IndexSet, Int) -> Void)? {
        guard !readOnly else { return nil }
        return { from, to in onReorder(QueueOrdering.moved(ids, from: from, to: to)) }
    }

    private func header(count: Int) -> some View {
        HStack {
            Text(L("queueTabTasks"))
            Spacer(minLength: 8)
            if count > 1, !readOnly {
                Button(editMode.isEditing ? L("mobileViewDone") : L("menuEdit")) {
                    withFluxAnimation(.smooth, reduceMotion: reduceMotion) {
                        editMode = editMode.isEditing ? .inactive : .active
                    }
                }
                .font(.subheadline)
                .textCase(nil)
                .frame(minHeight: 44)
            }
        }
    }

    private func row(_ task: DownloadTask, index: Int, ids: [String]) -> some View {
        HStack(spacing: 4) {
            Text(task.fileName.isEmpty ? task.url : task.fileName)
                .lineLimit(2)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            if !editMode.isEditing {
                moveButton(systemImage: "chevron.up", label: L("moveUpAction"), enabled: index > 0) {
                    QueueOrdering.moved(ids, index: index, by: -1)
                }
                moveButton(systemImage: "chevron.down", label: L("moveDownAction"), enabled: index < ids.count - 1) {
                    QueueOrdering.moved(ids, index: index, by: 1)
                }
            }
        }
    }

    private func moveButton(
        systemImage: String,
        label: String,
        enabled: Bool,
        order: @escaping () -> [String]?
    ) -> some View {
        Button {
            if let next = order() { onReorder(next) }
        } label: {
            Image(systemName: systemImage)
                .font(.body.weight(.semibold))
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(.rect)
        }
        .buttonStyle(.borderless)
        .disabled(!enabled || readOnly)
        .accessibilityLabel(label)
    }
}
