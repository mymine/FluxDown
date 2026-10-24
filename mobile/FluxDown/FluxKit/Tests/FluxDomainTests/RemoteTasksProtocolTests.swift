import FluxDomain
import Foundation
import Testing

/// 跨设备任务 / 云端设备：解码 + 控制矩阵 + 下发目录规则。
struct RemoteTasksProtocolTests {
    private static func decode<T: Decodable>(_ json: String, as type: T.Type = T.self) throws -> T {
        try ProtocolJSON.decode(T.self, from: Data(json.utf8), what: "test")
    }

    // MARK: 解码

    @Test func remoteTaskDecodesNullFileNameAndUnknownStatus() throws {
        let tasks = try Self.decode("""
        [{"id":"r1","fromDevice":"a","toDevice":"b","url":"https://x/y.bin","saveDir":null,"fileName":null,
          "status":"downloading","totalBytes":2048,"downloadedBytes":512,"speed":100,"progress":0.25,"error":null,
          "createdAt":"2025-01-01T00:00:00Z","updatedAt":"2025-01-01T00:01:00Z"},
         {"id":"r2","status":"quantumSuperposition","totalBytes":null}]
        """, as: [RemoteTaskDto].self)
        #expect(tasks[0].fileName == "")
        #expect(tasks[0].status == .downloading)
        #expect(tasks[0].totalBytes == 2048)
        #expect(tasks[0].progress == 0.25)
        #expect(tasks[1].status == .unknown("quantumSuperposition"))
        #expect(tasks[1].status.isUnknown)
        #expect(tasks[1].toDevice == "")
        #expect(tasks[1].totalBytes == nil)
        // 未知状态往返无损。
        let encoded = try ProtocolJSON.encode(tasks[1], what: "t")
        #expect(String(decoding: encoded, as: UTF8.self).contains("quantumSuperposition"))
    }

    @Test func dispatchAndCommandParamsOmitEmptyOptionals() throws {
        let dispatch = try ProtocolJSON.encode(RemoteDispatchParams(toDevice: "d", url: "http://x/a"), what: "t")
        #expect(String(decoding: dispatch, as: UTF8.self) == #"{"toDevice":"d","url":"http://x/a"}"#)
        let full = try ProtocolJSON.encode(RemoteDispatchParams(toDevice: "d", url: "u", fileName: "a.bin", saveDir: "/dl"), what: "t")
        #expect(String(decoding: full, as: UTF8.self) == #"{"fileName":"a.bin","saveDir":"/dl","toDevice":"d","url":"u"}"#)
        let command = try ProtocolJSON.encode(RemoteCommandParams(taskId: "t", action: .delete, deleteFiles: true), what: "t")
        #expect(String(decoding: command, as: UTF8.self) == #"{"action":"delete","deleteFiles":true,"taskId":"t"}"#)
        let result = try Self.decode(#"{"task":{"id":"r","status":"accepted"}}"#, as: RemoteDispatchResult.self)
        #expect(result.task.status == .accepted)
    }

    // MARK: 控制矩阵

    /// 与 `docs/mobile-ui/ios/04-rss-devices.md` V1 §E 的表逐格对应（暂停 / 继续 / 取消 / 删除）。
    @Test func controlMatrixMatchesDesignTable() {
        typealias Row = (RemoteTaskStatus, pause: Bool, resume: Bool, cancel: Bool, delete: Bool)
        let rows: [Row] = [
            (.pending, true, false, true, true),
            (.accepted, true, false, true, true),
            (.downloading, true, false, true, true),
            (.paused, false, true, true, true),
            (.completed, false, false, false, true),
            (.failed, false, false, false, true),
            (.canceled, false, false, false, true),
            (.unknown("x"), false, false, false, false),
        ]
        for row in rows {
            #expect(row.0.allows(.pause) == row.pause, "pause \(row.0.wire)")
            #expect(row.0.allows(.resume) == row.resume, "resume \(row.0.wire)")
            #expect(row.0.allows(.cancel) == row.cancel, "cancel \(row.0.wire)")
            #expect(row.0.allows(.delete) == row.delete, "delete \(row.0.wire)")
        }
    }

    @Test func pauseAndResumeNeedAnOnlineTargetButCancelAndDeleteDoNot() {
        let downloading = RemoteTaskDto(id: "1", status: .downloading)
        let paused = RemoteTaskDto(id: "2", status: .paused)
        #expect(RemoteTaskRules.canIssue(.pause, to: downloading, targetOnline: true))
        #expect(!RemoteTaskRules.canIssue(.pause, to: downloading, targetOnline: false))
        // 在线状态未知（云连接未建立）不据此禁用。
        #expect(RemoteTaskRules.canIssue(.pause, to: downloading, targetOnline: nil))
        #expect(!RemoteTaskRules.canIssue(.resume, to: paused, targetOnline: false))
        #expect(RemoteTaskRules.canIssue(.resume, to: paused, targetOnline: true))
        #expect(RemoteTaskRules.canIssue(.cancel, to: downloading, targetOnline: false))
        #expect(RemoteTaskRules.canIssue(.delete, to: downloading, targetOnline: false))
        // 状态矩阵优先：已暂停不能再暂停，进行中不能继续。
        #expect(!RemoteTaskRules.canIssue(.pause, to: paused, targetOnline: true))
        #expect(!RemoteTaskRules.canIssue(.resume, to: downloading, targetOnline: true))
        // 未知状态一律不可控制，含删除。
        let unknown = RemoteTaskDto(id: "3", status: .unknown("z"))
        for action in RemoteCommandAction.allCases {
            #expect(!RemoteTaskRules.canIssue(action, to: unknown, targetOnline: true))
        }
    }

    @Test func primaryActionIsPauseWhenActiveResumeWhenPausedElseNone() {
        #expect(RemoteTaskRules.primaryAction(for: RemoteTaskDto(id: "1", status: .pending)) == .pause)
        #expect(RemoteTaskRules.primaryAction(for: RemoteTaskDto(id: "1", status: .accepted)) == .pause)
        #expect(RemoteTaskRules.primaryAction(for: RemoteTaskDto(id: "1", status: .downloading)) == .pause)
        #expect(RemoteTaskRules.primaryAction(for: RemoteTaskDto(id: "1", status: .paused)) == .resume)
        #expect(RemoteTaskRules.primaryAction(for: RemoteTaskDto(id: "1", status: .failed)) == nil)
        #expect(RemoteTaskRules.primaryAction(for: RemoteTaskDto(id: "1", status: .unknown("x"))) == nil)
    }

    @Test func filtersGroupStatuses() {
        func statuses(_ filter: RemoteTaskRules.Filter) -> [String] {
            let all: [RemoteTaskStatus] = [.pending, .accepted, .downloading, .paused, .completed, .failed, .canceled, .unknown("u")]
            return all.filter(filter.matches).map(\.wire)
        }
        #expect(statuses(.all).count == 8)
        #expect(statuses(.downloading) == ["pending", "accepted", "downloading"])
        #expect(statuses(.paused) == ["paused"])
        #expect(statuses(.completed) == ["completed"])
        #expect(statuses(.failedOrCanceled) == ["failed", "canceled"])
    }

    @Test func visibilityCountingAndOrdering() {
        let tasks = [
            RemoteTaskDto(id: "1", toDevice: "me", status: .downloading),
            RemoteTaskDto(id: "2", toDevice: "pc", status: .completed, updatedAt: "2025-01-02"),
            RemoteTaskDto(id: "3", toDevice: "pc", status: .paused, updatedAt: "2025-01-01"),
            RemoteTaskDto(id: "4", toDevice: "nas", status: .downloading, updatedAt: "2025-01-03"),
        ]
        #expect(RemoteTaskRules.visible(tasks, currentDeviceId: "me").map(\.id) == ["2", "3", "4"])
        #expect(RemoteTaskRules.visible(tasks, currentDeviceId: nil).count == 4)
        #expect(RemoteTaskRules.countByTarget(tasks) == ["me": 1, "pc": 2, "nas": 1])
        // 未结束优先，其后按更新时间降序。
        #expect(RemoteTaskRules.sorted(tasks).map(\.id) == ["4", "3", "1", "2"])
    }

    // MARK: 云端设备

    @Test func cloudDeviceDecodesFullShapeAndUnknownPathStyle() throws {
        let list = try Self.decode("""
        {"devices":[{"id":"row-1","deviceId":"dev-1","name":"MacBook","platform":"macos","createdAt":"2025-01-01T00:00:00Z",
          "lastSeenAt":"2025-01-02T00:00:00Z","lastIp":"10.0.0.2","appVersion":"0.18.0","isOnline":true,"isCurrent":false,
          "defaultSaveDir":"/Users/z/Downloads","pathStyle":"posix"},
         {"id":"row-2","deviceId":"dev-2","platform":"templeos","pathStyle":"holyc"}]}
        """, as: CloudDeviceList.self)
        #expect(list.devices[0].id == "row-1")
        #expect(list.devices[0].deviceId == "dev-1")
        #expect(list.devices[0].lastIp == "10.0.0.2")
        #expect(list.devices[0].effectivePathStyle == .posix)
        #expect(list.devices[1].name == "")
        #expect(list.devices[1].pathStyle == .unknown("holyc"))
        #expect(list.devices[1].effectivePathStyle == nil)
        let rename = try ProtocolJSON.encode(DeviceRenameParams(id: "row-1", name: "Mac"), what: "t")
        #expect(String(decoding: rename, as: UTF8.self) == #"{"id":"row-1","name":"Mac"}"#)
    }

    @Test func pathStyleInferenceAndAbsolutePaths() {
        #expect(PathStyle.effective(reported: nil, platform: "Windows") == .windows)
        #expect(PathStyle.effective(reported: .unknown("x"), platform: "android") == .posix)
        #expect(PathStyle.effective(reported: .windows, platform: "linux") == .windows)
        #expect(PathStyle.effective(reported: nil, platform: "web") == nil)
        #expect(PathStyle.windows.isAbsolute(#"C:\Users"#))
        #expect(PathStyle.windows.isAbsolute("d:/dl"))
        #expect(PathStyle.windows.isAbsolute(#"\\server\share"#))
        #expect(!PathStyle.windows.isAbsolute("/home"))
        #expect(!PathStyle.windows.isAbsolute("C:"))
        #expect(PathStyle.posix.isAbsolute(" /mnt/dl "))
        #expect(!PathStyle.posix.isAbsolute("dl"))
        #expect(!PathStyle.unknown("q").isAbsolute("/x"))
    }

    @Test func remoteSaveDirFollowsTargetPathStyle() {
        #expect(DeviceRules.checkSaveDir("  ", style: .posix) == .useDefault)
        #expect(DeviceRules.checkSaveDir(" /mnt/dl ", style: .posix) == .explicit("/mnt/dl"))
        #expect(DeviceRules.checkSaveDir(#"C:\dl"#, style: .posix) == .invalid)
        #expect(DeviceRules.checkSaveDir("/mnt", style: .windows) == .invalid)
        #expect(DeviceRules.checkSaveDir(#"D:\x"#, style: .windows) == .explicit(#"D:\x"#))
        // 风格未知：无法判断，非空即放行。
        #expect(DeviceRules.checkSaveDir("whatever", style: nil) == .explicit("whatever"))
        #expect(DeviceRules.checkSaveDir("whatever", style: .unknown("?")) == .explicit("whatever"))
        #expect(PathStyle.example(.windows) == #"D:\Downloads"#)
        #expect(PathStyle.example(nil) == "/home/user/Downloads")
    }

    @Test func deviceSortingPutsCurrentFirstThenOnlineThenName() {
        func device(_ name: String, online: Bool = false, current: Bool = false) -> CloudDeviceRecord {
            CloudDeviceRecord(id: name, deviceId: name, name: name, isOnline: online, isCurrent: current)
        }
        let devices = [device("zeta", online: true), device("Alpha"), device("beta", online: true), device("me", current: true), device("Gamma")]
        #expect(DeviceRules.sorted(devices, presenceKnown: true).map(\.name) == ["me", "beta", "zeta", "Alpha", "Gamma"])
        // presence 未知：不按在线分组，纯名称序。
        #expect(DeviceRules.sorted(devices, presenceKnown: false).map(\.name) == ["me", "Alpha", "beta", "Gamma", "zeta"])
        #expect(DeviceRules.others(devices).count == 4)
        #expect(DeviceRules.filter(devices, query: " BET ").map(\.name) == ["beta"])
        #expect(DeviceRules.filter(devices, query: "").count == 5)
        let session = AgentSessionDto(user: CloudUser(id: "u", email: "a@b.co"), device: device("session"))
        #expect(DeviceRules.currentDeviceId(session: session, devices: devices) == "session")
        #expect(DeviceRules.currentDeviceId(session: nil, devices: devices) == "me")
        #expect(DeviceRules.currentDeviceId(session: nil, devices: []) == nil)
    }

    @Test func dispatchTargetsSkipCurrentDedupeAndDisambiguate() {
        let cloud = [
            CloudDevice(deviceId: "me-1", name: "Me", platform: "ios", isOnline: true, isCurrent: true, appVersion: nil),
            CloudDevice(
                deviceId: "pc-aa11", name: " Office ", platform: "windows", isOnline: false, isCurrent: false,
                appVersion: nil, defaultSaveDir: "  ", pathStyle: nil
            ),
            CloudDevice(deviceId: "pc-aa11", name: "dup", platform: nil, isOnline: true, isCurrent: false, appVersion: nil),
            CloudDevice(deviceId: "", name: "no id", platform: nil, isOnline: true, isCurrent: false, appVersion: nil),
            CloudDevice(
                deviceId: "nas-bb22", name: "office", platform: "linux", isOnline: true, isCurrent: false,
                appVersion: nil, defaultSaveDir: " /srv/dl ", pathStyle: .windows
            ),
        ]
        let link = [
            LinkDevice(fingerprint: "fp:cc-33", name: "", platform: nil, online: true, defaultSaveDir: "/x", pathStyle: .posix),
            LinkDevice(fingerprint: "", name: "ghost", platform: nil, online: true),
        ]
        let targets = DeviceRules.dispatchTargets(cloud: cloud, link: link, cloudPresenceKnown: true, localReady: true)
        #expect(targets.map(\.id) == ["cloud:pc-aa11", "cloud:nas-bb22", "link:fp:cc-33"])
        // 同名（忽略大小写与首尾空白）追加短码；空名用短码。
        #expect(targets.map(\.name) == ["Office · aa11", "office · bb22", "cc33"])
        #expect(targets.map(\.online) == [false, true, true])
        // 空白目录读作无；自报风格优先于平台推断。
        #expect(targets[0].defaultSaveDir == nil)
        #expect(targets[0].pathStyle == .windows)
        #expect(targets[1].defaultSaveDir == "/srv/dl")
        #expect(targets[1].pathStyle == .windows)
        #expect(targets[0].isCloud && targets[0].isOffline)
        #expect(!targets[2].isCloud && !targets[2].isOffline)
    }

    @Test func dispatchTargetPresenceUnknownUntilTrusted() {
        let cloud = [CloudDevice(deviceId: "pc", name: "PC", platform: nil, isOnline: false, isCurrent: false, appVersion: nil)]
        let link = [LinkDevice(fingerprint: "fp", name: "Mac", platform: "macos", online: false)]
        // 云端 presence 不可信：云设备在线未知（不当作离线），局域网设备仍按本地探测。
        let cloudUnknown = DeviceRules.dispatchTargets(cloud: cloud, link: link, cloudPresenceKnown: false, localReady: true)
        #expect(cloudUnknown.map(\.online) == [nil, false])
        #expect(!cloudUnknown[0].isOffline)
        // 本地服务未就绪：两类都未知。
        let notReady = DeviceRules.dispatchTargets(cloud: cloud, link: link, cloudPresenceKnown: true, localReady: false)
        #expect(notReady.map(\.online) == [nil, nil])
    }

    @Test func linkAttachesToCloudEntryAndRoutes() {
        let cloud = [
            CloudDevice(deviceId: "pc", name: "PC", platform: "windows", isOnline: false, isCurrent: false, appVersion: nil),
            CloudDevice(deviceId: "nas", name: "NAS", platform: "linux", isOnline: true, isCurrent: false, appVersion: nil),
        ]
        let link = [
            // 挂接到 pc：云离线 + 直连在线 → 在线、走直连。
            LinkDevice(fingerprint: "fp-pc", name: "pc-direct", platform: "windows", online: true, cloudDeviceId: "pc"),
            // 同一云条目已被挂接：第二条 link 成为独立条目。
            LinkDevice(fingerprint: "fp-pc2", name: "pc-second", platform: nil, online: false, cloudDeviceId: "pc"),
            // revoked：不参与任何目标。
            LinkDevice(fingerprint: "fp-rev", name: "gone", platform: nil, online: true, cloudDeviceId: "nas", revoked: true),
            // cloudDeviceId 不在云名册：独立条目。
            LinkDevice(fingerprint: "fp-x", name: "Phone", platform: "ios", online: true, cloudDeviceId: "unknown"),
        ]
        let targets = DeviceRules.dispatchTargets(cloud: cloud, link: link, cloudPresenceKnown: true, localReady: true)
        #expect(targets.map(\.id) == ["cloud:pc", "cloud:nas", "link:fp-pc2", "link:fp-x"])
        #expect(targets[0].name == "PC")
        #expect(targets[0].online == true && targets[0].linkOnline == true)
        #expect(targets[0].routesViaLink && targets[0].dispatchFingerprint == "fp-pc")
        #expect(!targets[1].routesViaLink && targets[1].dispatchFingerprint == nil)
        #expect(targets[2].routesViaLink && targets[2].dispatchFingerprint == "fp-pc2")
        // 账户页「同一账号」标签只给真正挂接的记录（后到的同 id、cloudDeviceId 不在名册、revoked 都不算）。
        #expect(DeviceRules.attachedLinkFingerprints(cloud: cloud, link: link) == ["fp-pc"])
        // 持久化的 link:<fp> 解析为已挂接的云条目。
        #expect(DeviceRules.resolveTarget(id: "link:fp-pc", in: targets)?.id == "cloud:pc")
        #expect(DeviceRules.resolveTarget(id: "link:fp-rev", in: targets) == nil)

        // 直连离线：回落走云。
        let offline = [LinkDevice(fingerprint: "fp-pc", name: "x", platform: nil, online: false, cloudDeviceId: "pc")]
        let fallback = DeviceRules.dispatchTargets(cloud: cloud, link: offline, cloudPresenceKnown: true, localReady: true)
        #expect(fallback[0].online == false && !fallback[0].routesViaLink)
        // 本地未就绪：直连在线未知 → 合并后云离线也只能是未知。
        let notReady = DeviceRules.dispatchTargets(cloud: cloud, link: offline, cloudPresenceKnown: true, localReady: false)
        #expect(notReady[0].online == nil && !notReady[0].routesViaLink)
    }
}
