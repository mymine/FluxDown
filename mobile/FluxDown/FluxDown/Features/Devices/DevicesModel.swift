import FluxDomain
import FluxUI
import Foundation
import Observation

/// 设备页的数据与动作中枢（V1 / V1a / V3 共用，由 `DevicesScreen` 持有并经环境下发）。
///
/// - 云端设备：快照里的 `cloudDevices` 没有云端行 id，重命名 / 删除必须用 `agent.device.list` 的完整记录，
///   因此这里按需加载 `CloudDeviceRecord`，并在快照里的设备集合（id / 名称 / 在线 / 本机）变化时重新加载。
/// - 局域网设备：`agent.link.refresh` 的完整 `LinkDeviceInfo`（配对时间 / 默认目录 / 路径风格）。
/// - 远程任务命令：行内「进行中」状态、失败 Toast；适用性判定只走 `RemoteTaskRules.canIssue`。
///
/// 所有 agent.* 调用都落在**当前主机**的 agent 上；主机切换后旧结果一律丢弃。
@MainActor
@Observable
final class DevicesModel {
    nonisolated enum Phase: Equatable {
        case idle, loading, loaded
        case failed(HostError)
    }

    /// 云端设备的同步指纹：只含会改变列表呈现的字段。
    nonisolated struct CloudSig: Hashable {
        var deviceId: String
        var name: String
        var isOnline: Bool
        var isCurrent: Bool

        init(_ device: CloudDevice) {
            deviceId = device.deviceId
            name = device.name
            isOnline = device.isOnline
            isCurrent = device.isCurrent
        }

        init(_ record: CloudDeviceRecord) {
            deviceId = record.deviceId
            name = record.name
            isOnline = record.isOnline
            isCurrent = record.isCurrent
        }
    }

    /// `.onChange(of:)` 的触发键：任一项变化都意味着需要重新对齐一次。
    nonisolated struct SyncKey: Hashable {
        var host: String
        var live: Bool
        var cloudEligible: Bool
        var linkEligible: Bool
        var cloud: [CloudSig]
        var link: [String]
    }

    /// 远程任务目标设备的展示信息。
    nonisolated struct Target: Equatable {
        var name: String
        var online: Bool
    }

    /// 命令已被云端接受、等待状态事件回写：状态与 `updatedAt` 都没变化期间行保持转圈。
    private struct Settling {
        var status: RemoteTaskStatus
        var updatedAt: String
    }

    // MARK: 状态

    private(set) var cloudRecords: [CloudDeviceRecord] = []
    private(set) var cloudPhase: Phase = .idle
    private(set) var linkInfos: [LinkDeviceInfo] = []
    private(set) var linkPhase: Phase = .idle
    /// 「重试 / 重新连接」按钮的进行中状态。
    private(set) var retrying = false
    private(set) var cloudEligible = false
    private(set) var linkEligible = false
    private var commandsInFlight: Set<String> = []
    private var settling: [String: Settling] = [:]

    /// 通用分区的缓存读取器（各屏共用同一份缓存）。
    @ObservationIgnored let sections = AgentSections()
    @ObservationIgnored private unowned let container: AppContainer
    @ObservationIgnored private var syncedHost: String?
    @ObservationIgnored private var snapshotCloud: [CloudSig] = []
    @ObservationIgnored private var snapshotLink: [String] = []
    @ObservationIgnored private var cloudInFlight: Task<HostError?, Never>?
    @ObservationIgnored private var linkInFlight: Task<HostError?, Never>?
    @ObservationIgnored private var cloudResync = false

    /// 命令落地后等待状态事件回写的最长时间。
    private static let settleTimeout: Duration = .seconds(6)

    init(container: AppContainer) {
        self.container = container
    }

    // MARK: 派生

    func isLoggedIn(_ state: HostState) -> Bool {
        state.has(HostCapability.agentAuth) && sections.session(state) != nil
    }

    func presenceKnown(_ state: HostState) -> Bool {
        CloudPresence.isKnown(sections.connection(state), localReady: state.connection == .live)
    }

    func syncKey(_ state: HostState) -> SyncKey {
        let live = state.connection == .live
        let cloudOn = live && isLoggedIn(state)
        let linkOn = live && state.has(HostCapability.agentDeviceLink)
        return SyncKey(
            host: container.host.id,
            live: live,
            cloudEligible: cloudOn,
            linkEligible: linkOn,
            cloud: cloudOn ? state.cloudDevices.map(CloudSig.init) : [],
            link: linkOn ? state.linkDevices.map(\.fingerprint).sorted() : []
        )
    }

    /// 本机在 FluxCloud 的 deviceId（会话优先，其次设备列表里的 `isCurrent`）。
    func currentDeviceId(_ state: HostState) -> String? {
        DeviceRules.currentDeviceId(session: sections.session(state), devices: cloudRecords)
    }

    /// 可见的远程任务（目标是本机的不作为镜像行显示）。
    func remoteTasks(_ state: HostState) -> [RemoteTaskDto] {
        RemoteTaskRules.visible(sections.remoteTasks(state), currentDeviceId: currentDeviceId(state))
    }

    /// 目标设备名称与在线状态：快照（实时）优先，其次已加载的完整记录。
    func target(for deviceId: String, state: HostState) -> Target? {
        if let device = state.cloudDevices.first(where: { $0.deviceId == deviceId }) {
            return Target(name: device.name, online: device.isOnline)
        }
        if let record = cloudRecords.first(where: { $0.deviceId == deviceId }) {
            return Target(name: record.name, online: record.isOnline)
        }
        return nil
    }

    /// 局域网设备的完整信息：以快照里的实时名称 / 在线状态覆盖加载到的记录；设备不在快照里 = nil。
    func linkInfo(_ fingerprint: String, state: HostState) -> LinkDeviceInfo? {
        guard let live = state.linkDevices.first(where: { $0.fingerprint == fingerprint }) else { return nil }
        var info = linkInfos.first(where: { $0.fingerprint == fingerprint })
            ?? LinkDeviceInfo(fingerprint: live.fingerprint, name: live.name, platform: live.platform, online: live.online)
        info.name = live.name
        info.online = live.online
        if let platform = live.platform { info.platform = platform }
        info.customName = live.customName
        info.appVersion = live.appVersion
        info.cloudDeviceId = live.cloudDeviceId
        info.revoked = live.revoked
        if live.pairedAt > 0 { info.pairedAt = live.pairedAt }
        if live.lastSeenAt > 0 { info.lastSeenAt = live.lastSeenAt }
        return info
    }

    // MARK: 同步

    /// 让云端记录 / 局域网信息与快照对齐（幂等；`.onChange(of: syncKey)` 与首次出现时调用）。
    func sync(_ state: HostState) {
        let hostId = container.host.id
        if syncedHost != hostId {
            syncedHost = hostId
            resetAll()
        }
        let live = state.connection == .live
        cloudEligible = live && isLoggedIn(state)
        linkEligible = live && state.has(HostCapability.agentDeviceLink)
        snapshotCloud = state.cloudDevices.map(CloudSig.init)
        snapshotLink = state.linkDevices.map(\.fingerprint)

        if cloudEligible {
            if cloudPhase == .loading {
                cloudResync = true
            } else if cloudNeedsLoad {
                Task { await loadCloud() }
            }
        } else if cloudPhase != .loading, cloudPhase != .idle || !cloudRecords.isEmpty {
            cloudRecords = []
            cloudPhase = .idle
        }

        if linkEligible {
            if linkPhase != .loading, linkNeedsLoad {
                Task { await loadLink() }
            }
        } else if linkPhase != .loading, linkPhase != .idle || !linkInfos.isEmpty {
            linkInfos = []
            linkPhase = .idle
        }
    }

    private var cloudNeedsLoad: Bool {
        switch cloudPhase {
        case .idle: true
        case .loading: false
        case .loaded, .failed: cloudRecords.map(CloudSig.init) != snapshotCloud
        }
    }

    private var linkNeedsLoad: Bool {
        switch linkPhase {
        case .idle: true
        case .loading: false
        case .loaded, .failed: Set(linkInfos.map(\.fingerprint)) != Set(snapshotLink)
        }
    }

    private func resetAll() {
        cloudRecords = []
        cloudPhase = .idle
        linkInfos = []
        linkPhase = .idle
        commandsInFlight = []
        settling = [:]
        cloudResync = false
    }

    // MARK: 云端设备

    /// 加载 `agent.device.list`；同一时刻只有一次在途调用（并发调用方共享结果）。返回失败原因。
    @discardableResult
    func loadCloud() async -> HostError? {
        guard cloudEligible else { return nil }
        if let running = cloudInFlight { return await running.value }
        cloudResync = false
        let task = Task { await self.runCloudLoad() }
        cloudInFlight = task
        let result = await task.value
        cloudInFlight = nil
        if result == nil, cloudResync, cloudEligible, cloudRecords.map(CloudSig.init) != snapshotCloud {
            cloudResync = false
            return await loadCloud()
        }
        cloudResync = false
        return result
    }

    private func runCloudLoad() async -> HostError? {
        let hostId = container.host.id
        cloudPhase = .loading
        do throws(HostError) {
            let list = try await container.agent.deviceList()
            guard container.host.id == hostId else { return nil }
            cloudRecords = list
            cloudPhase = .loaded
            return nil
        } catch {
            guard container.host.id == hostId else { return nil }
            cloudPhase = .failed(error)
            return error
        }
    }

    /// 重命名（`name` 已校验为 1–64 字符）。成功后重新加载列表。
    func rename(_ record: CloudDeviceRecord, to name: String) async -> HostError? {
        do throws(HostError) {
            try await container.agent.deviceRename(id: record.id, name: name)
        } catch {
            return error
        }
        await loadCloud()
        return nil
    }

    /// 删除设备；删除本机同时清除本机会话（`session` 分区随之变空，页面自动回到未登录态）。
    func delete(_ record: CloudDeviceRecord) async -> HostError? {
        do throws(HostError) {
            try await container.agent.deviceDelete(id: record.id)
        } catch {
            return error
        }
        if !record.isCurrent { await loadCloud() }
        return nil
    }

    // MARK: 局域网设备

    /// `agent.link.refresh`：探测全部已配对设备的在线状态并返回完整信息（同时推送快照）。
    @discardableResult
    func loadLink() async -> HostError? {
        guard linkEligible else { return nil }
        if let running = linkInFlight { return await running.value }
        let task = Task { await self.runLinkLoad() }
        linkInFlight = task
        let result = await task.value
        linkInFlight = nil
        return result
    }

    private func runLinkLoad() async -> HostError? {
        let hostId = container.host.id
        linkPhase = .loading
        do throws(HostError) {
            let infos = try await container.agent.linkRefresh()
            guard container.host.id == hostId else { return nil }
            linkInfos = infos
            linkPhase = .loaded
            return nil
        } catch {
            guard container.host.id == hostId else { return nil }
            linkPhase = .failed(error)
            return error
        }
    }

    func unpair(fingerprint: String) async -> HostError? {
        do throws(HostError) {
            try await container.agent.linkRemove(fingerprint: fingerprint)
        } catch {
            return error
        }
        linkInfos.removeAll { $0.fingerprint == fingerprint }
        await loadLink()
        return nil
    }

    /// 设置 / 清除本机备注名（`agent.link.rename`）；快照随 `LinkedDevicesChanged` 推送更新。
    func renameLink(fingerprint: String, name: String?) async -> HostError? {
        do throws(HostError) {
            let info = try await container.agent.linkRename(fingerprint: fingerprint, name: name)
            if let index = linkInfos.firstIndex(where: { $0.fingerprint == fingerprint }) { linkInfos[index] = info }
            return nil
        } catch {
            return error
        }
    }

    // MARK: 刷新

    /// 下拉刷新：云端设备列表（presence 未知且主机支持时先请求重连）+ 局域网刷新；失败各出一条 Toast。
    func refreshAll(presenceKnown: Bool, reconnectable: Bool) async {
        async let linkError = loadLink()
        let cloudError = await refreshCloudIfEligible(presenceKnown: presenceKnown, reconnectable: reconnectable)
        let linkFailure = await linkError
        if let cloudError {
            container.toasts.show(text: AccountText.error(cloudError), tone: .error)
        } else if cloudEligible {
            container.toasts.show(text: L("accountCloudRefreshDone"), tone: .success)
        }
        if let linkFailure {
            container.toasts.show(text: AccountText.error(linkFailure, context: .pairing), tone: .error)
        }
    }

    private func refreshCloudIfEligible(presenceKnown: Bool, reconnectable: Bool) async -> HostError? {
        guard cloudEligible else { return nil }
        if !presenceKnown, reconnectable {
            do throws(HostError) {
                try await container.agent.remoteReconnect()
            } catch {
                return error
            }
        }
        return await loadCloud()
    }

    /// 已信任设备分组里的「重试 / 重新连接」（Web `DevicesCard.refresh`）：presence 未知时先请求重连，再刷新列表。
    func retryCloud(presenceKnown: Bool, reconnectable: Bool) async {
        guard cloudEligible, !retrying else { return }
        retrying = true
        defer { retrying = false }
        let reconnecting = !presenceKnown && reconnectable
        if let error = await refreshCloudIfEligible(presenceKnown: presenceKnown, reconnectable: reconnectable) {
            container.toasts.show(text: AccountText.error(error), tone: .error)
            return
        }
        container.toasts.show(
            text: L(reconnecting ? "cloudConnectionRetryStarted" : "accountCloudRefreshDone"),
            tone: .success
        )
    }

    // MARK: 远程任务命令

    /// 行是否处于过渡态：命令在途，或已被接受但任务的状态 / `updatedAt` 还没回写。
    func isBusy(_ task: RemoteTaskDto) -> Bool {
        if commandsInFlight.contains(task.id) { return true }
        guard let pending = settling[task.id] else { return false }
        return pending.status == task.status && pending.updatedAt == task.updatedAt
    }

    /// 下发 `agent.remote.command`。适用性由调用方（行视图）经 `RemoteTaskRules.canIssue` 保证；失败 Toast。
    func issue(_ action: RemoteCommandAction, to task: RemoteTaskDto, deleteFiles: Bool = false) {
        guard !commandsInFlight.contains(task.id) else { return }
        commandsInFlight.insert(task.id)
        settling[task.id] = nil
        let taskId = task.id
        let before = Settling(status: task.status, updatedAt: task.updatedAt)
        Task {
            do throws(HostError) {
                try await container.agent.remoteCommand(
                    RemoteCommandParams(taskId: taskId, action: action, deleteFiles: deleteFiles)
                )
                commandsInFlight.remove(taskId)
                settling[taskId] = before
                expireSettling(taskId)
            } catch {
                commandsInFlight.remove(taskId)
                container.toasts.show(text: AccountText.error(error), tone: .error)
            }
        }
    }

    private func expireSettling(_ taskId: String) {
        Task {
            do {
                try await Task.sleep(for: Self.settleTimeout)
            } catch {
                return
            }
            settling[taskId] = nil
        }
    }
}
