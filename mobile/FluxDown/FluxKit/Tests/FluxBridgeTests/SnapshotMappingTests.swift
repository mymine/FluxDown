@testable import FluxBridge
import FluxDomain
import FluxRustBindings
import Testing

/// `HostSnapshotDto` 的任务表分页载入标志经 UniFFI DTO → 领域模型 → 状态仓库保真：
/// 加载态快照（空占位 + 总数）不得被当成完整列表。
struct SnapshotMappingTests {
    private func dto(tasksLoading: Bool, taskTotal: UInt64) -> HostSnapshotDto {
        HostSnapshotDto(
            info: HostInfoDto(serviceName: "t", serviceVersion: "1", protocolVersion: 17, capabilities: []),
            daemonConnected: true,
            tasksLoading: tasksLoading,
            taskTotal: taskTotal,
            tasks: [],
            runtime: [],
            queues: [],
            queueOrder: [],
            groups: [],
            stats: RuntimeStatsDto(
                activeTasks: 0, pendingTasks: 0, totalDownloadBps: 0, totalUploadBps: 0,
                diskFreeBytes: nil, saveDir: "", retryPendingTasks: 0
            ),
            priorityTaskId: nil,
            pendingSelections: [],
            config: [:],
            configRevision: 1,
            rssSources: [],
            cloudDevices: [],
            linkDevices: [],
            categories: [],
            sections: [:]
        )
    }

    @Test func loadingSnapshotKeepsTheFlagAndTotal() {
        let snapshot = dto(tasksLoading: true, taskTotal: 230_000).domain
        #expect(snapshot.tasksLoading)
        #expect(snapshot.taskTotal == 230_000)
        #expect(snapshot.tasks.isEmpty)
    }

    @Test func completeSnapshotIsNotLoading() {
        let snapshot = dto(tasksLoading: false, taskTotal: 0).domain
        #expect(!snapshot.tasksLoading)
        #expect(snapshot.taskTotal == 0)
    }

    @Test func categoryMappingCarriesUrlPattern() {
        let dto = CategoryDto(
            id: "site", name: "Site", icon: "globe", extensions: [], regexPattern: "",
            urlPattern: "https://*.example.com/*", position: 2, visible: true, builtinType: nil
        )
        let category = dto.domain
        #expect(category.urlPattern == "https://*.example.com/*")
        #expect(category.regexPattern.isEmpty)
        #expect(category.id == "site")
    }

    @MainActor
    @Test func loadingFlagReachesTheStoreAndClearsOnTheCompleteSnapshot() {
        let store = HostStore()
        store.apply(.snapshot(dto(tasksLoading: true, taskTotal: 5).domain))
        #expect(store.state.tasksLoading && store.state.taskTotal == 5)
        store.apply(.snapshot(dto(tasksLoading: false, taskTotal: 5).domain))
        #expect(!store.state.tasksLoading && store.state.taskTotal == 0)
    }
}
