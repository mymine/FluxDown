import Foundation

// 队列相关 wire DTO（镜像 `native/protocol/src/daemon.rs` 的 `QueueDto` / `CreateQueueRequest` 与
// `web/src/lib/rpc/protocol/{queue,params}.ts`）+ 队列管理（D7 / D8）的纯逻辑：定时位掩码、HH:MM、表单校验、排序。

// MARK: - Wire

/// `daemon.queue.list` 元素（完整 `QueueDto`：比快照里的 ``TaskQueue`` 多 `defaultSegments` / `defaultUserAgent`，
/// 编辑表单必须用它回填，否则保存会把这两个字段清零）。
public struct QueueDetail: Codable, Sendable, Hashable, Identifiable {
    public var queueId: String
    public var name: String
    /// KB/s，0 = 不限速。
    public var speedLimitKbps: Int64
    /// KB/s，0 = 不限速。
    public var uploadLimitKbps: Int64
    /// 0 = 跟随全局。
    public var maxConcurrent: Int32
    public var defaultSaveDir: String
    public var position: Int32
    /// 0 = 自动。
    public var defaultSegments: Int32
    public var defaultUserAgent: String
    public var isRunning: Bool
    public var scheduleEnabled: Bool
    /// `HH:MM`，空 = 不定时启动。
    public var scheduleStart: String
    public var scheduleStop: String
    /// bit0 = 周一 … bit6 = 周日；127 = 每天。
    public var scheduleDays: Int32

    public var id: String { queueId }

    public init(
        queueId: String,
        name: String,
        speedLimitKbps: Int64 = 0,
        uploadLimitKbps: Int64 = 0,
        maxConcurrent: Int32 = 0,
        defaultSaveDir: String = "",
        position: Int32 = 0,
        defaultSegments: Int32 = 0,
        defaultUserAgent: String = "",
        isRunning: Bool = true,
        scheduleEnabled: Bool = false,
        scheduleStart: String = "",
        scheduleStop: String = "",
        scheduleDays: Int32 = WeekdayMask.everyDay
    ) {
        self.queueId = queueId
        self.name = name
        self.speedLimitKbps = speedLimitKbps
        self.uploadLimitKbps = uploadLimitKbps
        self.maxConcurrent = maxConcurrent
        self.defaultSaveDir = defaultSaveDir
        self.position = position
        self.defaultSegments = defaultSegments
        self.defaultUserAgent = defaultUserAgent
        self.isRunning = isRunning
        self.scheduleEnabled = scheduleEnabled
        self.scheduleStart = scheduleStart
        self.scheduleStop = scheduleStop
        self.scheduleDays = scheduleDays
    }

    private enum CodingKeys: String, CodingKey {
        case queueId, name, speedLimitKbps, uploadLimitKbps, maxConcurrent, defaultSaveDir, position
        case defaultSegments, defaultUserAgent, isRunning, scheduleEnabled, scheduleStart, scheduleStop, scheduleDays
    }

    /// Rust 侧 `#[serde(default)]` 的字段旧主机可能不下发：全部按默认值宽松解码。
    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        queueId = try c.decode(String.self, forKey: .queueId)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        speedLimitKbps = try c.decodeIfPresent(Int64.self, forKey: .speedLimitKbps) ?? 0
        uploadLimitKbps = try c.decodeIfPresent(Int64.self, forKey: .uploadLimitKbps) ?? 0
        maxConcurrent = try c.decodeIfPresent(Int32.self, forKey: .maxConcurrent) ?? 0
        defaultSaveDir = try c.decodeIfPresent(String.self, forKey: .defaultSaveDir) ?? ""
        position = try c.decodeIfPresent(Int32.self, forKey: .position) ?? 0
        defaultSegments = try c.decodeIfPresent(Int32.self, forKey: .defaultSegments) ?? 0
        defaultUserAgent = try c.decodeIfPresent(String.self, forKey: .defaultUserAgent) ?? ""
        isRunning = try c.decodeIfPresent(Bool.self, forKey: .isRunning) ?? true
        scheduleEnabled = try c.decodeIfPresent(Bool.self, forKey: .scheduleEnabled) ?? false
        scheduleStart = try c.decodeIfPresent(String.self, forKey: .scheduleStart) ?? ""
        scheduleStop = try c.decodeIfPresent(String.self, forKey: .scheduleStop) ?? ""
        scheduleDays = try c.decodeIfPresent(Int32.self, forKey: .scheduleDays) ?? WeekdayMask.everyDay
    }

    public var isBuiltin: Bool { QueueIds.isBuiltin(queueId) }
}

/// `daemon.queue.create` 参数（`CreateQueueRequest`）；`daemon.queue.update` 在其上拍平 `queueId`。
public struct QueueInput: Codable, Sendable, Hashable {
    public var name: String
    public var speedLimitKbps: Int64
    public var uploadLimitKbps: Int64
    public var maxConcurrent: Int32
    public var defaultSaveDir: String
    public var defaultSegments: Int32
    public var defaultUserAgent: String

    public init(
        name: String,
        speedLimitKbps: Int64 = 0,
        uploadLimitKbps: Int64 = 0,
        maxConcurrent: Int32 = 0,
        defaultSaveDir: String = "",
        defaultSegments: Int32 = 0,
        defaultUserAgent: String = ""
    ) {
        self.name = name
        self.speedLimitKbps = speedLimitKbps
        self.uploadLimitKbps = uploadLimitKbps
        self.maxConcurrent = maxConcurrent
        self.defaultSaveDir = defaultSaveDir
        self.defaultSegments = defaultSegments
        self.defaultUserAgent = defaultUserAgent
    }
}

/// `daemon.queue.update`：`queueId` + 拍平的 ``QueueInput``。
public struct QueueUpdateParams: Codable, Sendable, Hashable {
    public var queueId: String
    public var name: String
    public var speedLimitKbps: Int64
    public var uploadLimitKbps: Int64
    public var maxConcurrent: Int32
    public var defaultSaveDir: String
    public var defaultSegments: Int32
    public var defaultUserAgent: String

    public init(queueId: String, input: QueueInput) {
        self.queueId = queueId
        name = input.name
        speedLimitKbps = input.speedLimitKbps
        uploadLimitKbps = input.uploadLimitKbps
        maxConcurrent = input.maxConcurrent
        defaultSaveDir = input.defaultSaveDir
        defaultSegments = input.defaultSegments
        defaultUserAgent = input.defaultUserAgent
    }
}

/// `daemon.queue.{delete,start,stop}`。
public struct QueueIdParams: Codable, Sendable, Hashable {
    public var queueId: String

    public init(queueId: String) { self.queueId = queueId }
}

/// `daemon.queue.schedule`：`HH:MM` 空串 = 不定时；`days` 位掩码（0 / 缺省 = 每天）。
public struct QueueScheduleParams: Codable, Sendable, Hashable {
    public var queueId: String
    public var enabled: Bool
    public var startTime: String
    public var stopTime: String
    public var days: Int32

    public init(queueId: String, enabled: Bool, startTime: String, stopTime: String, days: Int32) {
        self.queueId = queueId
        self.enabled = enabled
        self.startTime = startTime
        self.stopTime = stopTime
        self.days = days
    }
}

/// `daemon.queue.reorder`：队列内任务的完整新顺序。
public struct QueueReorderParams: Codable, Sendable, Hashable {
    public var queueId: String
    public var taskIds: [String]

    public init(queueId: String, taskIds: [String]) {
        self.queueId = queueId
        self.taskIds = taskIds
    }
}

/// `daemon.queue.moveTask`：`queueId` 空串 = 默认队列。
public struct QueueMoveTaskParams: Codable, Sendable, Hashable {
    public var taskId: String
    public var queueId: String

    public init(taskId: String, queueId: String) {
        self.taskId = taskId
        self.queueId = queueId
    }
}

// MARK: - 队列 id

public enum QueueIds {
    /// 内置队列：不可重命名 / 删除。
    public static func isBuiltin(_ id: String) -> Bool {
        id == TaskQueue.main || id == TaskQueue.later
    }

    /// 主队列：始终运行，不能停止，也不能定时。
    public static func isMain(_ id: String) -> Bool { normalized(id) == TaskQueue.main }

    /// 运行中的队列能否被「停止」（主队列不能；停止态永远走「启动」）。
    public static func canStop(_ id: String) -> Bool { !isMain(id) }

    /// 队列能否配置每日定时（主队列不能）。
    public static func canSchedule(_ id: String) -> Bool { !isMain(id) }

    /// 任务的 `queueId` 为空串 = 隐式主队列。
    public static func normalized(_ id: String) -> String { id.isEmpty ? TaskQueue.main : id }
}

// MARK: - 星期位掩码（bit0 = 周一 … bit6 = 周日）

public enum WeekdayMask {
    public static let everyDay: Int32 = 127

    /// 线上 `0` = 每天；高位噪声丢弃。
    public static func normalized(_ mask: Int32) -> Int32 {
        let value = mask & everyDay
        return value == 0 ? everyDay : value
    }

    /// `day`：0 = 周一 … 6 = 周日。
    public static func contains(_ mask: Int32, day: Int) -> Bool {
        guard (0 ... 6).contains(day) else { return false }
        return normalized(mask) & (1 << Int32(day)) != 0
    }

    /// 切换某天。至少保留一天（全部取消没有意义，线上 0 又等于每天）：试图取消最后一天时原样返回。
    public static func toggling(_ mask: Int32, day: Int) -> Int32 {
        guard (0 ... 6).contains(day) else { return normalized(mask) }
        let current = normalized(mask)
        let next = current ^ (1 << Int32(day))
        return next == 0 ? current : next
    }

    /// 选中的天（升序）。
    public static func days(_ mask: Int32) -> [Int] {
        (0 ... 6).filter { contains(mask, day: $0) }
    }

    /// 连续区间（周一至周五 → `[0...4]`）；不跨周（周日 → 周一不合并，与 PC 摘要一致）。
    public static func runs(_ mask: Int32) -> [ClosedRange<Int>] {
        var out: [ClosedRange<Int>] = []
        var start: Int?
        for day in 0 ... 7 {
            let on = day < 7 && contains(mask, day: day)
            if on {
                if start == nil { start = day }
            } else if let from = start {
                out.append(from ... (day - 1))
                start = nil
            }
        }
        return out
    }

    public static func isEveryDay(_ mask: Int32) -> Bool { normalized(mask) == everyDay }
}

// MARK: - HH:MM

public enum ScheduleTime {
    /// 当日分钟步长（选择器的分钟列）。
    public static let minuteStep = 5

    /// `HH:MM` → 当日分钟数；空串 / 非法 = nil（不定时）。
    public static func parse(_ text: String) -> Int? {
        let parts = text.trimmingCharacters(in: .whitespaces).split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count == 2,
              let hours = Int(parts[0]), let minutes = Int(parts[1]),
              parts[0].allSatisfy(\.isASCII), parts[1].allSatisfy(\.isASCII),
              (1 ... 2).contains(parts[0].count), (1 ... 2).contains(parts[1].count),
              hours < 24, minutes < 60 else { return nil }
        return hours * 60 + minutes
    }

    /// 当日分钟数 → `HH:MM`；nil = 空串。
    public static func format(_ minutes: Int?) -> String {
        guard let minutes, (0 ..< 1440).contains(minutes) else { return "" }
        return String(format: "%02d:%02d", minutes / 60, minutes % 60)
    }

    /// 分钟列候选：按步长取整点，外加当前值（旧数据可能不是步长整数倍），升序去重。
    public static func minuteChoices(current: Int) -> [Int] {
        var choices = Set(stride(from: 0, to: 60, by: minuteStep))
        if (0 ..< 60).contains(current) { choices.insert(current) }
        return choices.sorted()
    }
}

// MARK: - 编辑表单（D8）

/// 队列编辑表单草稿：全部以文本 / 原始值持有，``validated(builtin:)`` 产出线上参数（对齐 Web `QueueEditor.save`
/// 与 GPUI `queue_manager.rs`）。
public struct QueueDraft: Sendable, Hashable {
    public enum Issue: Error, Sendable, Hashable {
        /// 非内置队列名称为空。
        case nameRequired
        /// 数值不是非负整数（线程数另限 0–64）。
        case invalidNumber
        /// 启用定时但启动 / 停止时间都未设置。
        case scheduleNeedsOneTime
    }

    public static let maxSegments = 64

    public var name: String
    public var speedLimit: String
    public var uploadLimit: String
    public var maxConcurrent: String
    public var segments: String
    public var saveDir: String
    public var userAgent: String
    public var scheduleEnabled: Bool
    /// 当日分钟数；nil = 不定时。
    public var scheduleStart: Int?
    public var scheduleStop: Int?
    public var days: Int32

    /// 新建默认（全 0 / 空，定时关，每天）。
    public init() {
        name = ""
        speedLimit = "0"
        uploadLimit = "0"
        maxConcurrent = "0"
        segments = "0"
        saveDir = ""
        userAgent = ""
        scheduleEnabled = false
        scheduleStart = nil
        scheduleStop = nil
        days = WeekdayMask.everyDay
    }

    public init(queue: QueueDetail) {
        name = queue.name
        speedLimit = String(queue.speedLimitKbps)
        uploadLimit = String(queue.uploadLimitKbps)
        maxConcurrent = String(queue.maxConcurrent)
        segments = String(queue.defaultSegments)
        saveDir = queue.defaultSaveDir
        userAgent = queue.defaultUserAgent
        scheduleEnabled = queue.scheduleEnabled
        scheduleStart = ScheduleTime.parse(queue.scheduleStart)
        scheduleStop = ScheduleTime.parse(queue.scheduleStop)
        days = WeekdayMask.normalized(queue.scheduleDays)
    }

    /// 空 = 0；纯十进制数字（无符号 / 小数 / 其他字符）否则 nil。
    static func nonNegativeInt(_ text: String) -> Int64? {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty { return 0 }
        guard trimmed.allSatisfy({ $0.isASCII && $0.isNumber }) else { return nil }
        return Int64(trimmed)
    }

    /// 校验并产出 ``QueueInput``。内置队列名称沿用 `existingName`（不可重命名）。
    public func validated(builtin: Bool, existingName: String? = nil) -> Result<QueueInput, Issue> {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !builtin, trimmed.isEmpty { return .failure(.nameRequired) }
        guard let speed = Self.nonNegativeInt(speedLimit),
              let upload = Self.nonNegativeInt(uploadLimit),
              let concurrent = Self.nonNegativeInt(maxConcurrent), concurrent <= Int64(Int32.max),
              let threads = Self.nonNegativeInt(segments), threads <= Int64(Self.maxSegments) else {
            return .failure(.invalidNumber)
        }
        if scheduleEnabled, scheduleStart == nil, scheduleStop == nil { return .failure(.scheduleNeedsOneTime) }
        return .success(QueueInput(
            name: builtin ? (existingName ?? trimmed) : trimmed,
            speedLimitKbps: speed,
            uploadLimitKbps: upload,
            maxConcurrent: Int32(concurrent),
            defaultSaveDir: saveDir.trimmingCharacters(in: .whitespacesAndNewlines),
            defaultSegments: Int32(threads),
            defaultUserAgent: userAgent.trimmingCharacters(in: .whitespacesAndNewlines)
        ))
    }

    /// 定时参数（`daemon.queue.schedule`）。
    public func schedule(queueId: String) -> QueueScheduleParams {
        QueueScheduleParams(
            queueId: queueId,
            enabled: scheduleEnabled,
            startTime: ScheduleTime.format(scheduleStart),
            stopTime: ScheduleTime.format(scheduleStop),
            days: WeekdayMask.normalized(days)
        )
    }

    /// 需要发 `daemon.queue.schedule`：主队列永不；其余任一定时字段有效。
    public func needsScheduleCall(queueId: String) -> Bool {
        QueueIds.canSchedule(queueId) && (scheduleEnabled || scheduleStart != nil || scheduleStop != nil)
    }
}

// MARK: - 新建后定位新队列

public enum QueueLookup {
    /// `daemon.queue.create` 不回 id：按名称取 `position` 最大者（最新创建）。
    public static func created(named name: String, in list: [QueueDetail]) -> QueueDetail? {
        list.filter { $0.name == name }.max { $0.position < $1.position }
    }
}

// MARK: - 队列内待处理任务顺序（`daemon.queue.reorder`）

public enum QueueOrdering {
    /// 队列内待下载（pending）任务，按引擎启动顺序：排序键升序（即排队名次）；不在排队顺序里的排后并按创建时间。
    public static func pending(
        in queueId: String,
        tasks: [DownloadTask],
        order: QueueOrder
    ) -> [DownloadTask] {
        let key = QueueIds.normalized(queueId)
        return tasks
            .filter { $0.status == .pending && QueueIds.normalized($0.queueId) == key }
            .map { (task: $0, order: order.order(of: $0.taskId) ?? .max) }
            .sorted { lhs, rhs in
                if lhs.order != rhs.order { return lhs.order < rhs.order }
                if lhs.task.createdAt != rhs.task.createdAt { return lhs.task.createdAt < rhs.task.createdAt }
                return lhs.task.taskId < rhs.task.taskId
            }
            .map(\.task)
    }

    /// 把 `index` 处的任务移动 `delta`（±1）位；越界返回 nil。
    public static func moved(_ ids: [String], index: Int, by delta: Int) -> [String]? {
        let target = index + delta
        guard ids.indices.contains(index), ids.indices.contains(target) else { return nil }
        var next = ids
        next.swapAt(index, target)
        return next
    }

    /// `List.onMove` 语义（`IndexSet` → 目标偏移）的纯实现，供拖动重排。
    public static func moved(_ ids: [String], from sources: IndexSet, to destination: Int) -> [String] {
        let moving = sources.filter { ids.indices.contains($0) }.map { ids[$0] }
        var rest = ids.enumerated().filter { !sources.contains($0.offset) }.map(\.element)
        let insertAt = destination - sources.filter { $0 < destination }.count
        rest.insert(contentsOf: moving, at: min(max(insertAt, 0), rest.count))
        return rest
    }
}
