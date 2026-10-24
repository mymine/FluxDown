import FluxBridge
import FluxDomain
import Foundation
import Observation
import os

/// 本机引擎活动量的来源（同 Android `AppContainer.localActivity`，与当前选中的主机无关）：
///
/// - 当前主机就是本机：直接投影主 `HostStore` 的状态，不再开第二个会话；
/// - 当前主机是远端：本机引擎仍在进程内运行（下载照样在跑），对同一个本机主机再开一个只读会话，
///   绑定到私有 `HostStore`（1 s 合帧；结构性变化立即发布）。回到本机或主机切换时关闭。
///
/// 全程事件驱动：`withObservationTracking` 只在被读取的状态变化时唤醒，没有周期轮询。
/// 失联 / 未连上（`stale` / `failed` / `connecting`）一律视为空闲，避免后台任务僵死。
@MainActor
final class LocalActivityMonitor {
    private let container: AppContainer
    private let onChange: @MainActor (LocalActivity) -> Void
    private let log = Logger(subsystem: "com.fluxdown.app", category: "background")

    private var batch = TransferBatch()
    private var current = LocalActivity.idle

    private let probeStore = HostStore(publishInterval: .seconds(1))
    private var probeSession: (any HostSession)?
    /// 非 nil = 本轮远端期间已尝试打开探测会话（失败后不重试，直到回本机再切远端）。
    private var probeTask: Task<Void, Never>?

    init(container: AppContainer, onChange: @escaping @MainActor (LocalActivity) -> Void) {
        self.container = container
        self.onChange = onChange
    }

    func start() {
        observe()
    }

    private func observe() {
        let (isLocalHost, state) = withObservationTracking {
            let isLocal = container.isLocalHost
            return (isLocal, isLocal ? container.store.state : probeStore.state)
        } onChange: { [weak self] in
            // onChange 在变更生效前（willSet）同步触发：下一轮主 actor 再重新读取并重新登记。
            Task { @MainActor [weak self] in self?.observe() }
        }
        syncProbe(isLocalHost: isLocalHost)
        publish(derive(state))
    }

    private func derive(_ state: HostState) -> LocalActivity {
        guard state.connection == .live else {
            batch.reset()
            return .idle
        }
        let stats = LocalActivity(stats: state.stats, progress: .indeterminate)
        guard stats.busy else {
            batch.reset()
            return .idle
        }
        // 任务表分页载入中：空表 / 半份表不是「成员被删除」，保持上一轮成员不动（载入完成后再 observe）。
        if !state.tasksLoading { batch.observe(state.tasks) }
        return LocalActivity(stats: state.stats, progress: batch.progress)
    }

    private func publish(_ next: LocalActivity) {
        guard next != current else { return }
        current = next
        onChange(next)
    }

    // MARK: 远端期间的本机探测会话

    private func syncProbe(isLocalHost: Bool) {
        if isLocalHost {
            guard probeTask != nil || probeSession != nil else { return }
            stopProbe()
        } else if probeTask == nil {
            startProbe()
        }
    }

    private func startProbe() {
        probeTask = Task { [weak self] in
            do throws(HostError) {
                // 本机主机已在运行时复用同一个（目录参数被忽略）。
                let session = try await FluxBridge.openLocal(
                    dataDir: LocalPaths.dataDirectory.path,
                    saveDir: LocalPaths.documents.path,
                    deviceName: LocalDevice.name
                )
                guard let self, !Task.isCancelled else {
                    session.close()
                    return
                }
                probeSession = session
                probeStore.attach(session)
            } catch {
                self?.log.notice("local activity probe unavailable: \(error.description, privacy: .public)")
            }
        }
    }

    private func stopProbe() {
        probeTask?.cancel()
        probeTask = nil
        probeStore.detach()
        probeSession?.close()
        probeSession = nil
    }
}
