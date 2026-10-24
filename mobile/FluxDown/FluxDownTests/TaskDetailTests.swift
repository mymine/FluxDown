import FluxDomain
import FluxUI
import Foundation
import Testing
@testable import FluxDown

@MainActor
struct TaskDetailTests {
    private func task(
        url: String = "https://example.com/a.bin",
        status: TaskStatus = .downloading,
        downloaded: Int64 = 0,
        total: Int64 = 0,
        accel: SourceBytes = SourceBytes(),
        origin: String = ""
    ) -> DownloadTask {
        DownloadTask(
            taskId: "t1",
            url: url,
            originUrl: origin,
            fileName: "a.bin",
            status: status,
            downloadedBytes: downloaded,
            totalBytes: total,
            sourceBytes: accel
        )
    }

    // MARK: 视觉状态

    @Test func visualStateMatrix() {
        #expect(TaskDetailVisual(task: task(status: .pending), queuePosition: 2) == .queued)
        #expect(TaskDetailVisual(task: task(status: .pending), queuePosition: 0) == .pending)
        #expect(TaskDetailVisual(task: task(status: .preparing, total: 0), queuePosition: 0) == .preparing)
        #expect(TaskDetailVisual(task: task(status: .preparing, total: 10), queuePosition: 0) == .verifying)
        #expect(TaskDetailVisual(task: task(status: .unknown), queuePosition: 0) == .pending)

        var done = task(status: .completed)
        #expect(TaskDetailVisual(task: done, queuePosition: 0) == .completed)
        done.seedingStatus = .seeding
        // store 不含做种：做种任务按已完成呈现。
        #if FULL_FLAVOR
        #expect(TaskDetailVisual(task: done, queuePosition: 0) == .seeding)
        #else
        #expect(TaskDetailVisual(task: done, queuePosition: 0) == .completed)
        #endif
        done.fileMissing = true
        #expect(TaskDetailVisual(task: done, queuePosition: 0) == .missing)
        #expect(TaskDetailVisual.missing.isFinished)
        #expect(!TaskDetailVisual.paused.isFinished)
    }

    // MARK: 来源构成

    @Test func httpCompositionSubtractsAccelFromOrigin() {
        let t = task(downloaded: 1000, total: 2000, accel: SourceBytes(cdn: 200, proxy: 100, nic: 50))
        let c = TaskSourceComposition(task: t)
        #expect(!c.isP2P)
        #expect(c.rows == [
            .init(kind: .origin, bytes: 650),
            .init(kind: .cdn, bytes: 200),
            .init(kind: .proxy, bytes: 100),
            .init(kind: .nic, bytes: 50),
        ])
        #expect(abs(c.accelShare - 0.35) < 1e-9)
        #expect(c.accelPercentText == "35%")
    }

    @Test func originNeverNegative() {
        let t = task(downloaded: 100, accel: SourceBytes(cdn: 300))
        let c = TaskSourceComposition(task: t)
        #expect(c.rows.first == .init(kind: .origin, bytes: 0))
        #expect(c.accelShare == 1)
    }

    @Test func p2pCompositionIsSingleRow() {
        let bt = TaskSourceComposition(task: task(url: "magnet:?xt=urn:btih:abc", downloaded: 500))
        #expect(bt.isP2P)
        #expect(bt.rows == [.init(kind: .p2p, bytes: 500)])
        let ed2k = TaskSourceComposition(task: task(url: "ed2k://|file|x|1|h|/", downloaded: 7))
        #expect(ed2k.isP2P)
    }

    @Test func nonHttpListsOnlyAcceleratedRows() {
        let ftp = TaskSourceComposition(task: task(url: "ftp://h/a", downloaded: 100, accel: SourceBytes(proxy: 40)))
        #expect(ftp.rows.map(\.kind) == [.origin, .proxy])
        let http = TaskSourceComposition(task: task(downloaded: 100))
        #expect(http.rows.map(\.kind) == [.origin, .cdn, .proxy, .nic])
    }

    @Test func percentText() {
        #expect(TaskSourceComposition.percentText(0) == "0.0%")
        #expect(TaskSourceComposition.percentText(0.0004) == "<0.1%")
        #expect(TaskSourceComposition.percentText(0.123) == "12.3%")
        #expect(TaskSourceComposition.percentText(1) == "100.0%")
    }

    // MARK: 速度曲线

    @Test func speedHistoryModelAppliesNullAndRejectsOtherTasks() {
        let model = TaskSpeedHistoryModel()
        let mine = TaskSpeedHistory(taskId: "a", samples: [1, 2], activeSecs: 2, transferredBytes: 3, peakBps: 2)
        model.apply(mine, for: "a")
        #expect(model.history == mine)
        // 回包属于别的任务：丢弃，保留上一份。
        model.apply(TaskSpeedHistory(taskId: "b", samples: [9, 9]), for: "a")
        #expect(model.history == mine)
        // null = 无记录（如重新下载后）：清空。
        model.apply(nil, for: "a")
        #expect(model.history == nil)
    }

    // MARK: 做种与格式化

    @Test func seedRatio() {
        #expect(TaskDetailFormat.seedRatioText(uploaded: 150, downloaded: 100, total: 200) == "1.50")
        // 无已下载 → 退回总大小
        #expect(TaskDetailFormat.seedRatioText(uploaded: 50, downloaded: 0, total: 200) == "0.25")
        #expect(TaskDetailFormat.seedRatioText(uploaded: 50, downloaded: 0, total: 0) == "0.00")
    }

    @Test func durationParts() {
        let p = TaskDetailFormat.durationParts(86_400 + 3 * 3600 + 5 * 60 + 9)
        #expect(p.days == 1 && p.hours == 3 && p.minutes == 5)
        let zero = TaskDetailFormat.durationParts(-5)
        #expect(zero.days == 0 && zero.hours == 0 && zero.minutes == 0)
    }

    @Test func siteLabel() {
        #expect(TaskDetailFormat.siteLabel(task(url: "https://www.example.com/x")) == "example.com")
        #if FULL_FLAVOR
        #expect(TaskDetailFormat.siteLabel(task(url: "torrent-file://local")) == "BitTorrent")
        #expect(TaskDetailFormat.siteLabel(task(url: "magnet:?xt=urn:btih:abc")) == "BitTorrent")
        #else
        #expect(TaskDetailFormat.siteLabel(task(url: "torrent-file://local")) == "BT")
        #expect(TaskDetailFormat.siteLabel(task(url: "magnet:?xt=urn:btih:abc")) == "BT")
        #endif
        #expect(TaskDetailFormat.siteLabel(task(url: "torrent-file://local", origin: "https://t.example.org/a.torrent")) == "t.example.org")
        #expect(TaskDetailFormat.siteLabel(task(url: "ed2k://|file|x|1|h|/")) == "ED2K")
    }

    @Test func firstLineAndDateAndBytes() {
        #expect(TaskDetailFormat.firstLine("\n  \n connection reset \nstack") == "connection reset")
        #expect(TaskDetailFormat.firstLine("") == "")
        #expect(TaskDetailFormat.dateTime(0) == "")
        #expect(TaskDetailFormat.dateTime(1_700_000_000).count == 19)
        #expect(TaskDetailFormat.exactBytes(1_234_567).contains("567"))
    }

    // MARK: 模型派生

    @Test func heroSpansUseInclusiveEnd() {
        var state = HostState()
        let t = task(status: .downloading, downloaded: 50, total: 100)
        state.tasks = [t]
        state.runtime["t1"] = TaskRuntime(
            taskId: "t1",
            sampleSequence: 1,
            activeTransfers: 1,
            connectedPeers: nil,
            totalBytes: 100,
            segments: [Segment(index: 0, startByte: 0, endByte: 49, downloadedBytes: 50, active: true)]
        )
        guard let model = TaskDetailModel(state: state, taskId: "t1") else {
            Issue.record("model missing")
            return
        }
        #expect(model.heroSpans == [SegmentSpan(startByte: 0, endByte: 50, downloadedBytes: 50, active: true)])
        #expect(model.remainingBytes == 50)
        #expect(model.progress == 0.5)
        #expect(TaskDetailModel(state: state, taskId: "missing") == nil)
    }

    @Test func pausedSegmentsAreNotActive() {
        var state = HostState()
        state.tasks = [task(status: .paused, downloaded: 10, total: 100)]
        state.runtime["t1"] = TaskRuntime(
            taskId: "t1", sampleSequence: 1, activeTransfers: nil, connectedPeers: nil, totalBytes: 100,
            segments: [Segment(index: 0, startByte: 0, endByte: 99, downloadedBytes: 10, active: true)]
        )
        let model = TaskDetailModel(state: state, taskId: "t1")
        #expect(model?.heroSpans.first?.active == false)
    }
}
