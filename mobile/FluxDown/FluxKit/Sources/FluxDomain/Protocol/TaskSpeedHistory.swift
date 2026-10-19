import Foundation

/// `daemon.task.speedHistory` 参数。
public struct TaskSpeedHistoryQuery: Codable, Sendable, Hashable {
    public var taskId: String

    public init(taskId: String) {
        self.taskId = taskId
    }
}

/// 任务整个生命周期的持久化速度曲线与统计（`TaskSpeedHistoryDto`）；结果为 `null` 表示尚无记录。
///
/// - `samples`：每个点为 `bucketSecs` 秒的平均 B/s，从旧到新；最后一个点可能是仍在累积的不完整桶。
/// - `activeSecs`：处于下载状态的总秒数（横轴为活跃时间，暂停 / 排队 / 做种不计）。
public struct TaskSpeedHistory: Codable, Sendable, Hashable {
    public var taskId: String
    public var bucketSecs: UInt32
    public var samples: [Int64]
    public var activeSecs: Int64
    public var transferredBytes: Int64
    public var peakBps: Int64

    public init(
        taskId: String, bucketSecs: UInt32 = 1, samples: [Int64] = [],
        activeSecs: Int64 = 0, transferredBytes: Int64 = 0, peakBps: Int64 = 0
    ) {
        self.taskId = taskId
        self.bucketSecs = bucketSecs
        self.samples = samples
        self.activeSecs = activeSecs
        self.transferredBytes = transferredBytes
        self.peakBps = peakBps
    }

    /// 平均速度 = 已传输字节 / 活跃秒数（无活跃时间为 0）。
    public var averageBps: Int64 {
        activeSecs > 0 ? transferredBytes / activeSecs : 0
    }

    /// 曲线是否值得绘制：至少 2 个点且峰值 > 0。
    public var hasCurve: Bool {
        samples.count >= 2 && peakBps > 0
    }

    /// 每个采样点对应的已下载时长（秒）：第 i 点 = min((i+1)·bucketSecs, activeSecs)，并强制严格递增
    /// （末尾不完整桶被 activeSecs 截断后不会与前一点重复）。
    public var elapsedSeconds: [Int64] {
        let bucket = Int64(max(bucketSecs, 1))
        var result: [Int64] = []
        result.reserveCapacity(samples.count)
        for i in samples.indices {
            var e = min(Int64(i + 1) * bucket, activeSecs)
            if let last = result.last, e <= last { e = last + 1 }
            result.append(e)
        }
        return result
    }
}

/// 时钟式时长：`m:ss`，满 1 小时为 `h:mm:ss`（负数按 0）。无需本地化。
public func clockDurationText(_ seconds: Int64) -> String {
    let s = max(seconds, 0)
    let h = s / 3600, m = (s % 3600) / 60, sec = s % 60
    if h > 0 { return "\(h):\(m < 10 ? "0" : "")\(m):\(sec < 10 ? "0" : "")\(sec)" }
    return "\(m):\(sec < 10 ? "0" : "")\(sec)"
}
