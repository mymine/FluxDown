import FluxDomain
import Foundation
import Testing

/// 站点规则：宽松解析表、往返一致、`enabledCount`、`displayEndpoint`、`ProxyRuleWriter` 的 move 边界与冲突重放、DTO 解码。
/// 与引擎 / protocol / Web / Android 的解析器行为逐条一致（规则模型表）。
@MainActor
struct ProxyRulesProtocolTests {
    // MARK: 宽松解析

    @Test func invalidValuesParseToEmpty() {
        for raw in ["", "  ", "nope", "{}", "42", "\"[]\"", "null"] {
            #expect(ProxyRule.parseList(raw).isEmpty, "\(raw)")
        }
        #expect(ProxyRule.parseList(nil).isEmpty)
    }

    @Test func missingOrWrongTypedFieldsFallBackToDefaults() {
        let raw = """
        [1, "x", null, [], {}, {"id": 5, "enabled": "no", "name": null, "match": "glob", "pattern": 7, "action": "tunnel", "proxy": false}]
        """
        let list = ProxyRule.parseList(raw)
        #expect(list.count == 2)
        let defaults = ProxyRule(id: "")
        #expect(list[0] == defaults)
        #expect(list[1] == defaults)
        #expect(defaults.enabled && defaults.match == "domain" && defaults.action == "direct")
        #expect(defaults.name == "" && defaults.pattern == "" && defaults.proxy == "")
    }

    @Test func validValuesAreKept() {
        let raw = """
        [{"id":"pr_1","enabled":false,"name":"GitHub","match":"regex","pattern":"github\\\\.com/","action":"custom","proxy":"socks5://u:p@127.0.0.1:7891"},
         {"id":"pr_2","match":"domain","pattern":"a.com, b.com","action":"manual"},
         {"id":"pr_3","action":"system"}]
        """
        let list = ProxyRule.parseList(raw)
        #expect(list.map(\.id) == ["pr_1", "pr_2", "pr_3"])
        #expect(list[0] == ProxyRule(
            id: "pr_1", enabled: false, name: "GitHub", match: "regex", pattern: "github\\.com/",
            action: "custom", proxy: "socks5://u:p@127.0.0.1:7891"
        ))
        #expect(list[1].enabled && list[1].action == "manual" && list[1].pattern == "a.com, b.com")
        #expect(list[2].action == "system" && list[2].match == "domain")
    }

    // MARK: 往返

    @Test func encodeThenParseRoundTripsAndWritesAllSevenFields() throws {
        let list = [
            ProxyRule(id: "a", enabled: false, name: "N", match: "regex", pattern: "^https://x/\\d+", action: "custom", proxy: "http://h:1"),
            ProxyRule(id: "b"),
        ]
        let encoded = try ProxyRule.encodeList(list)
        #expect(ProxyRule.parseList(encoded) == list)
        let value = try ProtocolJSON.makeDecoder().decode(JSONValue.self, from: Data(encoded.utf8))
        guard case let .array(items) = value, case let .object(first) = items[0] else {
            Issue.record("expected array of objects")
            return
        }
        #expect(Set(first.keys) == ["id", "enabled", "name", "match", "pattern", "action", "proxy"])
    }

    // MARK: 计数 / 端点

    @Test func enabledCountSkipsDisabled() throws {
        let raw = try ProxyRule.encodeList([ProxyRule(id: "a"), ProxyRule(id: "b", enabled: false), ProxyRule(id: "c")])
        #expect(ProxyRule.enabledCount(raw) == 2)
        #expect(ProxyRule.enabledCount("") == 0)
        #expect(ProxyRule.enabledCount(nil) == 0)
        #expect(ProxyRule.enabledCount("garbage") == 0)
    }

    @Test func displayEndpointStripsCredentialsAndTrailingSlash() {
        #expect(ProxyRule.displayEndpoint("socks5://user:pass@127.0.0.1:7891") == "socks5://127.0.0.1:7891")
        #expect(ProxyRule.displayEndpoint("socks5://u:p@h:1/") == "socks5://h:1")
        #expect(ProxyRule.displayEndpoint(" http://h:8080 ") == "http://h:8080")
        #expect(ProxyRule.displayEndpoint("u:p@h:1") == "h:1")
        #expect(ProxyRule.displayEndpoint("h:1") == "h:1")
    }

    // MARK: 纯变换

    @Test func upsertingReplacesByIdOrAppends() {
        let list = [ProxyRule(id: "a", name: "old"), ProxyRule(id: "b")]
        #expect(ProxyRule.upserting(ProxyRule(id: "a", name: "new"), in: list).map(\.name) == ["new", ""])
        #expect(ProxyRule.upserting(ProxyRule(id: "c"), in: list).map(\.id) == ["a", "b", "c"])
    }

    @Test func movingHandlesBoundaries() {
        let list = ["a", "b", "c"].map { ProxyRule(id: $0) }
        #expect(ProxyRule.moving(id: "a", by: -1, in: list) == nil)
        #expect(ProxyRule.moving(id: "c", by: 1, in: list) == nil)
        #expect(ProxyRule.moving(id: "x", by: 1, in: list) == nil)
        #expect(ProxyRule.moving(id: "b", by: 0, in: list) == nil)
        #expect(ProxyRule.moving(id: "a", by: 1, in: list)?.map(\.id) == ["b", "a", "c"])
        #expect(ProxyRule.moving(id: "c", by: -1, in: list)?.map(\.id) == ["a", "c", "b"])
    }

    // MARK: 读-改-写

    private final class FakePort: DaemonConfigPort {
        var server = DaemonConfigSnapshotDto(revision: 5, values: [:])
        var cache: DaemonConfigSnapshotDto?
        var forcedConflicts = 0
        var concurrent: [ProxyRule] = []
        var patches: [(revision: UInt64, list: [ProxyRule])] = []
        var fetches = 0

        var cachedConfig: DaemonConfigSnapshotDto? { cache }

        func fetchConfig() async throws(HostError) -> DaemonConfigSnapshotDto {
            fetches += 1
            return server
        }

        func patchConfig(expectedRevision: UInt64, values: [String: String]) async throws(HostError) {
            await Task.yield()
            patches.append((expectedRevision, ProxyRule.parseList(values[ProxyRule.configKey])))
            if forcedConflicts > 0 {
                forcedConflicts -= 1
                var current = ProxyRule.parseList(server.values[ProxyRule.configKey])
                current.append(contentsOf: concurrent)
                concurrent = []
                server = DaemonConfigSnapshotDto(
                    revision: server.revision + 1, values: [ProxyRule.configKey: try ProxyRule.encodeList(current)]
                )
                throw HostError(.conflict)
            }
            guard expectedRevision == server.revision else { throw HostError(.conflict) }
            server = DaemonConfigSnapshotDto(revision: server.revision + 1, values: values)
            // 真实宿主随后经快照 / 事件把写入回流到缓存。
            cache = server
        }

        func seed(_ list: [ProxyRule]) throws {
            server = DaemonConfigSnapshotDto(revision: server.revision, values: [ProxyRule.configKey: try ProxyRule.encodeList(list)])
            cache = server
        }

        var stored: [ProxyRule] { ProxyRule.parseList(server.values[ProxyRule.configKey]) }
    }

    private func rules(_ ids: String...) -> [ProxyRule] { ids.map { ProxyRule(id: $0, name: $0) } }

    @Test func moveWritesOnlyWhenInBounds() async throws {
        let port = FakePort()
        try port.seed(rules("a", "b", "c"))
        let writer = ProxyRuleWriter(port: port)
        try await writer.move(id: "a", by: -1)
        try await writer.move(id: "c", by: 1)
        try await writer.move(id: "missing", by: 1)
        #expect(port.patches.isEmpty)
        try await writer.move(id: "a", by: 1)
        #expect(port.stored.map(\.id) == ["b", "a", "c"])
        #expect(port.patches.count == 1)
    }

    @Test func moveToOffsetFollowsOnMoveSemantics() async throws {
        let port = FakePort()
        try port.seed(rules("a", "b", "c"))
        let writer = ProxyRuleWriter(port: port)
        try await writer.move(id: "a", toOffset: 3) // 拖到末尾
        #expect(port.stored.map(\.id) == ["b", "c", "a"])
        try await writer.move(id: "a", toOffset: 0) // 拖到开头
        #expect(port.stored.map(\.id) == ["a", "b", "c"])
        let before = port.patches.count
        try await writer.move(id: "b", toOffset: 2) // 原位（插入到 c 之前）→ 不写入
        #expect(port.patches.count == before)
    }

    @Test func conflictReplaysTheMoveOnTopOfTheLatestList() async throws {
        let port = FakePort()
        try port.seed(rules("a", "b"))
        port.forcedConflicts = 1
        port.concurrent = rules("other")
        let writer = ProxyRuleWriter(port: port)
        try await writer.move(id: "a", by: 1)
        #expect(port.patches.count == 2)
        #expect(port.patches[0].list.map(\.id) == ["b", "a"])
        #expect(port.patches[1].revision == 6)
        #expect(port.patches[1].list.map(\.id) == ["b", "a", "other"])
        #expect(port.stored.map(\.id) == ["b", "a", "other"])
    }

    @Test func upsertSetEnabledAndRemove() async throws {
        let port = FakePort()
        try port.seed(rules("a", "b"))
        let writer = ProxyRuleWriter(port: port)
        try await writer.upsert(ProxyRule(id: "b", name: "B2"))
        try await writer.upsert(ProxyRule(id: "c"))
        try await writer.setEnabled(id: "a", enabled: false)
        try await writer.remove(id: "b")
        #expect(port.stored.map(\.id) == ["a", "c"])
        #expect(port.stored.first?.enabled == false)
    }

    // MARK: DTO

    @Test func routeResponseDecodesLeniently() throws {
        let decoder = ProtocolJSON.makeDecoder()
        let full = try decoder.decode(ProxyRouteResponse.self, from: Data("""
        {"issues":[{"index":2,"field":"pattern","code":"invalidRegex","detail":"x"}],
         "matched":{"index":0,"id":"a","name":"N"},"reason":"proxy","endpoint":"socks5://h:1"}
        """.utf8))
        #expect(full.issues == [ProxyRuleIssue(index: 2, field: "pattern", code: "invalidRegex", detail: "x")])
        #expect(full.matched == ProxyRouteMatch(index: 0, id: "a", name: "N"))
        #expect(full.reason == "proxy" && full.endpoint == "socks5://h:1")
        let empty = try decoder.decode(ProxyRouteResponse.self, from: Data("{}".utf8))
        #expect(empty == ProxyRouteResponse())
    }

    @Test func routeRequestOmitsNilRules() throws {
        let data = try ProtocolJSON.encode(ProxyRouteRequest(url: "https://x"), what: "req")
        #expect(String(decoding: data, as: UTF8.self) == "{\"url\":\"https://x\"}")
        let withRules = try ProtocolJSON.encode(ProxyRouteRequest(url: "", rules: "[]"), what: "req")
        #expect(String(decoding: withRules, as: UTF8.self) == "{\"rules\":\"[]\",\"url\":\"\"}")
    }
}
