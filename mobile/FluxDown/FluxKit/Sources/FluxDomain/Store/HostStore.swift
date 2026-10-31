import Foundation
import Observation

/// 连接状态（断连横幅 / 只读态据此判定）。
public enum Connection: Sendable, Equatable {
    case connecting
    case live
    /// 800ms 宽限后仍未恢复：只读。
    case stale
    case failed(HostError)
}

/// 单任务实时量：只来自 TaskProgress 事件，非下载中下行为 0。
public struct LiveSpeed: Sendable, Hashable {
    public var down: Int64
    public var up: Int64

    public init(down: Int64, up: Int64) {
        self.down = down
        self.up = up
    }

    public static let zero = LiveSpeed(down: 0, up: 0)
}

/// UI 观察的不可变主机状态。
public struct HostState: Sendable {
    public var connection: Connection = .connecting
    public var info: HostInfo?
    /// 按创建顺序（快照顺序 + 新增追加）。
    public var tasks: [DownloadTask] = []
    /// 任务表尚在分页载入：`tasks` 只是占位 / 半份列表，**不是完整列表**。UI 只能显示加载态
    /// （进度文案可用 ``taskTotal``），任务计数、空态、批量操作范围、「任务已不存在」的删除推断都不得基于 `tasks`。
    public var tasksLoading = false
    /// ``tasksLoading`` 期间服务端的任务总数；载入完成后为 0。
    public var taskTotal: UInt64 = 0
    public var runtime: [String: TaskRuntime] = [:]
    public var speeds: [String: LiveSpeed] = [:]
    public var queues: [TaskQueue] = []
    /// 排队顺序（按引擎排序键的名次 = 「排队 #n」）；顺序未变时前后状态共享同一份存储。
    public var queueOrder = QueueOrder.empty
    public var groups: [DownloadGroup] = []
    public var stats = RuntimeStats()
    public var priorityTaskId: String?
    public var selections: [SelectionRequest] = []
    public var speedHistory = SpeedHistory.empty
    public var taskSpeedHistory: [String: SpeedHistory] = [:]
    public var config: [String: String] = [:]
    public var configRevision: UInt64 = 0
    public var rssSources: [RssSource] = []
    public var cloudDevices: [CloudDevice] = []
    public var linkDevices: [LinkDevice] = []
    public var categories: [TaskCategory] = TaskCategory.builtin
    /// 通用分区（键见 ``HostSection``）：协议 serde wire 的 JSON，按需 `section(_:as:)` 解码。
    public var sections: [String: Data] = [:]

    public init() {}

    public var isReadOnly: Bool { connection != .live }

    public func task(_ id: String) -> DownloadTask? { tasks.first { $0.taskId == id } }

    public func speed(_ id: String) -> LiveSpeed { speeds[id] ?? .zero }

    /// 能力门控：主机不支持的分区整段隐藏（README §2.1）。未连上时一律 false。
    public func has(_ capability: String) -> Bool { info?.has(capability) ?? false }

    /// 解码一个通用分区（键见 ``HostSection``）；分区缺失或解码失败为 nil。
    /// 每次调用都会解码，列表型分区请在视图模型里缓存结果。
    public func section<T: Decodable>(_ key: String, as type: T.Type = T.self) -> T? {
        guard let data = sections[key] else { return nil }
        return try? ProtocolJSON.makeDecoder().decode(T.self, from: data)
    }
}

/// 一条一次性通知（`HostEvent.notice`）：不进快照，按到达顺序记入 ``HostStore/notices``。
public struct HostNotice: Sendable, Hashable, Identifiable {
    /// store 内单调递增的序号（从 1 起）。
    public let id: Int
    /// serde 变体名，见 ``HostNoticeName``。
    public let name: String
    /// 载荷 JSON（协议 serde wire）。
    public let json: Data

    public init(id: Int, name: String, json: Data) {
        self.id = id
        self.name = name
        self.json = json
    }

    /// 解码载荷；失败为 nil。
    public func decode<T: Decodable>(_ type: T.Type = T.self) -> T? {
        try? ProtocolJSON.makeDecoder().decode(T.self, from: json)
    }
}

/// 主机投影仓库：按 `native/protocol` 的 reducer 规则（`event.rs` apply_daemon_event /
/// apply_engine_message）应用信号，发布不可变 `HostState`（与 Android `HostStore.kt` 同规则）。
///
/// 性能：事件就地合并进工作副本，发布按 `publishInterval` 合帧（结构性信号立即发布），
/// 高频 TaskProgress 不会造成逐事件的整表拷贝与视图失效。全部在主 actor 上串行执行。
@MainActor
@Observable
public final class HostStore {
    public private(set) var state = HostState()
    /// 最近的一次性通知（最多 ``noticeLimit`` 条，旧→新）。`id` 在 store 生命周期内单调递增（切换主机
    /// 清空日志但不重置计数）；观察者记住已处理的最大 `id`，处理 `id` 更大的条目即可不漏不重。
    public private(set) var notices: [HostNotice] = []

    @ObservationIgnored private var nextNoticeId = 0

    @ObservationIgnored private var w = Working()
    @ObservationIgnored private var publishTask: Task<Void, Never>?
    @ObservationIgnored private var sessionTask: Task<Void, Never>?
    @ObservationIgnored private let clock: @Sendable () -> Int64
    @ObservationIgnored private let publishInterval: Duration

    /// 通知日志保留条数。
    public nonisolated static let noticeLimit = 50

    public init(
        clock: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) },
        publishInterval: Duration = .milliseconds(100)
    ) {
        self.clock = clock
        self.publishInterval = publishInterval
    }

    /// 绑定一个主机会话（切换主机时先 `detach` 再绑定新会话）。
    public func attach(_ session: any HostSession) {
        detach()
        w = Working()
        state = HostState()
        notices = []
        let signals = session.signals
        sessionTask = Task { [weak self] in
            var fatalSeen = false
            for await signal in signals {
                guard let self, !Task.isCancelled else { return }
                if case .fatal = signal { fatalSeen = true }
                self.apply(signal)
            }
            // 信号流结束：被 detach 取消时不改状态；Rust 先发 fatal 再结束则保持该原因。
            // 其余（会话驱动意外退出）不能停在最后一帧不报错——那可能是加载态骨架，界面会一直转圈。
            guard let self, !Task.isCancelled, !fatalSeen else { return }
            self.apply(.fatal(HostError(.unavailable, message: "host session ended unexpectedly")))
        }
    }

    public func detach() {
        sessionTask?.cancel()
        sessionTask = nil
        publishTask?.cancel()
        publishTask = nil
    }

    /// 测试与同步场景：直接应用一个信号。
    public func apply(_ signal: HostSignal) {
        switch signal {
        case let .snapshot(snapshot):
            w = Working(snapshot: snapshot, previous: w)
            w.connection = .live
            publishNow()
        case let .event(.notice(name, json)):
            recordNotice(name: name, json: json)
        case let .event(event):
            if applyEvent(event) { publishNow() } else { schedulePublish() }
        case .stale:
            w.connection = .stale
            w.runtime.removeAll()
            w.speeds.removeAll()
            publishNow()
        case let .fatal(error):
            w.connection = .failed(error)
            w.speeds.removeAll()
            publishNow()
        }
    }

    /// 立即发布挂起的合帧（测试用）。
    public func flush() { publishNow() }

    private func recordNotice(name: String, json: Data) {
        nextNoticeId += 1
        notices.append(HostNotice(id: nextNoticeId, name: name, json: json))
        if notices.count > Self.noticeLimit { notices.removeFirst(notices.count - Self.noticeLimit) }
    }

    /// - Returns: true = 结构性变化（立即发布）。
    private func applyEvent(_ event: HostEvent) -> Bool {
        switch event {
        case let .taskChanged(task):
            w.upsert(task)
            settleRuntime(task.taskId, status: task.status)
            return true
        case let .taskDeleted(taskId):
            removeTask(taskId)
            return true
        case let .taskProgress(progress):
            return applyProgress(progress)
        case let .taskRuntimeChanged(runtime):
            applyRuntime(runtime)
        case let .queuesChanged(queues):
            w.queues = queues
            return true
        case let .queuePositionsChanged(entries):
            w.queueEntries = entries
        case let .queueOrderChanged(removed, added):
            w.queueEntries = QueueOrder.applying(removed: removed, added: added, to: w.queueEntries)
        case let .groupsChanged(groups):
            w.groups = groups
            return true
        case let .fileMissingChanged(updates):
            for (id, missing) in updates where w.tasks[id] != nil {
                w.tasks[id]?.fileMissing = missing
            }
        case let .priorityTaskChanged(taskId):
            w.priorityTaskId = (taskId?.isEmpty ?? true) ? nil : taskId
        case let .runtimeStatsChanged(stats):
            w.stats = stats
            w.history = w.history.recording(nowMs: clock(), down: stats.totalDownloadBps, up: stats.totalUploadBps)
        case let .daemonConnectionChanged(connected):
            if !connected {
                w.runtime.removeAll()
                w.speeds.removeAll()
            }
            return true
        case let .selectionPending(request):
            w.selections.removeAll { $0.requestId == request.requestId }
            w.selections.append(request)
            return true
        case let .selectionResolved(requestId):
            w.selections.removeAll { $0.requestId == requestId }
            return true
        case let .configChanged(values, revision):
            w.config = values
            w.configRevision = revision
            return true
        case let .rssSourcesChanged(sources):
            w.rssSources = sources
            return true
        case let .cloudDevicesChanged(devices):
            w.cloudDevices = devices
            return true
        case let .linkedDevicesChanged(devices):
            w.linkDevices = devices
            return true
        case let .categoriesChanged(categories):
            w.categories = categories
            return true
        case let .sectionChanged(name, json):
            w.sections[name] = json
            return true
        case .notice:
            // 由 `apply` 单独记入 `notices`，不触碰 `state`。
            return false
        }
        return false
    }

    private func applyProgress(_ e: TaskProgress) -> Bool {
        // status == 4 && errorMessage == "deleted" 表示任务已被删除。
        if e.status == TaskStatus.failed.rawValue, e.errorMessage == "deleted" {
            removeTask(e.taskId)
            return true
        }
        guard let previous = w.tasks[e.taskId] else { return false }
        let status = TaskStatus(wire: e.status)
        settleRuntime(e.taskId, status: status)
        var next = previous
        next.status = status
        next.downloadedBytes = e.downloadedBytes
        next.totalBytes = e.totalBytes
        next.errorMessage = e.errorMessage
        next.uploadedBytes = e.uploadedBytes
        next.seedingStatus = SeedingStatus(wire: e.seedingStatus)
        if !e.fileName.isEmpty { next.fileName = e.fileName }
        w.tasks[e.taskId] = next
        let down = status == .downloading ? e.speed : 0
        w.speeds[e.taskId] = LiveSpeed(down: down, up: e.uploadSpeed)
        let history = w.taskHistory[e.taskId] ?? .empty
        w.taskHistory[e.taskId] = history.recording(nowMs: clock(), down: down, up: e.uploadSpeed)
        return previous.status != status
    }

    private func applyRuntime(_ runtime: TaskRuntime) {
        guard let task = w.tasks[runtime.taskId], task.status.retainsRuntime else { return }
        let previous = w.runtime[runtime.taskId]
        if let previous, previous.sampleSequence != 0, runtime.sampleSequence <= previous.sampleSequence { return }
        var next = runtime
        if runtime.segments.isEmpty, let previous { next.segments = previous.segments }
        if !task.status.isActive { next = Self.inactive(next) }
        w.runtime[runtime.taskId] = next
    }

    /// 任务状态变化后的运行时收尾（对应 Rust `settle_runtime`）：完成即删除条目，其余非活跃状态清零读数。
    private func settleRuntime(_ id: String, status: TaskStatus) {
        if !status.retainsRuntime {
            w.runtime[id] = nil
            w.speeds[id] = nil
        } else if !status.isActive {
            clearActiveRuntime(id)
        }
    }

    private func clearActiveRuntime(_ id: String) {
        if let runtime = w.runtime[id] { w.runtime[id] = Self.inactive(runtime) }
        w.speeds[id] = nil
    }

    private static func inactive(_ runtime: TaskRuntime) -> TaskRuntime {
        var next = runtime
        next.activeTransfers = 0
        next.connectedPeers = 0
        next.segments = runtime.segments.map { segment in
            var s = segment
            if s.active != false { s.active = false }
            return s
        }
        return next
    }

    private func removeTask(_ id: String) {
        w.remove(id)
        w.runtime[id] = nil
        w.speeds[id] = nil
        w.taskHistory[id] = nil
    }

    private func schedulePublish() {
        guard publishTask == nil else { return }
        let interval = publishInterval
        publishTask = Task { [weak self] in
            try? await Task.sleep(for: interval)
            guard !Task.isCancelled else { return }
            self?.publishNow()
        }
    }

    private func publishNow() {
        publishTask?.cancel()
        publishTask = nil
        state = w.snapshot()
    }
}

/// 可变工作副本：只在主 actor 上访问。
private struct Working {
    var connection: Connection = .connecting
    var info: HostInfo?
    /// 插入序（快照顺序 + 新增追加）。
    var order: [String] = []
    var tasks: [String: DownloadTask] = [:]
    var tasksLoading = false
    var taskTotal: UInt64 = 0
    var runtime: [String: TaskRuntime] = [:]
    var speeds: [String: LiveSpeed] = [:]
    var queues: [TaskQueue] = []
    /// 顺序变化才重建不可变视图；其余发布复用同一份。
    var queueEntries: [QueueOrderEntry] = [] {
        didSet { queueOrderCache = nil }
    }
    private var queueOrderCache: QueueOrder?
    var queueOrder: QueueOrder {
        mutating get {
            if let cached = queueOrderCache { return cached }
            let built = QueueOrder(sorted: queueEntries)
            queueOrderCache = built
            return built
        }
    }
    var groups: [DownloadGroup] = []
    var stats = RuntimeStats()
    var priorityTaskId: String?
    var selections: [SelectionRequest] = []
    var history = SpeedHistory.empty
    var taskHistory: [String: SpeedHistory] = [:]
    var config: [String: String] = [:]
    var configRevision: UInt64 = 0
    var rssSources: [RssSource] = []
    var cloudDevices: [CloudDevice] = []
    var linkDevices: [LinkDevice] = []
    var categories: [TaskCategory] = TaskCategory.builtin
    var sections: [String: Data] = [:]

    init() {}

    /// 快照替换投影；速度清空（只来自事件），速度历史保留（同一主机重同步不应抹平曲线）。
    init(snapshot s: HostSnapshot, previous: Working) {
        info = s.info
        for task in s.tasks { upsert(task) }
        tasksLoading = s.tasksLoading
        taskTotal = s.tasksLoading ? s.taskTotal : 0
        runtime = s.runtime.filter { tasks[$0.key]?.status.retainsRuntime == true }
        queues = s.queues
        queueEntries = s.queueOrder
        groups = s.groups
        stats = s.stats
        priorityTaskId = s.priorityTaskId
        selections = s.pendingSelections
        config = s.config
        configRevision = s.configRevision
        rssSources = s.rssSources
        cloudDevices = s.cloudDevices
        linkDevices = s.linkDevices
        categories = s.categories
        sections = s.sections
        history = previous.history
        // 载入期间的空任务表不是「任务都被删了」：保留速度曲线，载入完成的快照再按真实任务表裁剪。
        taskHistory = s.tasksLoading ? previous.taskHistory : previous.taskHistory.filter { tasks[$0.key] != nil }
        if !s.daemonConnected { runtime.removeAll() }
    }

    mutating func upsert(_ task: DownloadTask) {
        if tasks.updateValue(task, forKey: task.taskId) == nil { order.append(task.taskId) }
    }

    mutating func remove(_ id: String) {
        if tasks.removeValue(forKey: id) != nil { order.removeAll { $0 == id } }
    }

    mutating func snapshot() -> HostState {
        var state = HostState()
        state.connection = connection
        state.info = info
        state.tasks = order.compactMap { tasks[$0] }
        state.tasksLoading = tasksLoading
        state.taskTotal = taskTotal
        state.runtime = runtime
        state.speeds = speeds
        state.queues = queues
        state.queueOrder = queueOrder
        state.groups = groups
        state.stats = stats
        state.priorityTaskId = priorityTaskId
        state.selections = selections
        state.speedHistory = history
        state.taskSpeedHistory = taskHistory
        state.config = config
        state.configRevision = configRevision
        state.rssSources = rssSources
        state.cloudDevices = cloudDevices
        state.linkDevices = linkDevices
        state.categories = categories
        state.sections = sections
        return state
    }
}
