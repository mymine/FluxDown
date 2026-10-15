import FluxDomain
import Foundation
import Testing

/// 队列 wire DTO 与 D7 / D8 纯逻辑：定时位掩码、HH:MM、表单校验、待处理顺序。
struct QueuesProtocolTests {
    // MARK: Wire

    @Test func queueDtoDecodesServerShape_andMissingDefaultedFieldsFallBack() throws {
        let full = Data(#"""
        {"queueId":"q1","name":"夜间","speedLimitKbps":512,"uploadLimitKbps":64,"maxConcurrent":3,
         "defaultSaveDir":"/data/n","position":2,"defaultSegments":8,"defaultUserAgent":"UA/1",
         "isRunning":false,"scheduleEnabled":true,"scheduleStart":"01:30","scheduleStop":"07:00","scheduleDays":31}
        """#.utf8)
        let queue = try ProtocolJSON.decode(QueueDetail.self, from: full, what: "queue")
        #expect(queue.queueId == "q1")
        #expect(queue.speedLimitKbps == 512 && queue.uploadLimitKbps == 64)
        #expect(queue.defaultSegments == 8 && queue.defaultUserAgent == "UA/1")
        #expect(!queue.isRunning && queue.scheduleEnabled)
        #expect(queue.scheduleDays == 31)
        #expect(!queue.isBuiltin)

        // 旧主机不下发 #[serde(default)] 字段：运行中 / 定时关 / 每天。
        let legacy = Data(#"{"queueId":"main","name":"Main","speedLimitKbps":0,"maxConcurrent":0,"defaultSaveDir":"","position":0,"defaultSegments":0,"defaultUserAgent":""}"#.utf8)
        let main = try ProtocolJSON.decode(QueueDetail.self, from: legacy, what: "queue")
        #expect(main.isRunning)
        #expect(!main.scheduleEnabled && main.scheduleStart.isEmpty)
        #expect(main.scheduleDays == 127)
        #expect(main.uploadLimitKbps == 0)
        #expect(main.isBuiltin)
    }

    @Test func updateParamsFlattenQueueIdWithInput() throws {
        let input = QueueInput(name: "A", speedLimitKbps: 10, uploadLimitKbps: 5, maxConcurrent: 2, defaultSaveDir: "/d", defaultSegments: 4, defaultUserAgent: "ua")
        let data = try ProtocolJSON.encode(QueueUpdateParams(queueId: "q9", input: input), what: "update")
        let json = try ProtocolJSON.decode(JSONValue.self, from: data, what: "update")
        #expect(json["queueId"]?.stringValue == "q9")
        #expect(json["name"]?.stringValue == "A")
        #expect(json["speedLimitKbps"]?.intValue == 10)
        #expect(json["defaultSegments"]?.intValue == 4)
        #expect(json.objectValue?.count == 8)
    }

    @Test func scheduleParamsEncodeWireNames() throws {
        let data = try ProtocolJSON.encode(QueueScheduleParams(queueId: "q", enabled: true, startTime: "09:00", stopTime: "", days: 31), what: "schedule")
        #expect(String(decoding: data, as: UTF8.self) == #"{"days":31,"enabled":true,"queueId":"q","startTime":"09:00","stopTime":""}"#)
    }

    // MARK: 星期位掩码

    @Test func weekdayMask_zeroMeansEveryDay_andMapsBitsMondayFirst() {
        #expect(WeekdayMask.normalized(0) == 127)
        #expect(WeekdayMask.normalized(127) == 127)
        #expect(WeekdayMask.normalized(0b1_0000_001) == 1)
        #expect(WeekdayMask.contains(0b0000001, day: 0))
        #expect(!WeekdayMask.contains(0b0000001, day: 1))
        #expect(WeekdayMask.contains(0b1000000, day: 6))
        #expect(WeekdayMask.contains(0, day: 3))
        #expect(!WeekdayMask.contains(127, day: 7))
        #expect(WeekdayMask.days(0b0010101) == [0, 2, 4])
    }

    @Test func weekdayMask_toggling_keepsAtLeastOneDay() {
        #expect(WeekdayMask.toggling(127, day: 6) == 0b0111111)
        #expect(WeekdayMask.toggling(0b0111111, day: 6) == 127)
        // 取消最后一天：原样返回，不会得到线上 0（= 每天）的歧义值。
        #expect(WeekdayMask.toggling(0b0000100, day: 2) == 0b0000100)
        // 线上 0 展示为每天，取消一天后得到其余六天。
        #expect(WeekdayMask.toggling(0, day: 0) == 0b1111110)
    }

    @Test func weekdayMask_runsGroupConsecutiveDays() {
        #expect(WeekdayMask.runs(127) == [0 ... 6])
        #expect(WeekdayMask.runs(0b0011111) == [0 ... 4])
        #expect(WeekdayMask.runs(0b1100000) == [5 ... 6])
        #expect(WeekdayMask.runs(0b1010101) == [0 ... 0, 2 ... 2, 4 ... 4, 6 ... 6])
        #expect(WeekdayMask.isEveryDay(0))
        #expect(!WeekdayMask.isEveryDay(0b0111111))
    }

    // MARK: HH:MM

    @Test func scheduleTime_parsesAndFormats() {
        #expect(ScheduleTime.parse("09:05") == 545)
        #expect(ScheduleTime.parse("9:5") == 545)
        #expect(ScheduleTime.parse(" 23:59 ") == 1439)
        #expect(ScheduleTime.parse("00:00") == 0)
        #expect(ScheduleTime.parse("") == nil)
        #expect(ScheduleTime.parse("24:00") == nil)
        #expect(ScheduleTime.parse("12:60") == nil)
        #expect(ScheduleTime.parse("12") == nil)
        #expect(ScheduleTime.parse("aa:bb") == nil)
        #expect(ScheduleTime.parse("123:00") == nil)
        #expect(ScheduleTime.format(545) == "09:05")
        #expect(ScheduleTime.format(nil) == "")
        #expect(ScheduleTime.format(1440) == "")
        #expect(ScheduleTime.format(0) == "00:00")
    }

    @Test func scheduleTime_minuteChoicesIncludeOffGridCurrentValue() {
        #expect(ScheduleTime.minuteChoices(current: 0) == Array(stride(from: 0, to: 60, by: 5)))
        let withOdd = ScheduleTime.minuteChoices(current: 7)
        #expect(withOdd.contains(7) && withOdd == withOdd.sorted() && withOdd.count == 13)
    }

    // MARK: 表单

    @Test func draftValidation_nameNumbersAndSchedule() throws {
        var draft = QueueDraft()
        draft.name = "  "
        #expect(draft.validated(builtin: false) == .failure(.nameRequired))
        // 内置队列不要求名称，且沿用原名。
        #expect(try draft.validated(builtin: true, existingName: "Main").get().name == "Main")

        draft.name = " Night "
        draft.segments = "65"
        #expect(draft.validated(builtin: false) == .failure(.invalidNumber))
        draft.segments = "64"
        draft.speedLimit = "-1"
        #expect(draft.validated(builtin: false) == .failure(.invalidNumber))
        draft.speedLimit = "1.5"
        #expect(draft.validated(builtin: false) == .failure(.invalidNumber))
        draft.speedLimit = ""
        draft.uploadLimit = "128"
        draft.maxConcurrent = "3"
        draft.saveDir = " /data/x "
        draft.userAgent = " UA "
        let input = try draft.validated(builtin: false).get()
        #expect(input == QueueInput(name: "Night", speedLimitKbps: 0, uploadLimitKbps: 128, maxConcurrent: 3, defaultSaveDir: "/data/x", defaultSegments: 64, defaultUserAgent: "UA"))

        draft.scheduleEnabled = true
        #expect(draft.validated(builtin: false) == .failure(.scheduleNeedsOneTime))
        draft.scheduleStop = 18 * 60
        _ = try draft.validated(builtin: false).get()
        let schedule = draft.schedule(queueId: "q")
        #expect(schedule == QueueScheduleParams(queueId: "q", enabled: true, startTime: "", stopTime: "18:00", days: 127))
        #expect(draft.needsScheduleCall(queueId: "q"))
        #expect(!draft.needsScheduleCall(queueId: "main"))
        #expect(!draft.needsScheduleCall(queueId: ""))
        #expect(draft.needsScheduleCall(queueId: "later"))
    }

    @Test func draftRoundTripsExistingQueue() {
        let queue = QueueDetail(queueId: "q", name: "N", speedLimitKbps: 1, uploadLimitKbps: 2, maxConcurrent: 3, defaultSaveDir: "/s", defaultSegments: 4, defaultUserAgent: "u", scheduleEnabled: true, scheduleStart: "08:30", scheduleStop: "", scheduleDays: 0)
        let draft = QueueDraft(queue: queue)
        #expect(draft.scheduleStart == 510 && draft.scheduleStop == nil)
        #expect(draft.days == 127)
        #expect(draft.schedule(queueId: "q").startTime == "08:30")
        #expect(!QueueDraft().needsScheduleCall(queueId: "q"))
    }

    @Test func mainQueueIsAlwaysRunningAndUnscheduled() {
        #expect(!QueueIds.canStop("main"))
        #expect(!QueueIds.canSchedule("main"))
        #expect(!TaskQueue(queueId: "main", name: "").canStop)
        #expect(!TaskQueue(queueId: "main", name: "").canSchedule)
        #expect(TaskQueue(queueId: "later", name: "").canStop)
        #expect(TaskQueue(queueId: "later", name: "").canSchedule)
        #expect(TaskQueue(queueId: "custom", name: "x").canStop)
        #expect(TaskQueue(queueId: "custom", name: "x").canSchedule)
    }

    @Test func createdQueueLookup_picksNewestByPosition() {
        let list = [
            QueueDetail(queueId: "a", name: "X", position: 1),
            QueueDetail(queueId: "b", name: "X", position: 5),
            QueueDetail(queueId: "c", name: "Y", position: 9),
        ]
        #expect(QueueLookup.created(named: "X", in: list)?.queueId == "b")
        #expect(QueueLookup.created(named: "Z", in: list) == nil)
    }

    // MARK: 待处理顺序

    private func task(_ id: String, _ status: TaskStatus, queue: String = "", created: Int64 = 0) -> DownloadTask {
        DownloadTask(taskId: id, url: "https://x/\(id)", fileName: id, status: status, createdAt: created, queueId: queue)
    }

    @Test func pendingOrderUsesQueuePositions_thenCreationTime() {
        let tasks = [
            task("a", .pending, created: 30),
            task("b", .pending, queue: "main", created: 20),
            task("c", .pending, created: 10),
            task("d", .downloading),
            task("e", .pending, queue: "later"),
        ]
        let ordered = QueueOrdering.pending(in: "main", tasks: tasks, positions: ["a": 1, "b": 2])
        #expect(ordered.map(\.taskId) == ["a", "b", "c"])
        #expect(QueueOrdering.pending(in: "later", tasks: tasks, positions: [:]).map(\.taskId) == ["e"])
    }

    @Test func reorderHelpers() {
        #expect(QueueOrdering.moved(["a", "b", "c"], index: 0, by: 1) == ["b", "a", "c"])
        #expect(QueueOrdering.moved(["a", "b", "c"], index: 0, by: -1) == nil)
        #expect(QueueOrdering.moved(["a", "b", "c"], index: 2, by: 1) == nil)
        #expect(QueueOrdering.moved(["a", "b", "c", "d"], from: IndexSet(integer: 0), to: 3) == ["b", "c", "a", "d"])
        #expect(QueueOrdering.moved(["a", "b", "c", "d"], from: IndexSet(integer: 3), to: 1) == ["a", "d", "b", "c"])
        #expect(QueueOrdering.moved(["a", "b", "c"], from: IndexSet([0, 1]), to: 3) == ["c", "a", "b"])
    }

    @Test func queueIds() {
        #expect(QueueIds.normalized("") == "main")
        #expect(QueueIds.normalized("later") == "later")
        #expect(QueueIds.isBuiltin("main") && QueueIds.isBuiltin("later") && !QueueIds.isBuiltin("q1"))
    }
}
