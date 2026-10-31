import FluxDomain
import Foundation
import Testing

/// 与 Android `HostStoreTest` 覆盖同一组 reducer 规则（删除哨兵 / 旧采样丢弃 / 非活跃清段 / Stale 只读 / 合帧）。
@MainActor
struct HostStoreTests {
    private func task(_ id: String, _ status: TaskStatus) -> DownloadTask {
        DownloadTask(taskId: id, url: "https://x/\(id)", fileName: "\(id).bin", saveDir: "/d", status: status, downloadedBytes: 50, totalBytes: 100)
    }

    private func seg(_ i: Int32, _ active: Bool?) -> Segment {
        Segment(index: i, startByte: Int64(i) * 10, endByte: Int64(i) * 10 + 9, downloadedBytes: 5, active: active)
    }

    private func runtime(_ id: String, _ seq: UInt64, _ segs: [Segment], transfers: UInt32 = 2) -> TaskRuntime {
        TaskRuntime(taskId: id, sampleSequence: seq, activeTransfers: transfers, connectedPeers: nil, totalBytes: 100, segments: segs)
    }

    private func snapshot(_ tasks: [DownloadTask], runtime: [String: TaskRuntime] = [:]) -> HostSignal {
        .snapshot(HostSnapshot(
            info: HostInfo(serviceName: "t", serviceVersion: "1", protocolVersion: 7, capabilities: []),
            tasks: tasks,
            runtime: runtime,
            configRevision: 1
        ))
    }

    private func progress(_ id: String, _ status: TaskStatus, speed: Int64, error: String = "") -> HostSignal {
        .event(.taskProgress(TaskProgress(taskId: id, status: status.rawValue, downloadedBytes: 60, totalBytes: 100, speed: speed, errorMessage: error)))
    }

    /// 合帧间隔设为一小时：非结构性事件只能经 `flush()` 发布，测试不依赖时序。
    private func store() -> HostStore { HostStore(clock: { 1_000_000 }, publishInterval: .seconds(3600)) }

    @Test func progressWithDeletedSentinelRemovesTaskAndRuntime() {
        let s = store()
        s.apply(snapshot([task("a", .downloading)], runtime: ["a": runtime("a", 1, [seg(0, true)])]))
        s.apply(progress("a", .failed, speed: 0, error: "deleted"))
        #expect(s.state.task("a") == nil)
        #expect(s.state.runtime["a"] == nil)
    }

    @Test func staleRuntimeSampleIsDroppedAndEmptySegmentsKeepPrevious() {
        let s = store()
        s.apply(snapshot([task("a", .downloading)], runtime: ["a": runtime("a", 5, [seg(0, true), seg(1, true)])]))
        s.apply(.event(.taskRuntimeChanged(runtime("a", 4, [seg(0, false)]))))
        s.flush()
        #expect(s.state.runtime["a"]?.segments.count == 2)

        s.apply(.event(.taskRuntimeChanged(runtime("a", 6, [], transfers: 1))))
        s.flush()
        let rt = s.state.runtime["a"]
        #expect(rt?.sampleSequence == 6)
        #expect(rt?.segments.count == 2)
        #expect(rt?.activeTransfers == 1)
    }

    @Test func pausingClearsActiveSegmentsAndLiveSpeed() {
        let s = store()
        s.apply(snapshot([task("a", .downloading)], runtime: ["a": runtime("a", 1, [seg(0, true), seg(1, nil)])]))
        s.apply(progress("a", .downloading, speed: 4096))
        s.flush()
        #expect(s.state.speed("a").down == 4096)

        s.apply(progress("a", .paused, speed: 4096))
        let st = s.state
        #expect(st.task("a")?.status == .paused)
        #expect(st.speed("a").down == 0)
        #expect(st.runtime["a"]?.segments.allSatisfy { $0.active == false } == true)
        #expect(st.runtime["a"]?.activeTransfers == 0)
    }

    @Test func completedTasksLeaveNoRuntimeAndLateSamplesCannotReaddIt() {
        let s = store()
        s.apply(snapshot([]))
        for index in 0..<600 {
            let id = "t\(index)"
            s.apply(.event(.taskChanged(task(id, .downloading))))
            s.apply(.event(.taskRuntimeChanged(runtime(id, UInt64(index) + 1, [seg(0, true)]))))
            s.flush()
            #expect(Set(s.state.runtime.keys) == [id])
            // 两条完成路径：taskChanged / taskProgress。
            if index % 2 == 0 {
                s.apply(.event(.taskChanged(task(id, .completed))))
            } else {
                s.apply(progress(id, .completed, speed: 0))
            }
            s.flush()
            #expect(s.state.runtime.isEmpty)
            // 迟到的采样（序号更高也一样）不得把条目加回来。
            s.apply(.event(.taskRuntimeChanged(runtime(id, UInt64(index) + 10_000, [seg(0, true)]))))
            s.flush()
            #expect(s.state.runtime.isEmpty)
        }
        #expect(s.state.tasks.count == 600)
    }

    @Test func snapshotRuntimeOfCompletedTasksIsDroppedButPausedAndFailedKeepSegments() {
        let s = store()
        let ids = ["run", "paused", "failed", "done", "gone"]
        s.apply(snapshot(
            [task("run", .downloading), task("paused", .paused), task("failed", .failed), task("done", .completed)],
            runtime: Dictionary(uniqueKeysWithValues: ids.map { ($0, runtime($0, 1, [seg(0, true)])) })
        ))
        #expect(Set(s.state.runtime.keys) == ["run", "paused", "failed"])
        s.apply(progress("paused", .completed, speed: 0))
        s.apply(progress("failed", .paused, speed: 0))
        s.flush()
        #expect(Set(s.state.runtime.keys) == ["run", "failed"])
        #expect(s.state.runtime["failed"]?.segments.allSatisfy { $0.active == false } == true)
    }

    @Test func staleSignalMakesStateReadOnlyAndDropsRuntimeAndSpeeds() {
        let s = store()
        s.apply(snapshot([task("a", .downloading)], runtime: ["a": runtime("a", 1, [seg(0, true)])]))
        s.apply(progress("a", .downloading, speed: 100))
        s.apply(.stale)
        let st = s.state
        #expect(st.isReadOnly)
        #expect(st.runtime.isEmpty)
        #expect(st.speeds.isEmpty)
        #expect(st.tasks.count == 1)
    }

    @Test func highFrequencyProgressIsCoalescedIntoOnePublish() {
        let s = store()
        s.apply(snapshot([task("a", .downloading)]))
        for i in 1...20 { s.apply(progress("a", .downloading, speed: Int64(i))) }
        #expect(s.state.speeds["a"] == nil, "non-structural progress must not publish synchronously")
        s.flush()
        #expect(s.state.speed("a").down == 20)
    }

    @Test func resyncSnapshotKeepsInsertionOrderAndSpeedHistory() {
        let s = store()
        s.apply(snapshot([task("a", .downloading), task("b", .paused)]))
        s.apply(.event(.taskChanged(task("c", .pending))))
        #expect(s.state.tasks.map(\.taskId) == ["a", "b", "c"])
        s.apply(.event(.runtimeStatsChanged(RuntimeStats(totalDownloadBps: 512))))
        s.apply(snapshot([task("a", .downloading)]))
        #expect(s.state.tasks.map(\.taskId) == ["a"])
        #expect(s.state.speedHistory.latestDown == 512)
    }

    private func keys(_ pairs: [(String, Int64)]) -> [QueueOrderEntry] {
        pairs.map { QueueOrderEntry(taskId: $0.0, order: $0.1) }
    }

    @Test func queueOrderDeltaKeepsRanksByKeyAndDoesNotRekeyOthers() {
        let s = store()
        s.apply(snapshot([task("a", .pending), task("b", .pending), task("c", .pending)]))
        s.apply(.event(.queuePositionsChanged(keys([("a", 10), ("b", 20), ("c", 30)]))))
        s.flush()
        #expect(["a", "b", "c"].map { s.state.queueOrder.rank(of: $0) } == [1, 2, 3])

        // 队首出队：其余任务的键不变，名次整体前移。
        s.apply(.event(.queueOrderChanged(removed: ["a"], added: [])))
        s.flush()
        #expect(["a", "b", "c"].map { s.state.queueOrder.rank(of: $0) } == [0, 1, 2])
        #expect(s.state.queueOrder.order(of: "b") == 20)

        // 新增排在键的名次位置；同任务多次出现以最后一项为准；已有任务按新键移动。
        s.apply(.event(.queueOrderChanged(removed: [], added: keys([("d", 5), ("d", 25), ("b", 40)]))))
        s.flush()
        #expect(s.state.queueOrder.entries.map(\.taskId) == ["d", "c", "b"])
        #expect(["d", "c", "b"].map { s.state.queueOrder.rank(of: $0) } == [1, 2, 3])

        // 全量消息整表替换。
        s.apply(.event(.queuePositionsChanged(keys([("c", 1), ("b", 2)]))))
        s.flush()
        #expect(["d", "b", "c"].map { s.state.queueOrder.rank(of: $0) } == [0, 2, 1])
    }

    // MARK: 任务表分页载入（tasksLoading）

    /// Rust 驱动在分页拉取期间先发的「加载态」快照：任务表为空占位，只带服务端任务总数。
    private func loadingSnapshot(total: UInt64) -> HostSignal {
        .snapshot(HostSnapshot(
            info: HostInfo(serviceName: "t", serviceVersion: "1", protocolVersion: 17, capabilities: []),
            tasksLoading: true,
            taskTotal: total,
            configRevision: 1
        ))
    }

    @Test func defaultSnapshotIsACompleteTaskList() {
        let s = store()
        s.apply(snapshot([task("a", .paused)]))
        #expect(!s.state.tasksLoading)
        #expect(s.state.taskTotal == 0)
    }

    /// 加载态快照：空占位不是完整列表（`tasksLoading`），带总数供进度文案；连接仍视为在线。
    @Test func loadingSnapshotMarksTheTaskListIncompleteAndCarriesTheTotal() {
        let s = store()
        s.apply(loadingSnapshot(total: 230_000))
        let st = s.state
        #expect(st.tasksLoading)
        #expect(st.taskTotal == 230_000)
        #expect(st.tasks.isEmpty)
        #expect(st.connection == .live)
    }

    /// 载入完成的完整快照整表替换占位并清除加载态；分页期间到达、载入后回放的事件不丢、不重复。
    @Test func completedSnapshotAfterLoadingKeepsReplayedChangesWithoutLossOrDuplication() {
        let s = store()
        s.apply(loadingSnapshot(total: 3))
        // 完整快照 = 载入时刻的表（页取自 base 之后某时刻的状态）。
        s.apply(snapshot([task("a", .downloading), task("b", .paused), task("c", .pending)]))
        #expect(!s.state.tasksLoading)
        #expect(s.state.taskTotal == 0)
        #expect(s.state.tasks.map(\.taskId) == ["a", "b", "c"])

        // 缓冲帧按游标回放：已翻页之后才发生的变更（更新 / 新增 / 删除）。
        s.apply(.event(.taskChanged(task("b", .downloading))))
        s.apply(.event(.taskChanged(task("d", .pending))))
        s.apply(.event(.taskChanged(task("a", .completed))))
        s.apply(.event(.taskDeleted(taskId: "c")))
        // 同一帧被回放两次（整行写入幂等）：既不丢也不重复。
        s.apply(.event(.taskChanged(task("d", .pending))))
        s.flush()

        let tasks = s.state.tasks
        #expect(tasks.map(\.taskId) == ["a", "b", "d"])
        #expect(Set(tasks.map(\.taskId)).count == tasks.count)
        #expect(s.state.task("a")?.status == .completed)
        #expect(s.state.task("b")?.status == .downloading)
        #expect(s.state.task("c") == nil)
        #expect(!s.state.tasksLoading)
    }

    /// 重同步 / 整表重载再次进入加载态：旧任务表被清空但这不是「任务都被删了」——
    /// 速度曲线保留，完整快照到达后仍存在的任务继续接着画。
    @Test func reloadingKeepsTaskSpeedHistoryUntilTheCompleteSnapshotArrives() {
        let s = store()
        s.apply(snapshot([task("a", .downloading), task("gone", .downloading)]))
        s.apply(progress("a", .downloading, speed: 100))
        s.apply(progress("gone", .downloading, speed: 50))
        s.flush()
        #expect(s.state.taskSpeedHistory["a"] != nil)

        s.apply(loadingSnapshot(total: 2))
        #expect(s.state.tasksLoading && s.state.tasks.isEmpty)
        #expect(s.state.taskSpeedHistory["a"] != nil, "an empty placeholder is not a deletion")
        #expect(s.state.taskSpeedHistory["gone"] != nil)

        s.apply(snapshot([task("a", .downloading)]))
        #expect(!s.state.tasksLoading)
        #expect(s.state.taskSpeedHistory["a"] != nil)
        #expect(s.state.taskSpeedHistory["gone"] == nil, "the complete list decides what was deleted")
    }

    /// 载入期间断线：仍是加载态（半份列表不会因此变成完整列表），只读。
    @Test func staleWhileLoadingStaysIncomplete() {
        let s = store()
        s.apply(loadingSnapshot(total: 9))
        s.apply(.stale)
        #expect(s.state.isReadOnly)
        #expect(s.state.tasksLoading)
        #expect(s.state.taskTotal == 9)
    }

    // MARK: 通用分区 / 通知通道

    private struct Gateway: Decodable, Equatable { var takeoverEnabled: Bool }

    private func json(_ text: String) -> Data { Data(text.utf8) }

    @Test func snapshotSectionsAreDecodableAndSectionChangedReplacesThemImmediately() {
        let s = store()
        s.apply(.snapshot(HostSnapshot(
            info: HostInfo(serviceName: "t", serviceVersion: "1", protocolVersion: 7, capabilities: []),
            sections: [HostSection.agentGateway: json(#"{"takeoverEnabled":false}"#)]
        )))
        #expect(s.state.section(HostSection.agentGateway, as: Gateway.self) == Gateway(takeoverEnabled: false))
        #expect(s.state.section(HostSection.agentShell, as: Gateway.self) == nil, "missing section")

        // 结构性：不经 flush 立即可见。
        s.apply(.event(.sectionChanged(name: HostSection.agentGateway, json: json(#"{"takeoverEnabled":true}"#))))
        #expect(s.state.section(HostSection.agentGateway, as: Gateway.self) == Gateway(takeoverEnabled: true))

        // 解码失败（形状不符）不崩溃，返回 nil。
        s.apply(.event(.sectionChanged(name: HostSection.agentGateway, json: json("[1,2]"))))
        #expect(s.state.section(HostSection.agentGateway, as: Gateway.self) == nil)

        // 重同步快照整表替换分区。
        s.apply(.snapshot(HostSnapshot(
            info: HostInfo(serviceName: "t", serviceVersion: "1", protocolVersion: 7, capabilities: []),
            sections: [HostSection.agentShell: json("{}")]
        )))
        #expect(s.state.sections.keys.sorted() == [HostSection.agentShell])
    }

    @Test func noticesAreLoggedWithMonotonicIdsAndBoundedToTheLastFifty() {
        let s = store()
        s.apply(snapshot([]))
        let before = s.state.sections
        for i in 1...(HostStore.noticeLimit + 7) {
            s.apply(.event(.notice(name: HostNoticeName.captureTasksStarted, json: json("[\"t\(i)\"]"))))
        }
        #expect(s.notices.count == HostStore.noticeLimit)
        #expect(s.notices.first?.id == 8)
        #expect(s.notices.last?.id == HostStore.noticeLimit + 7)
        #expect(zip(s.notices, s.notices.dropFirst()).allSatisfy { $0.id + 1 == $1.id })
        #expect(s.notices.last?.decode([String].self) == ["t\(HostStore.noticeLimit + 7)"])
        #expect(s.notices.last?.name == "captureTasksStarted")
        #expect(s.state.sections == before, "notices never touch published state")
    }

    // MARK: 会话信号流结束（与 Android `HostStoreTest` 同组）

    private final class Scripted: HostSession, @unchecked Sendable {
        let signals: AsyncStream<HostSignal>
        init(_ signals: AsyncStream<HostSignal>) { self.signals = signals }
        func call(_: String, params _: Data?) async throws(HostError) -> Data { Data("null".utf8) }
        func close() {}
        func categorySaveDir(fileName _: String, url _: String) -> String? { nil }
        func createTask(_: CreateTaskRequest) async throws(HostError) -> String { "" }
        func pause(_: String) async throws(HostError) {}
        func resume(_: String) async throws(HostError) {}
        func delete(_: String, deleteFiles _: Bool) async throws(HostError) {}
        func pauseMany(_: [String]) async throws(HostError) {}
        func resumeMany(_: [String]) async throws(HostError) {}
        func deleteMany(_: [String], deleteFiles _: Bool) async throws(HostError) {}
        func pauseAll() async throws(HostError) {}
        func resumeAll() async throws(HostError) {}
        func rename(_: String, fileName _: String) async throws(HostError) {}
        func changeUrl(_: String, url _: String) async throws(HostError) {}
        func rescan() async throws(HostError) {}
        func moveToQueue(_: String, queueId _: String) async throws(HostError) {}
        func boost(_: String) async throws(HostError) {}
        func resolveSelection(_: String, outcome _: SelectionOutcome) async throws(HostError) {}
        func patchConfig(expectedRevision _: UInt64, values _: [String: String]) async throws(HostError) {}
        func refreshRssSource(_: String) async throws(HostError) {}
        func setRssSourceEnabled(_: String, enabled _: Bool) async throws(HostError) {}
    }

    private func finished(_ signals: [HostSignal]) -> AsyncStream<HostSignal> {
        AsyncStream { continuation in
            for signal in signals { continuation.yield(signal) }
            continuation.finish()
        }
    }

    private func failure(_ s: HostStore) async -> HostError? {
        for _ in 0..<200 {
            if case let .failed(error) = s.state.connection { return error }
            try? await Task.sleep(for: .milliseconds(5))
        }
        return nil
    }

    @Test func signalStreamEndingWithoutFatalSurfacesAFailureInsteadOfFreezingOnLoading() async {
        let s = store()
        s.attach(Scripted(finished([loadingSnapshot(total: 1)])))
        defer { s.detach() }
        #expect(await failure(s)?.code == .unavailable)
    }

    @Test func fatalBeforeStreamEndKeepsTheRealReason() async {
        let s = store()
        let fatal = HostError(.unauthorized, message: "revoked")
        s.attach(Scripted(finished([snapshot([]), .fatal(fatal)])))
        defer { s.detach() }
        #expect(await failure(s) == fatal)
        try? await Task.sleep(for: .milliseconds(50))
        #expect(s.state.connection == .failed(fatal))
    }

    @Test func detachDoesNotReportTheCancelledStreamAsFailure() async {
        let s = store()
        let (stream, continuation) = AsyncStream<HostSignal>.makeStream()
        continuation.yield(snapshot([]))
        s.attach(Scripted(stream))
        for _ in 0..<200 where s.state.connection != .live {
            try? await Task.sleep(for: .milliseconds(5))
        }
        s.detach()
        continuation.finish()
        try? await Task.sleep(for: .milliseconds(50))
        #expect(s.state.connection == .live)
    }
}
