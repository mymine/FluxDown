import FluxDomain
import FluxUI
import SwiftUI

/// 队列管理（D7 / D8）共用的展示文案与小部件。
enum QueueText {
    /// 星期标签（`weekdaysShort` 逗号分隔，周一起，共 7 个；数量不符返回空数组）。
    static func weekdayLabels() -> [String] {
        let labels = L("weekdaysShort").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        return labels.count == 7 ? labels : []
    }

    /// 星期摘要：每天 / 连续区间 `一–五`（不跨周）/ 零散日 `一, 三`。
    static func daysSummary(_ mask: Int32) -> String {
        if WeekdayMask.isEveryDay(mask) { return L("queueScheduleEveryDay") }
        let labels = weekdayLabels()
        guard !labels.isEmpty else { return "" }
        let parts = WeekdayMask.runs(mask).map { run -> String in
            switch run.count {
            case 1: labels[run.lowerBound]
            case 2: labels[run.lowerBound] + ", " + labels[run.upperBound]
            default: labels[run.lowerBound] + "–" + labels[run.upperBound]
            }
        }
        return L("queueScheduleDaysSummary", ["days": parts.joined(separator: ", ")])
    }

    /// `HH:MM–HH:MM`；未设置的一端显示 `--:--`。
    static func timeRange(start: String, stop: String) -> String {
        let from = ScheduleTime.parse(start).map { ScheduleTime.format($0) } ?? "--:--"
        let to = ScheduleTime.parse(stop).map { ScheduleTime.format($0) } ?? "--:--"
        return from + "–" + to
    }

    /// 定时摘要（未启用 = nil）：`09:00–18:00 · 周一–五`。
    static func scheduleSummary(_ queue: TaskQueue) -> String? {
        guard QueueIds.canSchedule(queue.queueId), queue.scheduleEnabled else { return nil }
        let days = daysSummary(queue.scheduleDays)
        let range = timeRange(start: queue.scheduleStart, stop: queue.scheduleStop)
        return days.isEmpty ? range : range + " · " + days
    }

    /// D7 行摘要：n 个任务 · 并发 · ↓ 限速 · ↑ 限速 · 定时（0 = 不限制的项省略）。
    /// `taskCount == nil` = 任务表还在分页载入，数量未知：显示载入文案而不是 0。
    static func summary(_ queue: TaskQueue, taskCount: Int?) -> String {
        var parts = [taskCount.map { L("nTasks", ["n": $0]) } ?? L("mobileTasksLoading")]
        if queue.maxConcurrent > 0 {
            parts.append(L("queueSummaryConcurrent", ["n": queue.maxConcurrent]))
        }
        if let down = Format.speed(bytesPerSecond(kbps: queue.speedLimitKbps)) {
            parts.append("↓ " + down.description)
        }
        if let up = Format.speed(bytesPerSecond(kbps: queue.uploadLimitKbps)) {
            parts.append("↑ " + up.description)
        }
        if let schedule = scheduleSummary(queue) { parts.append(schedule) }
        return parts.joined(separator: " · ")
    }

    /// 启停成功 Toast 文案（字面键，供 i18n 脚本扫描）。
    static func runningToast(_ running: Bool, name: String) -> String {
        running ? L("queueRunningToast", ["name": name]) : L("queueStoppedToast", ["name": name])
    }

    /// KB/s → B/s（防溢出）。
    private static func bytesPerSecond(kbps: Int64) -> Int64 {
        min(kbps, Int64.max / 1024) * 1024
    }

    /// 各队列任务数（"" 与 `main` 同一队列）。
    static func taskCounts(_ tasks: [DownloadTask]) -> [String: Int] {
        var counts: [String: Int] = [:]
        for task in tasks { counts[normalizedQueueId(task.queueId), default: 0] += 1 }
        return counts
    }
}

/// 运行状态徽标：圆点 + 文字（不只靠颜色）。
struct QueueStateBadge: View {
    let running: Bool

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(running ? Color.fdStatusSeeding : Color.fdStatusPaused)
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(L(running ? "queueRunningBadge" : "queueStoppedBadge"))
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }
}

/// 行内错误提示：图标 + 文字（语气不只靠颜色）。
struct QueueIssueText: View {
    let text: String

    var body: some View {
        Label {
            Text(text)
        } icon: {
            Image(systemName: FluxSymbol.failure)
        }
        .font(.footnote)
        .foregroundStyle(Color.fdStatusFailedText)
        .fixedSize(horizontal: false, vertical: true)
    }
}
