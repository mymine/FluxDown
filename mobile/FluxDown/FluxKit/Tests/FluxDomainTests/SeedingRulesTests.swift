@testable import FluxDomain
import Testing

struct SeedingRulesTests {
    private func task(
        _ status: TaskStatus, _ seeding: SeedingStatus = .none, missing: Bool = false
    ) -> DownloadTask {
        DownloadTask(taskId: "t", url: "magnet:?xt=1", fileName: "t", status: status, fileMissing: missing, seedingStatus: seeding)
    }

    @Test func isSeedingCoversSeedingAndQueuedOnlyWhenCompleted() {
        #expect(task(.completed, .seeding).isSeeding)
        #expect(task(.completed, .queuedForSlot).isSeeding)
        #expect(!task(.completed, .userStopped).isSeeding)
        #expect(!task(.completed, .none).isSeeding)
        #expect(!task(.completed, .unknown).isSeeding)
        #expect(!task(.downloading, .seeding).isSeeding)
        #expect(!task(.paused, .queuedForSlot).isSeeding)
    }

    @Test func canResumeSeedingNeedsStoppedSeedAndExistingFile() {
        let stopped: [SeedingStatus] = [.ratioReached, .timeReached, .userStopped, .taskDeleted, .sessionReleased, .inactiveStop]
        for status in stopped {
            #expect(task(.completed, status).canResumeSeeding)
            #expect(!task(.completed, status, missing: true).canResumeSeeding)
        }
        // 普通已完成任务保持不可恢复（#935）。
        #expect(!task(.completed, .none).canResumeSeeding)
        #expect(!task(.completed, .seeding).canResumeSeeding)
        #expect(!task(.completed, .queuedForSlot).canResumeSeeding)
        #expect(!task(.completed, .unknown).canResumeSeeding)
        #expect(!task(.paused, .userStopped).canResumeSeeding)
    }

    @Test func allPausedCountsSeedingAsPausableAndUserStoppedSeedAsResumable() {
        // 纯做种主机不是「全部已暂停」。
        #expect(![task(.completed, .seeding)].isAllPaused(active: 0, pending: 0))
        #expect(![task(.completed, .queuedForSlot), task(.paused)].isAllPaused(active: 0, pending: 0))
        #expect([task(.paused)].isAllPaused(active: 0, pending: 0))
        #expect(![task(.paused)].isAllPaused(active: 1, pending: 0))
        #expect(![task(.paused)].isAllPaused(active: 0, pending: 2))
        // 用户停止的做种可被「全部继续」恢复；文件缺失 / 其它停止原因不算。
        #expect([task(.completed, .userStopped)].isAllPaused(active: 0, pending: 0))
        #expect(![task(.completed, .userStopped, missing: true)].isAllPaused(active: 0, pending: 0))
        #expect(![task(.completed, .ratioReached)].isAllPaused(active: 0, pending: 0))
        #expect(![task(.completed)].isAllPaused(active: 0, pending: 0))
    }
}
