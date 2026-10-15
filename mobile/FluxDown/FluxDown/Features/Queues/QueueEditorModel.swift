import FluxDomain
import FluxUI
import SwiftUI

/// D8 队列表单状态与写操作（新建 / 编辑）。
///
/// - 完整 `QueueDetail` 来自 `daemon.queue.list`（快照里的 `TaskQueue` 缺 `defaultSegments` / `defaultUserAgent`，
///   直接回填会在保存时把这两项清零）。表单未被改动时跟随主机回流刷新；已编辑则保留用户输入。
/// - 保存 = Web `QueueEditor.save`：编辑 `update` → `schedule`；新建 `create` →（需要定时）`list` 按名称定位新队列 → `schedule`。
@MainActor
@Observable
final class QueueEditorModel {
    nonisolated enum LoadState: Equatable {
        case loading
        case loaded
        /// 主机上已找不到该队列（被删除）。
        case missing
        case failed(String)
    }

    /// nil = 新建。
    let queueId: String?
    var draft = QueueDraft()
    private(set) var detail: QueueDetail?
    private(set) var loadState: LoadState
    private(set) var isSaving = false
    /// 启停 / 删除 / 重排进行中。
    private(set) var isWorking = false
    /// 校验错误（行内提示）。
    var issue: QueueDraft.Issue?
    /// 写操作失败文案（表单顶部横幅）。
    var errorText: String?
    /// 拖动 / 箭头重排后到主机回流前的本地顺序（避免行先弹回再移动）；失败即清除。
    var orderOverride: [String]?

    @ObservationIgnored private var baseline = QueueDraft()
    @ObservationIgnored private var generation = 0

    init(queueId: String?) {
        self.queueId = queueId
        loadState = queueId == nil ? .loaded : .loading
    }

    var isBuiltin: Bool { queueId.map(QueueIds.isBuiltin) ?? false }
    var isCreating: Bool { queueId == nil }

    // MARK: 加载

    /// 拉取完整队列；快照队列变化时再次调用。过期结果（后发先至）丢弃。
    func reload(session: any HostSession) async {
        guard let queueId else { return }
        generation += 1
        let ticket = generation
        if detail == nil { loadState = .loading }
        do throws(HostError) {
            let list: [QueueDetail] = try await session.call(HostMethod.daemonQueueList)
            guard ticket == generation else { return }
            if let found = list.first(where: { $0.queueId == queueId }) {
                apply(found)
            } else {
                loadState = .missing
            }
        } catch {
            guard ticket == generation, detail == nil else { return }
            loadState = .failed(ErrorText.describe(error))
        }
    }

    private func apply(_ found: QueueDetail) {
        let fresh = QueueDraft(queue: found)
        if detail == nil || draft == baseline { draft = fresh }
        baseline = fresh
        detail = found
        loadState = .loaded
    }

    // MARK: 保存

    /// 成功返回 true；校验失败 / 主机失败返回 false（`issue` / `errorText` 已设置）。
    func save(session: any HostSession) async -> Bool {
        guard !isSaving else { return false }
        issue = nil
        errorText = nil
        let form = draft
        switch form.validated(builtin: isBuiltin, existingName: detail?.name) {
        case let .failure(found):
            issue = found
            FluxHaptic.error.play()
            return false
        case let .success(input):
            isSaving = true
            defer { isSaving = false }
            do throws(HostError) {
                if let queueId {
                    try await session.callVoid(HostMethod.daemonQueueUpdate, params: QueueUpdateParams(queueId: queueId, input: input))
                    if form.needsScheduleCall(queueId: queueId) {
                        try await session.callVoid(HostMethod.daemonQueueSchedule, params: form.schedule(queueId: queueId))
                    }
                } else {
                    try await session.callVoid(HostMethod.daemonQueueCreate, params: input)
                    // create 不回 id：需要定时时按名称取最新创建的队列再补发。
                    if form.needsScheduleCall(queueId: "") {
                        let list: [QueueDetail] = try await session.call(HostMethod.daemonQueueList)
                        if let created = QueueLookup.created(named: input.name, in: list) {
                            try await session.callVoid(HostMethod.daemonQueueSchedule, params: form.schedule(queueId: created.queueId))
                        }
                    }
                }
                FluxHaptic.success.play()
                return true
            } catch {
                errorText = ErrorText.describe(error)
                FluxHaptic.error.play()
                return false
            }
        }
    }

    // MARK: 启停 / 删除 / 重排

    /// 启动或停止当前队列；运行状态经快照回流（不乐观更新）。成功返回 true。
    func setRunning(_ running: Bool, session: any HostSession) async -> Bool {
        guard let queueId, !isWorking else { return false }
        isWorking = true
        errorText = nil
        defer { isWorking = false }
        do throws(HostError) {
            try await session.callVoid(
                running ? HostMethod.daemonQueueStart : HostMethod.daemonQueueStop,
                params: QueueIdParams(queueId: queueId)
            )
            return true
        } catch {
            errorText = ErrorText.describe(error)
            FluxHaptic.error.play()
            return false
        }
    }

    /// 删除当前（非内置）队列。成功返回 true。
    func delete(session: any HostSession) async -> Bool {
        guard let queueId, !isBuiltin, !isWorking else { return false }
        isWorking = true
        errorText = nil
        defer { isWorking = false }
        do throws(HostError) {
            try await session.callVoid(HostMethod.daemonQueueDelete, params: QueueIdParams(queueId: queueId))
            return true
        } catch {
            errorText = ErrorText.describe(error)
            FluxHaptic.error.play()
            return false
        }
    }

    /// `daemon.queue.reorder`：提交队列内待下载任务的完整新顺序。
    func reorder(_ ids: [String], session: any HostSession) async {
        guard let queueId else { return }
        orderOverride = ids
        errorText = nil
        do throws(HostError) {
            try await session.callVoid(HostMethod.daemonQueueReorder, params: QueueReorderParams(queueId: queueId, taskIds: ids))
        } catch {
            orderOverride = nil
            errorText = ErrorText.describe(error)
            FluxHaptic.error.play()
        }
    }
}
