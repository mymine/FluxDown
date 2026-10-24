import FluxDomain
import FluxUI
import Foundation

// 任务详情（D3）的只读派生量与纯函数。不含任何视图 / 文案查表，便于单测（同 Android `TaskDetailModel.kt`）。

/// 派生显示状态（02-downloads §1.3 / §6.3：颜色 + 图形 + 文字三通道）。
nonisolated enum TaskDetailVisual: Sendable, Hashable {
    case downloading, queued, pending, preparing, verifying, paused, failed, seeding, missing, completed

    init(task: DownloadTask, queuePosition: Int) {
        switch task.status {
        case .downloading: self = .downloading
        case .pending: self = queuePosition > 0 ? .queued : .pending
        case .preparing: self = task.totalBytes > 0 ? .verifying : .preparing
        case .paused: self = .paused
        case .failed: self = .failed
        case .completed:
            if task.fileMissing {
                self = .missing
            } else if task.presentsSeeding {
                self = .seeding
            } else {
                self = .completed
            }
        case .unknown: self = .pending
        }
    }

    /// 已完成一族（完成 / 做种 / 文件已删除）：进度恒为 100%，不再显示分段图谱。
    var isFinished: Bool { self == .completed || self == .seeding || self == .missing }

    /// 英雄图谱着色：准备 / 校验沿用「下载中」强调色（无总量时由图谱呈现不定进度）。
    var segmentTone: SegmentTone {
        switch self {
        case .downloading, .preparing, .verifying: .downloading
        case .queued, .pending: .queued
        case .paused: .paused
        case .failed: .failed
        case .seeding: .seeding
        case .completed, .missing: .completed
        }
    }

    /// 状态徽标语气。
    var badgeTone: BadgeTone {
        switch self {
        case .downloading: .accent
        case .failed: .failure
        case .seeding: .success
        case .missing: .warning
        case .queued, .pending, .preparing, .verifying, .paused, .completed: .neutral
        }
    }

    /// 状态符号（02-downloads §6.3）。
    var systemImage: String {
        switch self {
        case .downloading: "arrow.down.circle"
        case .queued, .pending: "clock"
        case .preparing, .verifying: "arrow.trianglehead.2.clockwise.rotate.90"
        case .paused: "pause.circle"
        case .failed: "exclamationmark.circle"
        case .seeding: "arrow.up.circle"
        case .missing: "exclamationmark.triangle"
        case .completed: "checkmark.circle"
        }
    }
}

/// 详情页一次渲染所需的派生量。值相等 → SwiftUI 不重算（主机约 10 Hz 发布，单任务字段变化频率低得多）。
nonisolated struct TaskDetailModel: Equatable {
    let task: DownloadTask
    let visual: TaskDetailVisual
    /// 排队序号（1 起）；0 = 不在排队。
    let queuePosition: Int
    let queue: TaskQueue?
    let group: FluxDomain.DownloadGroup?
    let speedDown: Int64
    let speedUp: Int64
    let runtime: TaskRuntime?
    let boosted: Bool

    init?(state: HostState, taskId: String) {
        guard let task = state.task(taskId) else { return nil }
        let position = state.queueOrder.rank(of: taskId)
        let speed = state.speed(taskId)
        self.task = task
        visual = TaskDetailVisual(task: task, queuePosition: position)
        queuePosition = position
        queue = state.queues.first { $0.queueId == task.queueId || (task.queueId.isEmpty && $0.queueId == TaskQueue.main) }
        group = task.groupId.isEmpty ? nil : state.groups.first { $0.groupId == task.groupId }
        speedDown = speed.down
        speedUp = speed.up
        runtime = state.runtime[taskId]
        boosted = state.priorityTaskId == taskId
    }

    var isTransferring: Bool { task.status == .downloading }

    /// 英雄图谱的真实字节区间：`Segment.endByte` 为闭区间，图谱的 `endByte` 为开区间（+1）；
    /// 非传输态一律「非活跃」（不呼吸）。无分段 / 无总量 → 空（图谱退化为单条进度）。
    var heroSpans: [SegmentSpan] {
        guard task.totalBytes > 0, let runtime else { return [] }
        let live = isTransferring
        return runtime.segments.map { segment in
            SegmentSpan(
                startByte: segment.startByte,
                endByte: segment.endByte + 1,
                downloadedBytes: max(segment.downloadedBytes, 0),
                active: live ? segment.active : false
            )
        }
    }

    /// 进度分数：完成一族恒为 1；总大小未知为 nil。
    var progress: Double? { visual.isFinished ? 1 : task.progress }

    /// 剩余大小（总大小未知 → nil）。
    var remainingBytes: Int64? {
        task.totalBytes > 0 ? max(task.totalBytes - task.downloadedBytes, 0) : nil
    }

    var etaSeconds: Int64? {
        Format.etaSeconds(downloaded: task.downloadedBytes, total: task.totalBytes, speed: speedDown)
    }

    /// 完成耗时（秒）：需要创建 / 完成时间都有效。
    var durationSeconds: Int64? {
        guard task.createdAt > 0, task.completedAt >= task.createdAt else { return nil }
        return task.completedAt - task.createdAt
    }
}

/// 来源构成（General 页环形图）：源站 = 已下载 − (CDN + 代理 + 多网卡)；BT / eD2K 全部来自 P2P。
nonisolated struct TaskSourceComposition: Equatable {
    nonisolated enum Kind: Hashable { case origin, cdn, proxy, nic, p2p }

    nonisolated struct Row: Equatable {
        let kind: Kind
        let bytes: Int64
    }

    let downloaded: Int64
    let rows: [Row]
    let isP2P: Bool
    /// 加速路径（CDN + 代理 + 多网卡）占已下载的份额，0…1。
    let accelShare: Double

    init(task: DownloadTask) {
        let downloaded = max(task.downloadedBytes, 0)
        let accel = task.sourceBytes
        self.downloaded = downloaded
        isP2P = task.protocol == .bt || task.protocol == .ed2k
        accelShare = downloaded > 0 ? min(max(Double(accel.total) / Double(downloaded), 0), 1) : 0
        if isP2P {
            rows = [Row(kind: .p2p, bytes: downloaded)]
        } else {
            var rows = [Row(kind: .origin, bytes: max(downloaded - accel.total, 0))]
            // HTTP 总是列出三条加速路径；其他协议只列有字节的行（02-downloads §5.3）。
            let always = task.protocol == .http
            for (kind, bytes) in [(Kind.cdn, accel.cdn), (.proxy, accel.proxy), (.nic, accel.nic)] where always || bytes > 0 {
                rows.append(Row(kind: kind, bytes: max(bytes, 0)))
            }
            self.rows = rows
        }
    }

    func share(of row: Row) -> Double {
        downloaded > 0 ? min(max(Double(row.bytes) / Double(downloaded), 0), 1) : 0
    }

    /// 图例百分比：1 位小数；非零但 < 0.1% 显示 `<0.1%`。
    static func percentText(_ fraction: Double) -> String {
        if fraction > 0, fraction < 0.001 { return "<0.1%" }
        return String(format: "%.1f%%", locale: Locale(identifier: "en_US_POSIX"), fraction * 100)
    }

    /// 摘要里的整数百分比（`sourcesAccelShare`）。
    var accelPercentText: String { "\(Int((accelShare * 100).rounded()))%" }
}

/// 速度页持久化曲线（`daemon.task.speedHistory`）：同一页内刷新串行（不重叠），
/// 失败（含旧主机缺方法）保留上一份有效数据；页面只展示 `taskId` 与当前任务一致的数据。
@MainActor
@Observable
final class TaskSpeedHistoryModel {
    private(set) var history: TaskSpeedHistory?

    /// 轮询周期：下载中每 2 秒重取一次。
    static let pollInterval: Duration = .seconds(2)

    /// 取一次；已被取消（任务 / 下载态变化、页面消失）则丢弃回包。
    func refresh(taskId: String, session: any HostSession) async {
        let result: TaskSpeedHistory?
        do {
            result = try await session.call(HostMethod.daemonTaskSpeedHistory, params: TaskSpeedHistoryQuery(taskId: taskId))
        } catch {
            return
        }
        if Task.isCancelled { return }
        apply(result, for: taskId)
    }

    /// 接收回包：`null`（无记录）清空曲线；回包任务与请求任务不符则丢弃。
    func apply(_ result: TaskSpeedHistory?, for taskId: String) {
        guard result == nil || result?.taskId == taskId else { return }
        history = result
    }

    /// 先取一次，下载中则每 2 秒再取；任务 / 下载态变化时由 `.task(id:)` 取消并重启，
    /// 因此离开下载态会再取一次最终曲线。
    func run(taskId: String, downloading: Bool, session: any HostSession) async {
        await refresh(taskId: taskId, session: session)
        guard downloading else { return }
        while !Task.isCancelled {
            try? await Task.sleep(for: Self.pollInterval)
            if Task.isCancelled { return }
            await refresh(taskId: taskId, session: session)
        }
    }
}

/// 详情页文本格式化（纯函数；需要本地化的在 `TaskDetailText`）。
enum TaskDetailFormat {
    /// 协议标记：除插件外为技术名词，不随语言变化。
    static func protocolTag(_ proto: TaskProtocol) -> String {
        switch proto {
        case .http: "HTTP"
        case .ftp: "FTP"
        case .bt: "BT"
        case .ed2k: "ED2K"
        case .hls: "HLS"
        case .plugin: L("protocolPlugin")
        }
    }

    /// 来源站点：originUrl ‖ url 的 host（去 `www.`）；取不到（含 `torrent-file://` 哨兵 / magnet）时 BT 为 BitTorrent，其余为协议名。
    static func siteLabel(_ task: DownloadTask) -> String {
        let raw = task.originUrl.isEmpty ? task.url : task.originUrl
        if !raw.hasPrefix("torrent-file://"),
           let host = URLComponents(string: raw)?.host, !host.isEmpty {
            return host.hasPrefix("www.") && host.count > 4 ? String(host.dropFirst(4)) : host
        }
        #if FULL_FLAVOR
        return task.protocol == .bt ? "BitTorrent" : protocolTag(task.protocol)
        #else
        return protocolTag(task.protocol)
        #endif
    }

    /// 首个非空行（错误信息常带多行堆栈）。
    static func firstLine(_ text: String) -> String {
        for line in text.split(whereSeparator: \.isNewline) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty { return trimmed }
        }
        return ""
    }

    /// Unix 秒 → `YYYY-MM-DD HH:MM:SS`（本地时区）；≤ 0 = 无。
    static func dateTime(_ epochSeconds: Int64) -> String {
        guard epochSeconds > 0 else { return "" }
        return Date(timeIntervalSince1970: TimeInterval(epochSeconds)).formatted(
            .verbatim(
                "\(year: .defaultDigits)-\(month: .twoDigits)-\(day: .twoDigits) \(hour: .twoDigits(clock: .twentyFourHour, hourCycle: .zeroBased)):\(minute: .twoDigits):\(second: .twoDigits)",
                locale: Locale(identifier: "en_US_POSIX"),
                timeZone: .current,
                calendar: Calendar(identifier: .gregorian)
            )
        )
    }

    /// 分享率 = 已上传 / 已下载（无已下载则退回总大小）；分母为 0 → 0。
    static func seedRatio(uploaded: Int64, downloaded: Int64, total: Int64) -> Double {
        let basis = downloaded > 0 ? downloaded : total
        return basis > 0 ? Double(uploaded) / Double(basis) : 0
    }

    static func seedRatioText(uploaded: Int64, downloaded: Int64, total: Int64) -> String {
        String(format: "%.2f", locale: Locale(identifier: "en_US_POSIX"), seedRatio(uploaded: uploaded, downloaded: downloaded, total: total))
    }

    /// 做种时长三档拆分（天 / 时 / 分）。
    static func durationParts(_ totalSeconds: Int64) -> (days: Int64, hours: Int64, minutes: Int64) {
        let seconds = max(totalSeconds, 0)
        return (seconds / 86_400, seconds % 86_400 / 3600, seconds % 3600 / 60)
    }

    /// 精确字节数 `1,234,567 B`（高级页）。
    static func exactBytes(_ n: Int64) -> String {
        n.formatted(.byteCount(style: .file, allowedUnits: .bytes))
    }
}
