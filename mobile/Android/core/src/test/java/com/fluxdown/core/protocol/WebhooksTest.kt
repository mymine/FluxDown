package com.fluxdown.core.protocol

import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Webhook DTO / 端点宽松解析 / 读-改-写（冲突重放 + 串行）/ 模板预览（用例同 iOS `WebhooksProtocolTests`）。 */
class WebhooksTest {
    // ── 事件名 ──

    @Test
    fun eventWireNamesMatchEngineKindsInCanonicalOrder() {
        assertEquals(
            listOf(
                "task.created",
                "task.started",
                "task.completed",
                "task.failed",
                "task.paused",
                "queue.started",
                "queue.drained",
            ),
            WebhookEvent.entries.map { it.wire },
        )
        assertEquals(listOf("task.completed", "task.failed"), WebhookEvent.defaultWires)
    }

    @Test
    fun firstEventFollowsCanonicalOrderNotSubscriptionOrder() {
        val e = WebhookEndpoint(id = "a", events = listOf("task.failed", "task.started"))
        assertEquals("task.started", e.firstEventWire)
        assertNull(WebhookEndpoint(id = "a", events = emptyList()).firstEventWire)
    }

    // ── 宽松解析 ──

    @Test
    fun parseListSkipsNonObjectsAndFallsBackToDefaultsForWrongTypes() {
        val raw = """
            [1, "x", null, [], {"id":"a","name":"A","url":"https://h/x","events":["task.completed", 3, null],
              "headers":{"X-A":"1","X-B":2},"enabled":"yes","allowHttp":1,"useProxy":true,"queueId":7},
             {"id":"b"}]
        """.trimIndent()
        val list = WebhookEndpoint.parseList(raw)
        assertEquals(listOf("a", "b"), list.map { it.id })
        val a = list[0]
        assertEquals(listOf("task.completed"), a.events)
        assertEquals(mapOf("X-A" to "1"), a.headers)
        // `enabled` 类型不符 → serde `default_true`；其余布尔回退 false；字符串字段回退空串。
        assertTrue(a.enabled && !a.allowHttp && a.useProxy && a.queueId.isEmpty())
        val b = list[1]
        assertTrue(b.enabled && b.events.isEmpty() && b.name.isEmpty() && b.headers.isEmpty())
    }

    @Test
    fun parseListToleratesEmptyAndInvalidConfig() {
        assertTrue(WebhookEndpoint.parseList(null).isEmpty())
        assertTrue(WebhookEndpoint.parseList("").isEmpty())
        assertTrue(WebhookEndpoint.parseList("  \n").isEmpty())
        assertTrue(WebhookEndpoint.parseList("{not json").isEmpty())
        assertTrue(WebhookEndpoint.parseList("""{"id":"a"}""").isEmpty())
    }

    @Test
    fun encodeListRoundTripsWireShape() {
        val endpoint = WebhookEndpoint(
            id = "wh_1", name = "N", preset = "ntfy", url = "https://ntfy.sh/t", enabled = false,
            events = listOf("task.failed"), queueId = "main", headers = mapOf("A" to "b"), bodyTemplate = "{event}",
            signSecret = "whsec_x", allowHttp = true, useProxy = true,
        )
        val text = WebhookEndpoint.encodeList(listOf(endpoint))
        val keys = Json.parse(text)[0].objOrNull!!.keys
        assertEquals(
            setOf(
                "id", "name", "preset", "url", "enabled", "events", "queueId", "headers",
                "bodyTemplate", "signSecret", "allowHttp", "useProxy",
            ),
            keys,
        )
        assertEquals(listOf(endpoint), WebhookEndpoint.parseList(text))
    }

    @Test
    fun deliveriesResponseDecodesWithVariablesAndTolerantFields() {
        val json = """
            {"deliveries":[{"deliveryId":"d1","timestampMs":1700000000000,"event":"task.completed","endpointId":"e","endpointName":"E",
              "url":"https://x","requestHeaders":"A: b","requestBody":"{}","statusCode":0,"responseBody":"","latencyMs":12,"attempts":3,"success":false,"error":"timeout"},
              {"event":"no-id"}],
             "presets":[{"id":"ntfy","label":"ntfy","urlPlaceholder":"https://ntfy.sh/topic","defaultTemplate":"{event}","contentType":"text/plain"}],
             "variables":["{event}","{task.fileName}"]}
        """.trimIndent()
        val response = WebhookDeliveriesResponse.fromJson(Json.parse(json))
        assertEquals(1, response.deliveries.size)
        assertEquals("timeout", response.deliveries[0].statusSummary)
        assertEquals("text/plain", response.presets[0].contentType)
        assertEquals(listOf("{event}", "{task.fileName}"), response.variables)
        assertEquals("200 · 40ms", WebhookDelivery("d", 1, "e", "x", "n", statusCode = 200, latencyMs = 40, success = true).statusSummary)
        assertEquals("HTTP 502", WebhookDelivery("d", 1, "e", "x", "n", statusCode = 502, success = false).statusSummary)
    }

    @Test
    fun latestDeliveryPicksNewestTimestampForTheEndpoint() {
        val list = listOf(
            WebhookDelivery("1", 10, "e", "a", "A"),
            WebhookDelivery("2", 30, "e", "a", "A"),
            WebhookDelivery("3", 50, "e", "b", "B"),
        )
        assertEquals("2", latestWebhookDelivery(list, "a")?.deliveryId)
        assertNull(latestWebhookDelivery(list, "z"))
    }

    // ── 读-改-写 ──

    /** 模拟主机：真实的 revision 校验；`cache` 可被固定成过期值（store 发布滞后）。 */
    private class FakePort : DaemonConfigPort {
        var server = DaemonConfigSnapshot(5, emptyMap())
        var cache: DaemonConfigSnapshot? = null
        var forcedConflicts = 0
        var concurrent: List<WebhookEndpoint> = emptyList()
        val patches = mutableListOf<Pair<Long, List<WebhookEndpoint>>>()
        var fetches = 0
        var failure: HostException? = null

        override val cachedConfig: DaemonConfigSnapshot? get() = cache

        override suspend fun fetchConfig(): DaemonConfigSnapshot {
            fetches += 1
            return server
        }

        override suspend fun patchConfig(expectedRevision: Long, values: Map<String, String>) {
            yield()
            val list = WebhookEndpoint.parseList(values[WebhookEndpoint.CONFIG_KEY])
            patches += expectedRevision to list
            failure?.let { throw it }
            if (forcedConflicts > 0) {
                forcedConflicts -= 1
                val current = WebhookEndpoint.parseList(server.values[WebhookEndpoint.CONFIG_KEY]) + concurrent
                concurrent = emptyList()
                server = DaemonConfigSnapshot(server.revision + 1, mapOf(WebhookEndpoint.CONFIG_KEY to WebhookEndpoint.encodeList(current)))
                throw HostException(HostErrorCode.Conflict)
            }
            if (expectedRevision != server.revision) throw HostException(HostErrorCode.Conflict)
            server = DaemonConfigSnapshot(server.revision + 1, values)
        }

        fun seed(list: List<WebhookEndpoint>) {
            server = DaemonConfigSnapshot(server.revision, mapOf(WebhookEndpoint.CONFIG_KEY to WebhookEndpoint.encodeList(list)))
            cache = server
        }

        val stored: List<WebhookEndpoint> get() = WebhookEndpoint.parseList(server.values[WebhookEndpoint.CONFIG_KEY])
    }

    private fun endpoint(id: String) = WebhookEndpoint(id = id, name = id, url = "https://h/$id")

    @Test
    fun conflictReReadsLatestConfigAndReplaysTheMutationOnTopOfIt() = runBlocking {
        val port = FakePort().apply { seed(listOf(endpoint("a"))) }
        port.forcedConflicts = 1
        port.concurrent = listOf(endpoint("other-client")) // 冲突发生时别处刚加的端点
        val writer = WebhookEndpointWriter(port)

        writer.upsert(endpoint("c"))

        // 第一次基于缓存（rev 5），冲突后重读（rev 6，含 other-client）再重放：不得覆盖别处的修改。
        assertEquals(2, port.patches.size)
        assertEquals(5L, port.patches[0].first)
        assertEquals(listOf("a", "c"), port.patches[0].second.map { it.id })
        assertEquals(6L, port.patches[1].first)
        assertEquals(listOf("a", "other-client", "c"), port.patches[1].second.map { it.id })
        assertEquals(1, port.fetches)
        assertEquals(listOf("a", "other-client", "c"), port.stored.map { it.id })
    }

    @Test
    fun conflictRetriesAreCappedAtThree() = runBlocking {
        val port = FakePort().apply { seed(listOf(endpoint("a"))) }
        port.forcedConflicts = 100
        val writer = WebhookEndpointWriter(port)
        val e = runCatching { writer.remove("a") }.exceptionOrNull() as HostException
        assertEquals(HostErrorCode.Conflict, e.code)
        // 首次 + 3 次重放 = 4 次 patch，3 次重读。
        assertEquals(1 + WebhookEndpointWriter.MAX_CONFLICT_RETRIES, port.patches.size)
        assertEquals(WebhookEndpointWriter.MAX_CONFLICT_RETRIES, port.fetches)
    }

    @Test
    fun nonConflictErrorsPropagateWithoutRetry() = runBlocking {
        val port = FakePort().apply { seed(listOf(endpoint("a"))) }
        port.failure = HostException(HostErrorCode.InvalidArgument, message = "bad")
        val writer = WebhookEndpointWriter(port)
        val e = runCatching { writer.upsert(endpoint("b")) }.exceptionOrNull() as HostException
        assertEquals(HostErrorCode.InvalidArgument, e.code)
        assertEquals("bad", e.message)
        assertTrue(port.patches.size == 1 && port.fetches == 0)
    }

    @Test
    fun missingCacheFetchesBeforeFirstWriteAndNoOpMutationsSkipThePatch() = runBlocking {
        val port = FakePort().apply { seed(listOf(endpoint("a"))) }
        port.cache = null
        val writer = WebhookEndpointWriter(port)
        writer.setEnabled("missing", false) // 不存在 → 不写入
        assertTrue(port.patches.isEmpty() && port.fetches == 1)
        writer.setEnabled("a", false)
        assertFalse(port.stored.first().enabled)
    }

    @Test
    fun concurrentWritesAreSerializedEvenWithAStaleSnapshot() = runBlocking {
        val port = FakePort().apply { seed(emptyList()) }
        val writer = WebhookEndpointWriter(port) // cache 永远停在 rev 5（发布滞后）
        listOf("one", "two", "three").map { id -> async { writer.upsert(endpoint(id)) } }.awaitAll()
        assertEquals(setOf("one", "two", "three"), port.stored.map { it.id }.toSet())
        // 串行：每次写入都基于上一次的结果，没有基于过期数组回写。
        assertEquals(3, port.stored.size)
    }

    @Test
    fun upsertReplacesByIdAndRemoveDeletes() = runBlocking {
        val port = FakePort().apply { seed(listOf(endpoint("a"), endpoint("b"))) }
        val writer = WebhookEndpointWriter(port)
        writer.upsert(endpoint("a").copy(name = "renamed"))
        assertEquals(listOf("renamed", "b"), port.stored.map { it.name })
        port.seed(port.stored)
        writer.remove("b")
        assertEquals(listOf("a"), port.stored.map { it.id })
    }

    // ── 模板 ──

    @Test
    fun previewSubstitutesKnownPlaceholdersAndKeepsUnknownSegments() {
        assertEquals(
            """{"text":"ubuntu-24.04.2-desktop-amd64.iso · 6.0 GB"}""",
            WebhookTemplate.renderPreview("""{"text":"{task.fileName} · {task.totalBytesHuman}"}""", formEscape = false),
        )
        assertEquals("{unknown} {{ {", WebhookTemplate.renderPreview("{unknown} {{ {", formEscape = false))
        assertEquals("q=ubuntu-24.04.2-desktop-amd64.iso+%C2%B7+6.0+GB", WebhookTemplate.renderPreview("q={event.summary}", formEscape = true))
        assertEquals("\"Download completed\"", WebhookTemplate.renderPreview("\"{event.title}\"", formEscape = false))
        assertEquals("a+b-_.!~*'()", WebhookTemplate.formEncode("a b-_.!~*'()"))
        assertEquals("%2F%3F%26%3D", WebhookTemplate.formEncode("/?&="))
    }

    @Test
    fun urlValidationRequiresHttpsUnlessPlaintextAllowed() {
        assertNull(WebhookTemplate.urlError("", allowHttp = false))
        assertNull(WebhookTemplate.urlError("https://ntfy.sh/topic", allowHttp = false))
        assertEquals(WebhookTemplate.UrlError.WarnHttp, WebhookTemplate.urlError("http://192.168.1.2:8123/api", allowHttp = false))
        assertNull(WebhookTemplate.urlError("http://192.168.1.2:8123/api", allowHttp = true))
        assertEquals(WebhookTemplate.UrlError.Invalid, WebhookTemplate.urlError("ftp://host", allowHttp = true))
        assertEquals(WebhookTemplate.UrlError.Invalid, WebhookTemplate.urlError("https:///path", allowHttp = true))
        assertEquals(WebhookTemplate.UrlError.Invalid, WebhookTemplate.urlError("not a url", allowHttp = true))
    }

    @Test
    fun generatedSecretIsWhsecPlus32LowerHexAndDiffersEachTime() {
        val secret = WebhookTemplate.generateSecret()
        assertTrue(secret.length == 38 && secret.startsWith("whsec_"))
        assertTrue(secret.drop(6).all { it in '0'..'9' || it in 'a'..'f' })
        assertNotEquals(secret, WebhookTemplate.generateSecret())
    }

    @Test
    fun requestPreviewBuildsEnvelopeOrPresetBody() {
        val envelope = WebhookTemplate.previewRequest("", null, signEnabled = true, template = "", preset = null)
        assertTrue(envelope.startsWith("POST \nContent-Type: application/json\nX-FluxDown-Event: task.completed"))
        assertTrue(envelope.contains("X-FluxDown-Signature: t=1789647128,v1=9c41f2…"))
        assertTrue(envelope.contains("\"schemaVersion\": 1"))
        // 信封必须是合法 JSON。
        val body = envelope.split("─".repeat(28)).last()
        assertTrue(Json.parseOrNull(body) != null)

        val preset = WebhookPreset("slack", "Slack", "https://hooks.slack.com/x", """{"text":"{task.fileName}"}""", "application/json")
        val text = WebhookTemplate.previewRequest(" https://h/x ", "task.failed", signEnabled = false, template = "", preset = preset)
        assertTrue(text.contains("POST https://h/x\n"))
        assertTrue(text.contains("X-FluxDown-Event: task.failed"))
        assertFalse(text.contains("X-FluxDown-Signature"))
        assertTrue(text.endsWith("{\n  \"text\": \"ubuntu-24.04.2-desktop-amd64.iso\"\n}"))
        val form = WebhookPreset("bark", "Bark", defaultTemplate = "title={event.title}", contentType = "application/x-www-form-urlencoded")
        assertEquals("title=Download+completed", WebhookTemplate.previewBody("", form))
    }

    @Test
    fun prettyJsonKeepsKeyOrderAndEmptyContainers() {
        assertEquals("{\n  \"b\": 1,\n  \"a\": [],\n  \"c\": {}\n}", WebhookTemplate.prettyJson("""{"b":1,"a":[],"c":{}}"""))
        assertNull(WebhookTemplate.prettyJson("not json"))
    }

    // ── Gateway ──

    @Test
    fun gatewayPatchOmitsUnsetFieldsAndFeaturePatchTouchesOnlyItself() {
        assertEquals("""{"corsEnabled":true}""", GatewayFeature.Cors.patch(true).toJson().toJson())
        assertEquals("""{"port":18000}""", GatewayPatchParams(port = 18000).toJson().toJson())
        // 空串 = 清除令牌，必须保留（不是省略）。
        assertEquals("""{"userToken":""}""", GatewayPatchParams(userToken = "").toJson().toJson())
        assertEquals("""{"regenerateUserToken":true}""", GatewayPatchParams(regenerateUserToken = true).toJson().toJson())
        val status = GatewayStatusDto.fromJson(Json.parse("""{"mcpEnabled":true}"""))
        assertTrue(GatewayFeature.Mcp.isOn(status) && !GatewayFeature.Api.isOn(status))
        assertEquals(GatewayStatusDto.DEFAULT_PORT, status.port)
        assertTrue(status.portEditable)
    }

    @Test
    fun gatewayAddressNormalizesSchemeAndDropsPath() {
        assertEquals("http://nas.local:17800", GatewayAddress.base("ws://nas.local:17800/rpc?x=1"))
        assertEquals("https://h.example", GatewayAddress.base(" wss://h.example/rpc "))
        assertNull(GatewayAddress.base("ftp://h"))
        assertNull(GatewayAddress.base("not a url"))
        assertEquals(17800, GatewayAddress.port("http://h:17800"))
        assertNull(GatewayAddress.port("http://h"))
    }
}
