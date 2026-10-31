package com.fluxdown.core.protocol

import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** 站点代理规则：宽松解析表 / 往返 / 展示端点 / 写入（用例同 iOS `ProxyRulesProtocolTests`、引擎 `parse_raw`）。 */
class ProxyRulesTest {
    @Test
    fun invalidValuesParseToEmpty() {
        for (raw in listOf("", "  ", "nope", "{}", "42", "\"[]\"")) {
            assertTrue(raw, ProxyRule.parseList(raw).isEmpty())
        }
        assertTrue(ProxyRule.parseList(null).isEmpty())
    }

    @Test
    fun missingFieldsFallBackToDefaults() {
        val rule = ProxyRule.parseList("[{}]").single()
        assertEquals(ProxyRule(id = ""), rule)
        assertTrue(rule.enabled)
        assertEquals("domain", rule.match)
        assertEquals("direct", rule.action)
    }

    @Test
    fun wrongTypesAndUnknownValuesFallBackToDefaults() {
        val raw = """[{"id":7,"enabled":"no","name":null,"match":"glob","pattern":[],"action":"tunnel","proxy":1}]"""
        assertEquals(ProxyRule(id = ""), ProxyRule.parseList(raw).single())
    }

    @Test
    fun nonObjectElementsAreSkipped() {
        val raw = """[1,{"id":"a"},"x",null,{"id":"b","enabled":false}]"""
        val list = ProxyRule.parseList(raw)
        assertEquals(listOf("a", "b"), list.map { it.id })
        assertEquals(listOf(true, false), list.map { it.enabled })
    }

    @Test
    fun validFieldsAreKept() {
        val raw = """[{"id":"r","enabled":false,"name":"GH","match":"regex","pattern":"a","action":"custom","proxy":"socks5://h:1"}]"""
        assertEquals(
            ProxyRule("r", false, "GH", "regex", "a", "custom", "socks5://h:1"),
            ProxyRule.parseList(raw).single(),
        )
    }

    @Test
    fun encodeRoundTripsParsedValues() {
        val list = listOf(
            ProxyRule("a", true, "n", "domain", "github.com, x.org", "manual", ""),
            ProxyRule("b", false, "", "regex", "^https://\\d+", "custom", "http://u:p@h:8"),
        )
        assertEquals(list, ProxyRule.parseList(ProxyRule.encodeList(list)))
    }

    @Test
    fun enabledCountIgnoresDisabledAndInvalid() {
        assertEquals(0, ProxyRule.enabledCount(""))
        assertEquals(0, ProxyRule.enabledCount("garbage"))
        assertEquals(2, ProxyRule.enabledCount("""[{"id":"a"},{"id":"b","enabled":false},3,{"id":"c","enabled":true}]"""))
    }

    @Test
    fun displayEndpointStripsCredentialsAndTrailingSlash() {
        assertEquals("socks5://h:1", ProxyRule.displayEndpoint("socks5://u:p@h:1/"))
        assertEquals("http://h:8080", ProxyRule.displayEndpoint(" http://h:8080 "))
        assertEquals("h:1", ProxyRule.displayEndpoint("u@h:1"))
        assertEquals("socks5://[::1]:1", ProxyRule.displayEndpoint("socks5://a:b@[::1]:1"))
    }

    @Test
    fun manualConfiguredNeedsHostAndPortInRange() {
        assertTrue(ProxyRule.manualConfigured("h", "8080"))
        assertTrue(!ProxyRule.manualConfigured("", "8080"))
        assertTrue(!ProxyRule.manualConfigured("h", ""))
        assertTrue(!ProxyRule.manualConfigured("h", "0"))
        assertTrue(!ProxyRule.manualConfigured("h", "65536"))
    }

    @Test
    fun routeResponseParsesIssuesAndMatch() {
        val json = """{"issues":[{"index":2,"field":"pattern","code":"invalidRegex","detail":"x"}],"matched":{"index":0,"id":"a"},"reason":"direct","endpoint":""}"""
        val r = ProxyRouteResponse.fromJson(Json.parse(json))
        assertEquals(ProxyRuleIssue(2, "pattern", "invalidRegex", "x"), r.issues.single())
        assertEquals(ProxyRouteMatch(0, "a", ""), r.matched)
        assertEquals("direct", r.reason)
        assertNull(ProxyRouteResponse.fromJson(Json.parse("""{"matched":null,"reason":"modeNone"}""")).matched)
    }

    @Test
    fun requestOmitsRulesWhenNull() {
        assertEquals("""{"url":"u"}""", ProxyRouteRequest("u").toJson().toJson())
        assertEquals("""{"url":"u","rules":"[]"}""", ProxyRouteRequest("u", "[]").toJson().toJson())
    }

    // ── 写入 ──

    private class FakePort : DaemonConfigPort {
        var server = DaemonConfigSnapshot(1, emptyMap())
        var forcedConflicts = 0
        var patches = 0
        override val cachedConfig: DaemonConfigSnapshot? get() = server

        override suspend fun fetchConfig(): DaemonConfigSnapshot = server

        override suspend fun patchConfig(expectedRevision: Long, values: Map<String, String>) {
            patches += 1
            if (forcedConflicts > 0) {
                forcedConflicts -= 1
                server = DaemonConfigSnapshot(server.revision + 1, server.values)
                throw HostException(HostErrorCode.Conflict)
            }
            if (expectedRevision != server.revision) throw HostException(HostErrorCode.Conflict)
            server = DaemonConfigSnapshot(server.revision + 1, values)
        }

        fun seed(list: List<ProxyRule>) {
            server = DaemonConfigSnapshot(server.revision, mapOf(ProxyRule.CONFIG_KEY to ProxyRule.encodeList(list)))
        }

        val stored: List<ProxyRule> get() = ProxyRule.parseList(server.values[ProxyRule.CONFIG_KEY])
    }

    private fun rule(id: String) = ProxyRule(id = id, name = id, pattern = "$id.test")

    @Test
    fun moveRespectsBounds() = runBlocking {
        val port = FakePort().apply { seed(listOf(rule("a"), rule("b"), rule("c"))) }
        val writer = ProxyRuleWriter(port)
        writer.move("a", -1)
        writer.move("c", 1)
        writer.move("missing", 1)
        writer.move("b", 0)
        assertEquals(0, port.patches)
        writer.move("a", 1)
        assertEquals(listOf("b", "a", "c"), port.stored.map { it.id })
        writer.move("a", 1)
        assertEquals(listOf("b", "c", "a"), port.stored.map { it.id })
        assertEquals(2, port.patches)
    }

    @Test
    fun upsertSetEnabledAndRemove() = runBlocking {
        val port = FakePort().apply { seed(listOf(rule("a"), rule("b"))) }
        val writer = ProxyRuleWriter(port)
        writer.upsert(rule("a").copy(name = "renamed"))
        writer.upsert(rule("c"))
        assertEquals(listOf("renamed", "b", "c"), port.stored.map { it.name })
        writer.setEnabled("b", false)
        assertEquals(listOf(true, false, true), port.stored.map { it.enabled })
        writer.remove("a")
        assertEquals(listOf("b", "c"), port.stored.map { it.id })
    }

    @Test
    fun conflictReplaysOnTopOfLatestConfig() = runBlocking {
        val port = FakePort().apply { seed(listOf(rule("a"))) }
        port.forcedConflicts = 1
        ProxyRuleWriter(port).upsert(rule("b"))
        assertEquals(2, port.patches)
        assertEquals(listOf("a", "b"), port.stored.map { it.id })
    }
}
