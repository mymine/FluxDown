import FluxDomain
import FluxUI
import Foundation
import Testing
@testable import FluxDown

/// R2 条目流的纯逻辑：状态 chip（含关联任务联动）、可见条目搜索 / 排序、日期与体积文案、编辑器表单。
@MainActor
struct RssItemsTests {
    private func item(
        _ guid: String,
        title: String = "T",
        status: Int32 = 0,
        taskId: String = "",
        pubDate: Int64 = 0,
        length: Int64 = 0,
        reason: String = ""
    ) -> RssItemDto {
        RssItemDto(sourceId: "s", guid: guid, title: title, enclosureLength: length, pubDate: pubDate, status: status, taskId: taskId, reason: reason)
    }

    // MARK: chip

    @Test func nonDownloadedStatusesMapToTheirOwnLabels() {
        #expect(RssFormat.chip(for: item("a", status: 0), task: nil).titleKey == "rssStatusNew")
        #expect(RssFormat.chip(for: item("a", status: 2), task: nil).titleKey == "rssStatusIgnored")
        #expect(RssFormat.chip(for: item("a", status: 3), task: nil).titleKey == "rssStatusFiltered")
        #expect(RssFormat.chip(for: item("a", status: 4), task: nil).titleKey == "rssStatusDuplicate")
        #expect(RssFormat.chip(for: item("a", status: 5), task: nil).titleKey == "rssStatusHistory")
    }

    @Test func downloadedItemFollowsLinkedTaskState() {
        let downloaded = item("a", status: 1, taskId: "t1")
        func key(_ task: RssLinkedTask?) -> String { RssFormat.chip(for: downloaded, task: task).titleKey }
        #expect(key(RssLinkedTask(status: .pending)) == "statusPending")
        #expect(key(RssLinkedTask(status: .downloading, fraction: 0.4)) == "statusDownloading")
        #expect(key(RssLinkedTask(status: .paused)) == "statusPaused")
        #expect(key(RssLinkedTask(status: .completed)) == "statusCompleted")
        #expect(key(RssLinkedTask(status: .completed, fileMissing: true)) == "statusIncomplete")
        #expect(key(RssLinkedTask(status: .failed)) == "statusError")
        #expect(key(RssLinkedTask(status: .preparing)) == "statusPreparing")
        #expect(key(RssLinkedTask(status: .unknown)) == "rssTaskCreated")
        #expect(RssFormat.chip(for: downloaded, task: RssLinkedTask(status: .downloading, fraction: 0.4)).progress == 0.4)
    }

    @Test func deletedOrUnlinkedTaskIsMarkedMissing() {
        #expect(RssFormat.chip(for: item("a", status: 1, taskId: "gone"), task: nil).titleKey == "rssTaskMissing")
        #expect(RssFormat.chip(for: item("a", status: 1, taskId: ""), task: RssLinkedTask(status: .completed)).titleKey == "rssTaskMissing")
        #expect(RssFormat.chip(for: item("a", status: 1, taskId: "gone"), task: nil).tone == .failure)
    }

    /// 任务表分页载入中查不到关联任务不代表已删除：不得显示「任务已删除」。
    @Test func missingLinkedTaskWhileTasksLoadAreNotMarkedMissing() {
        let downloaded = item("a", status: 1, taskId: "t1")
        #expect(RssFormat.chip(for: downloaded, task: nil, tasksLoading: true).titleKey == "rssTaskCreated")
        // 已查到的任务仍以真实状态为准；载入完成后查不到才是「已删除」。
        #expect(RssFormat.chip(for: downloaded, task: RssLinkedTask(status: .paused), tasksLoading: true).titleKey == "statusPaused")
        #expect(RssFormat.chip(for: downloaded, task: nil, tasksLoading: false).titleKey == "rssTaskMissing")
    }

    // MARK: 可见条目

    @Test func visibleFiltersByTitleCaseInsensitively() {
        let items = [item("1", title: "One Piece 1080p", pubDate: 10), item("2", title: "Bleach", pubDate: 20)]
        #expect(RssFormat.visible(items, query: "  PIECE ", oldestFirst: false).map(\.guid) == ["1"])
        #expect(RssFormat.visible(items, query: "", oldestFirst: false).map(\.guid) == ["2", "1"])
    }

    @Test func visibleSortsByDateWithUndatedLastAndStableTies() {
        let items = [
            item("undated-a", pubDate: 0),
            item("old", pubDate: 100),
            item("new", pubDate: 300),
            item("tie-a", pubDate: 200),
            item("tie-b", pubDate: 200),
            item("undated-b", pubDate: 0),
        ]
        #expect(RssFormat.visible(items, query: "", oldestFirst: false).map(\.guid)
            == ["new", "tie-a", "tie-b", "old", "undated-a", "undated-b"])
        #expect(RssFormat.visible(items, query: "", oldestFirst: true).map(\.guid)
            == ["old", "tie-a", "tie-b", "new", "undated-a", "undated-b"])
    }

    // MARK: 文案

    @Test func dateTextIsLocalAndEmptyWithoutTimestamp() {
        #expect(RssFormat.dateText(0) == "")
        #expect(RssFormat.dateText(-1) == "")
        let text = RssFormat.dateText(1_700_000_000)
        #expect(!text.isEmpty)
        #expect(!text.contains("UTC"))
    }

    @Test func bytesTextSharesAppFormat() {
        #expect(RssFormat.bytesText(0) == "")
        #expect(RssFormat.bytesText(512) == Format.bytes(512).description)
        #expect(RssFormat.bytesText(734_003_200) == Format.bytes(734_003_200).description)
    }

    @Test func queueLabelLocalizesBuiltinQueuesOnly() {
        #expect(RssFormat.queueLabel(id: "", name: "") == L("mainQueue"))
        #expect(RssFormat.queueLabel(id: TaskQueue.main, name: "x") == L("mainQueue"))
        #expect(RssFormat.queueLabel(id: TaskQueue.later, name: "x") == L("laterQueue"))
        #expect(RssFormat.queueLabel(id: "q1", name: "Anime") == "Anime")
        #expect(RssFormat.queueLabel(id: "q1", name: "") == "q1")
    }

    // MARK: 编辑器表单

    @Test func editorFormRoundTripsSourceFieldsAndAppliesEngineDefaults() {
        let source = RssSourceDetail(
            sourceId: "s1", url: "https://f", name: "Feed", enabled: false, autoDownload: false, startPaused: true,
            queueId: "", saveDir: "/data", intervalMinutes: 0, includePattern: "a|b", excludePattern: "c", useRegex: true,
            smartEpisode: true, sizeMinBytes: 209_715_200, sizeMaxBytes: 2_147_483_648, sendReferer: false,
            notifyOnDownload: false, maxPerFetch: 0, cookies: "k=v", userAgent: "UA", proxyUrl: "http://p"
        )
        let form = RssEditorModel.Form(source)
        #expect(form.queueId == TaskQueue.main)
        #expect(form.interval == 30)
        #expect(form.maxPerFetch == "20")
        #expect(form.sizeMin == "200M")
        #expect(form.sizeMax == "2G")
        #expect(!form.enabled && !form.autoDownload && form.startPaused && form.useRegex && form.smartEpisode)
        #expect(!form.sendReferer && !form.notifyOnDownload)
        #expect(form.include == "a|b" && form.exclude == "c" && form.saveDir == "/data")
        #expect(form.cookies == "k=v" && form.userAgent == "UA" && form.proxyUrl == "http://p")
    }

    @Test func newEditorFormMatchesCreateDefaults() {
        let form = RssEditorModel.Form()
        #expect(form.interval == 30)
        #expect(form.queueId == TaskQueue.main)
        #expect(form.enabled && form.autoDownload && form.sendReferer && form.notifyOnDownload)
        #expect(!form.startPaused && !form.useRegex && !form.smartEpisode)
        #expect(form.maxPerFetch == "20")
    }
}
