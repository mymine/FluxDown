import FluxDomain
import Foundation
import Testing

/// Webhook DTO / 端点宽松解析 / 读-改-写（冲突重放 + 串行）/ 模板预览。
@MainActor
struct WebhooksProtocolTests {
    // MARK: 宽松解析

    @Test func parseListSkipsNonObjectsAndFallsBackToDefaultsForWrongTypes() {
        let raw = """
        [1, "x", null, [], {"id":"a","name":"A","url":"https://h/x","events":["task.completed", 3, null],
          "headers":{"X-A":"1","X-B":2},"enabled":"yes","allowHttp":1,"useProxy":true,"queueId":7},
         {"id":"b"}]
        """
        let list = WebhookEndpoint.parseList(raw)
        #expect(list.map(\.id) == ["a", "b"])
        let a = list[0]
        #expect(a.events == ["task.completed"])
        #expect(a.headers == ["X-A": "1"])
        // `enabled` 类型不符 → serde `default_true`；其余布尔回退 false；字符串字段回退空串。
        #expect(a.enabled && !a.allowHttp && a.useProxy && a.queueId.isEmpty)
        let b = list[1]
        #expect(b.enabled && b.events.isEmpty && b.name.isEmpty && b.headers.isEmpty)
    }

    @Test func parseListToleratesEmptyAndInvalidConfig() {
        #expect(WebhookEndpoint.parseList(nil).isEmpty)
        #expect(WebhookEndpoint.parseList("").isEmpty)
        #expect(WebhookEndpoint.parseList("  \n").isEmpty)
        #expect(WebhookEndpoint.parseList("{not json").isEmpty)
        #expect(WebhookEndpoint.parseList(#"{"id":"a"}"#).isEmpty)
    }

    @Test func encodeListRoundTripsWireShape() throws {
        let endpoint = WebhookEndpoint(
            id: "wh_1", name: "N", preset: "ntfy", url: "https://ntfy.sh/t", enabled: false,
            events: ["task.failed"], queueId: "main", headers: ["A": "b"], bodyTemplate: "{event}",
            signSecret: "whsec_x", allowHttp: true, useProxy: true
        )
        let text = try WebhookEndpoint.encodeList([endpoint])
        let object = try #require(try JSONSerialization.jsonObject(with: Data(text.utf8)) as? [[String: Any]])
        #expect(Set(object[0].keys) == [
            "id", "name", "preset", "url", "enabled", "events", "queueId", "headers",
            "bodyTemplate", "signSecret", "allowHttp", "useProxy",
        ])
        #expect(WebhookEndpoint.parseList(text) == [endpoint])
    }

    @Test func deliveriesResponseDecodesWithVariablesAndTolerantFields() throws {
        let json = """
        {"deliveries":[{"deliveryId":"d1","timestampMs":1700000000000,"event":"task.completed","endpointId":"e","endpointName":"E",
          "url":"https://x","requestHeaders":"A: b","requestBody":"{}","statusCode":0,"responseBody":"","latencyMs":12,"attempts":3,"success":false,"error":"timeout"}],
         "presets":[{"id":"ntfy","label":"ntfy","urlPlaceholder":"https://ntfy.sh/topic","defaultTemplate":"{event}","contentType":"text/plain"}],
         "variables":["{event}","{task.fileName}"]}
        """
        let response = try ProtocolJSON.decode(WebhookDeliveriesResponse.self, from: Data(json.utf8), what: "t")
        #expect(response.deliveries[0].statusSummary == "timeout")
        #expect(response.presets[0].contentType == "text/plain")
        #expect(response.variables == ["{event}", "{task.fileName}"])
        let ok = WebhookDelivery(deliveryId: "d", timestampMs: 1, event: "e", endpointId: "x", endpointName: "n", statusCode: 200, latencyMs: 40)
        #expect(ok.statusSummary == "200 · 40ms")
        let none = WebhookDelivery(deliveryId: "d", timestampMs: 1, event: "e", endpointId: "x", endpointName: "n", statusCode: 502, success: false)
        #expect(none.statusSummary == "HTTP 502")
    }

    @Test func latestDeliveryPicksNewestTimestampForTheEndpoint() {
        let list = [
            WebhookDelivery(deliveryId: "1", timestampMs: 10, event: "e", endpointId: "a", endpointName: "A"),
            WebhookDelivery(deliveryId: "2", timestampMs: 30, event: "e", endpointId: "a", endpointName: "A"),
            WebhookDelivery(deliveryId: "3", timestampMs: 50, event: "e", endpointId: "b", endpointName: "B"),
        ]
        #expect(latestWebhookDelivery(list, endpointId: "a")?.deliveryId == "2")
        #expect(latestWebhookDelivery(list, endpointId: "z") == nil)
    }

    // MARK: 读-改-写

    /// 模拟主机：真实的 revision 校验；`cachedConfig` 可被固定成过期值（store 发布滞后）。
    private final class FakePort: DaemonConfigPort {
        var server = DaemonConfigSnapshotDto(revision: 5, values: [:])
        var cache: DaemonConfigSnapshotDto?
        /// 前 n 次 patch 无条件抛 conflict（并让别的客户端先写入 `concurrent`）。
        var forcedConflicts = 0
        var concurrent: [WebhookEndpoint] = []
        var patches: [(revision: UInt64, list: [WebhookEndpoint])] = []
        var fetches = 0
        var failure: HostError?

        var cachedConfig: DaemonConfigSnapshotDto? { cache }

        func fetchConfig() async throws(HostError) -> DaemonConfigSnapshotDto {
            fetches += 1
            return server
        }

        func patchConfig(expectedRevision: UInt64, values: [String: String]) async throws(HostError) {
            await Task.yield()
            let list = WebhookEndpoint.parseList(values[WebhookEndpoint.configKey])
            patches.append((expectedRevision, list))
            if let failure { throw failure }
            if forcedConflicts > 0 {
                forcedConflicts -= 1
                var current = WebhookEndpoint.parseList(server.values[WebhookEndpoint.configKey])
                current.append(contentsOf: concurrent)
                concurrent = []
                server = DaemonConfigSnapshotDto(
                    revision: server.revision + 1,
                    values: [WebhookEndpoint.configKey: try WebhookEndpoint.encodeList(current)]
                )
                throw HostError(.conflict)
            }
            guard expectedRevision == server.revision else { throw HostError(.conflict) }
            server = DaemonConfigSnapshotDto(revision: server.revision + 1, values: values)
        }

        func seed(_ list: [WebhookEndpoint]) throws {
            server = DaemonConfigSnapshotDto(revision: server.revision, values: [WebhookEndpoint.configKey: try WebhookEndpoint.encodeList(list)])
            cache = server
        }

        var stored: [WebhookEndpoint] { WebhookEndpoint.parseList(server.values[WebhookEndpoint.configKey]) }
    }

    private func endpoint(_ id: String, enabled: Bool = true) -> WebhookEndpoint {
        WebhookEndpoint(id: id, name: id, url: "https://h/\(id)", enabled: enabled)
    }

    @Test func conflictReReadsLatestConfigAndReplaysTheMutationOnTopOfIt() async throws {
        let port = FakePort()
        try port.seed([endpoint("a")])
        port.forcedConflicts = 1
        port.concurrent = [endpoint("other-client")] // 冲突发生时别处刚加的端点
        let writer = WebhookEndpointWriter(port: port)

        try await writer.upsert(endpoint("c"))

        // 第一次基于缓存（rev 5），冲突后重读（rev 6，含 other-client）再重放：不得覆盖别处的修改。
        #expect(port.patches.count == 2)
        #expect(port.patches[0].revision == 5 && port.patches[0].list.map(\.id) == ["a", "c"])
        #expect(port.patches[1].revision == 6 && port.patches[1].list.map(\.id) == ["a", "other-client", "c"])
        #expect(port.fetches == 1)
        #expect(port.stored.map(\.id) == ["a", "other-client", "c"])
    }

    @Test func conflictRetriesAreCappedAtThree() async throws {
        let port = FakePort()
        try port.seed([endpoint("a")])
        port.forcedConflicts = 100
        let writer = WebhookEndpointWriter(port: port)
        await #expect(throws: HostError.self) { try await writer.remove(id: "a") }
        // 首次 + 3 次重放 = 4 次 patch，3 次重读。
        #expect(port.patches.count == 1 + WebhookEndpointWriter.maxConflictRetries)
        #expect(port.fetches == WebhookEndpointWriter.maxConflictRetries)
    }

    @Test func nonConflictErrorsPropagateWithoutRetry() async throws {
        let port = FakePort()
        try port.seed([endpoint("a")])
        port.failure = HostError(.invalidArgument, message: "bad")
        let writer = WebhookEndpointWriter(port: port)
        do throws(HostError) {
            try await writer.upsert(endpoint("b"))
            Issue.record("expected failure")
        } catch {
            #expect(error.code == .invalidArgument && error.message == "bad")
        }
        #expect(port.patches.count == 1 && port.fetches == 0)
    }

    @Test func missingCacheFetchesBeforeFirstWriteAndNoOpMutationsSkipThePatch() async throws {
        let port = FakePort()
        try port.seed([endpoint("a")])
        port.cache = nil
        let writer = WebhookEndpointWriter(port: port)
        try await writer.setEnabled(id: "missing", enabled: false) // 不存在 → 不写入
        #expect(port.patches.isEmpty && port.fetches == 1)
        try await writer.setEnabled(id: "a", enabled: false)
        #expect(port.stored.first?.enabled == false)
    }

    @Test func concurrentWritesAreSerializedEvenWithAStaleSnapshot() async throws {
        let port = FakePort()
        try port.seed([])
        let writer = WebhookEndpointWriter(port: port) // cache 永远停在 rev 5（发布滞后）
        async let first: Void = writer.upsert(endpoint("one"))
        async let second: Void = writer.upsert(endpoint("two"))
        async let third: Void = writer.upsert(endpoint("three"))
        try await first
        try await second
        try await third
        #expect(Set(port.stored.map(\.id)) == ["one", "two", "three"])
        // 串行：每次写入都基于上一次的结果，没有基于过期数组回写。
        #expect(port.stored.count == 3)
    }

    @Test func upsertReplacesByIdAndRemoveDeletes() async throws {
        let port = FakePort()
        try port.seed([endpoint("a"), endpoint("b")])
        let writer = WebhookEndpointWriter(port: port)
        var changed = endpoint("a")
        changed.name = "renamed"
        try await writer.upsert(changed)
        #expect(port.stored.map(\.name) == ["renamed", "b"])
        try port.seed(port.stored)
        try await writer.remove(id: "b")
        #expect(port.stored.map(\.id) == ["a"])
    }

    // MARK: 模板

    @Test func previewSubstitutesKnownPlaceholdersAndKeepsUnknownSegments() {
        #expect(
            WebhookTemplate.renderPreview(#"{"text":"{task.fileName} · {task.totalBytesHuman}"}"#, formEscape: false)
                == #"{"text":"ubuntu-24.04.2-desktop-amd64.iso · 6.0 GB"}"#
        )
        #expect(WebhookTemplate.renderPreview("{unknown} {{ {", formEscape: false) == "{unknown} {{ {")
        #expect(WebhookTemplate.renderPreview("q={event.summary}", formEscape: true) == "q=ubuntu-24.04.2-desktop-amd64.iso+%C2%B7+6.0+GB")
        #expect(WebhookTemplate.renderPreview(#""{event.title}""#, formEscape: false) == #""Download completed""#)
        #expect(WebhookTemplate.formEncode("a b-_.!~*'()") == "a+b-_.!~*'()")
        #expect(WebhookTemplate.formEncode("/?&=") == "%2F%3F%26%3D")
    }

    @Test func urlValidationRequiresHttpsUnlessPlaintextAllowed() {
        #expect(WebhookTemplate.urlErrorKey("", allowHttp: false) == nil)
        #expect(WebhookTemplate.urlErrorKey("https://ntfy.sh/topic", allowHttp: false) == nil)
        #expect(WebhookTemplate.urlErrorKey("http://192.168.1.2:8123/api", allowHttp: false) == "webhookUrlWarnHttp")
        #expect(WebhookTemplate.urlErrorKey("http://192.168.1.2:8123/api", allowHttp: true) == nil)
        #expect(WebhookTemplate.urlErrorKey("ftp://host", allowHttp: true) == "webhookUrlInvalid")
        #expect(WebhookTemplate.urlErrorKey("https:///path", allowHttp: true) == "webhookUrlInvalid")
        #expect(WebhookTemplate.urlErrorKey("not a url", allowHttp: true) == "webhookUrlInvalid")
    }

    @Test func generatedSecretIsWhsecPlus32LowerHexAndDiffersEachTime() {
        let secret = WebhookTemplate.generateSecret()
        #expect(secret.count == 38 && secret.hasPrefix("whsec_"))
        #expect(secret.dropFirst(6).allSatisfy { $0.isHexDigit && !$0.isUppercase })
        #expect(WebhookTemplate.generateSecret() != secret)
    }

    @Test func requestPreviewBuildsEnvelopeOrPresetBody() throws {
        let envelope = WebhookTemplate.previewRequest(url: "", firstEvent: nil, signEnabled: true, template: "", preset: nil)
        #expect(envelope.hasPrefix("POST \nContent-Type: application/json\nX-FluxDown-Event: task.completed"))
        #expect(envelope.contains("X-FluxDown-Signature: t=1789647128,v1=9c41f2…"))
        #expect(envelope.contains(#""schemaVersion": 1"#))
        // 信封必须是合法 JSON。
        let body = try #require(envelope.components(separatedBy: String(repeating: "─", count: 28)).last)
        _ = try JSONSerialization.jsonObject(with: Data(body.utf8))

        let preset = WebhookPreset(id: "slack", label: "Slack", urlPlaceholder: "https://hooks.slack.com/x", defaultTemplate: #"{"text":"{task.fileName}"}"#, contentType: "application/json")
        let text = WebhookTemplate.previewRequest(url: " https://h/x ", firstEvent: "task.failed", signEnabled: false, template: "", preset: preset)
        #expect(text.contains("POST https://h/x\n"))
        #expect(text.contains("X-FluxDown-Event: task.failed"))
        #expect(!text.contains("X-FluxDown-Signature"))
        #expect(text.hasSuffix("{\n  \"text\": \"ubuntu-24.04.2-desktop-amd64.iso\"\n}"))
        let form = WebhookPreset(id: "bark", label: "Bark", defaultTemplate: "title={event.title}", contentType: "application/x-www-form-urlencoded")
        #expect(WebhookTemplate.previewBody(ownTemplate: "", preset: form) == "title=Download+completed")
    }

    @Test func prettyJSONKeepsKeyOrderAndEmptyContainers() {
        #expect(WebhookTemplate.prettyJSON(#"{"b":1,"a":[],"c":{}}"#) == "{\n  \"b\": 1,\n  \"a\": [],\n  \"c\": {}\n}")
        #expect(WebhookTemplate.prettyJSON("not json") == nil)
    }
}
