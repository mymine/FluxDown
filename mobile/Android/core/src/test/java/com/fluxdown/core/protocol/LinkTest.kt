package com.fluxdown.core.protocol

import com.fluxdown.core.model.LinkDevice
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** 与 iOS `LinkProtocolTests` 同一组直连配对规则用例。 */
class LinkTest {
    @Test
    fun lanStateFollowsGateway() {
        // 分区缺失（server 模式 / 无网关分区）：按可达处理。
        assertEquals(LinkLan.Active, LinkLan.of(null))
        assertEquals(LinkLan.Disabled, LinkLan.of(GatewayStatusDto()))
        assertEquals(LinkLan.RestartRequired, LinkLan.of(GatewayStatusDto(lanEnabled = true)))
        assertEquals(LinkLan.Active, LinkLan.of(GatewayStatusDto(lanEnabled = true, lanActive = true)))
        // 实际在监听（如 server 模式固定监听地址）时即使持久化配置为关也算可达。
        assertEquals(LinkLan.Active, LinkLan.of(GatewayStatusDto(lanEnabled = false, lanActive = true)))
        assertTrue(GatewayStatusDto.fromJson(Json.parse("""{"lanActive":true}""")).lanActive)
    }

    @Test
    fun inputValidation() {
        assertEquals("123456", LinkRules.normalizePairingCode("12a3-45 678"))
        assertTrue(LinkRules.isPairingCodeComplete("123456"))
        assertFalse(LinkRules.isPairingCodeComplete("12345"))
        assertEquals(
            LinkRules.Validation.Failed(LinkRules.InputError.AddressMissing),
            LinkRules.validate("  ", "123456"),
        )
        assertEquals(
            LinkRules.Validation.Failed(LinkRules.InputError.AddressMissing),
            LinkRules.validate("a b", "123456"),
        )
        assertEquals(
            LinkRules.Validation.Failed(LinkRules.InputError.CodeIncomplete),
            LinkRules.validate("10.0.0.2", "12 34"),
        )
        assertEquals(
            LinkRules.Validation.Ok(LinkPairBeginParams("10.0.0.2:17800", "123456")),
            LinkRules.validate(" 10.0.0.2:17800 ", "123-456"),
        )
    }

    @Test
    fun discoveredEntriesDedupeAndMarkPaired() {
        val peers = listOf(
            LinkDiscoveredPeer("fp-a", "zeta", "linux", "10.0.0.2", 17800, null, "mdns"),
            LinkDiscoveredPeer("fp-a", "zeta dup", "linux", "10.0.0.2", 17800, null, "mdns"),
            LinkDiscoveredPeer("fp-b", " ", "macos", "fe80::1", 17800, null, "mdns"),
            LinkDiscoveredPeer(null, "Alpha", null, "10.0.0.9", 9000, null, "manual"),
            LinkDiscoveredPeer("fp-r", "Revoked", null, "10.0.0.7", 17800, null, "mdns"),
        )
        val paired = listOf(
            LinkDevice("fp-a", "A", null, online = true),
            LinkDevice("fp-r", "R", null, online = false, revoked = true),
        )
        val entries = LinkRules.discoveredEntries(peers, paired)
        // 空名用主机；按名称不区分大小写排序；同地址只留一条。
        assertEquals(listOf("[fe80::1]:17800", "10.0.0.9:9000", "10.0.0.7:17800", "10.0.0.2:17800").sorted(), entries.map { it.address }.sorted())
        assertEquals(listOf("Alpha", "fe80::1", "Revoked", "zeta"), entries.map { it.name })
        // 已配对不可再选；对方已解除的记录不算已配对（可重新配对）。
        assertEquals(listOf(false, false, false, true), entries.map { it.paired })
    }

    @Test
    fun countdownAndGrouping() {
        assertEquals(0L, LinkRules.secondsUntil(1_000, 2_000))
        assertEquals(1L, LinkRules.secondsUntil(2_001, 2_000))
        assertEquals(2L, LinkRules.secondsUntil(3_000, 1_001))
        assertEquals("123 456", LinkRules.groupSas("123456"))
        assertEquals("12", LinkRules.groupSas("12"))
        assertEquals(1_700_000_000_000L, LinkRules.unixMillis(1_700_000_000))
        assertEquals(1_700_000_000_000L, LinkRules.unixMillis(1_700_000_000_000))
        assertNull(LinkRules.unixMillis(0))
        val a = LinkPairingRequest("a", "A", "fa", null, "111111", 300)
        val b = LinkPairingRequest("b", "B", "fb", null, "222222", 100)
        val c = LinkPairingRequest("c", "C", "fc", null, "333333", 100)
        assertEquals(listOf("b", "c", "a"), LinkRules.queue(listOf(a, b, c)).map { it.sessionId })
    }

    @Test
    fun renameValidation() {
        assertEquals(LinkRules.RenameCheck.Clear, LinkRules.checkRename("   "))
        assertEquals(LinkRules.RenameCheck.Name("NAS"), LinkRules.checkRename("  NAS "))
        assertEquals(LinkRules.RenameCheck.Name("x".repeat(64)), LinkRules.checkRename("x".repeat(64)))
        assertEquals(LinkRules.RenameCheck.Invalid, LinkRules.checkRename("x".repeat(65)))
        assertEquals(LinkRules.RenameCheck.Invalid, LinkRules.checkRename("a\u0007b"))
        assertEquals("""{"fingerprint":"fp"}""", LinkRenameParams("fp", null).toJson().toJson())
        assertEquals("""{"fingerprint":"fp","name":"N"}""", LinkRenameParams("fp", "N").toJson().toJson())
    }

    @Test
    fun wireDecoding() {
        val peers = LinkDiscoveredPeer.listFromJson(
            Json.parse("""[{"fingerprint":"f","name":"n","host":"h","port":1,"source":"manual"},{"name":"bad"}]"""),
        )
        assertEquals(listOf(LinkDiscoveredPeer("f", "n", null, "h", 1, null, "manual")), peers)
        val requests = LinkPairingRequest.listFromJson(
            Json.parse("""[{"sessionId":"s","peerName":"P","peerFingerprint":"fp","sas":"123456","expiresAtUnixMs":42}]"""),
        )
        assertEquals(listOf(LinkPairingRequest("s", "P", "fp", null, "123456", 42)), requests)
        val finish = LinkPairFinish.fromJson(Json.parse("""{"paired":true,"device":{"fingerprint":"f","name":"Mac"}}"""))
        assertEquals(LinkPairFinish(true, "Mac"), finish)
        assertEquals(LinkPairFinish(false, null), LinkPairFinish.fromJson(Json.parse("""{"paired":false}""")))
    }

    @Test
    fun controlFieldsOnTheWire() {
        // requestControl 只在为 true 时出现：普通设备配对的 wire 与旧版一致。
        assertEquals("""{"address":"a","code":"123456"}""", LinkPairBeginParams("a", "123456").toJson().toJson())
        assertEquals(
            """{"address":"a","code":"123456","requestControl":true}""",
            LinkPairBeginParams("a", "123456", requestControl = true).toJson().toJson(),
        )
        assertEquals(
            LinkRules.Validation.Ok(LinkPairBeginParams("h", "123456", requestControl = true)),
            LinkRules.validate("h", "123456", requestControl = true),
        )
        val requests = LinkPairingRequest.listFromJson(
            Json.parse("""[{"sessionId":"s","peerName":"P","peerFingerprint":"fp","sas":"1","expiresAtUnixMs":42,"requestsControl":true}]"""),
        )
        assertTrue(requests.single().requestsControl)
        assertFalse(LinkPairingRequest("s", "P", "fp", null, "1", 1).requestsControl)
        assertEquals("tok", LinkControlCredential.fromJson(Json.parse("""{"token":"tok"}""")).token)
        assertTrue(LinkRules.isControlUnsupported("pairingVersionMismatch"))
        assertTrue(LinkRules.isControlUnsupported("peerOutdated"))
        assertFalse(LinkRules.isControlUnsupported("pairingRejected"))
        assertFalse(LinkRules.isControlUnsupported(null))
    }

    @Test
    fun deadlinesUseRelativeTtl() {
        // ttlMs 从「收到 DTO 的本机时刻」起算，与主机时钟无关；缺失 / 0（旧主机）才回退 expiresAtUnixMs。
        assertEquals(10_000L + 60_000L, LinkRules.localDeadline(60_000, wireExpiresAtUnixMs = 5, nowMs = 10_000))
        assertEquals(5L, LinkRules.localDeadline(null, wireExpiresAtUnixMs = 5, nowMs = 10_000))
        assertEquals(5L, LinkRules.localDeadline(0, wireExpiresAtUnixMs = 5, nowMs = 10_000))
        val requests = LinkPairingRequest.listFromJson(
            Json.parse("""[{"sessionId":"s","peerName":"P","peerFingerprint":"fp","sas":"1","expiresAtUnixMs":1,"ttlMs":30000}]"""),
            nowMs = 1_000,
        )
        assertEquals(31_000L, requests.single().expiresAtUnixMs)
        val code = LinkPairingCode.fromJson(
            Json.parse("""{"code":"123456","expiresAtUnixMs":1,"ttlMs":120000,"addresses":[],"fingerprint":"f","deviceName":"d"}"""),
            nowMs = 2_000,
        )
        assertEquals(122_000L, code.expiresAtUnixMs)
        assertEquals(
            120L,
            LinkRules.secondsUntil(code.expiresAtUnixMs, nowMs = 2_000),
        )
    }

    @Test
    fun hostEntriesPutControllableFirst() {
        val peers = listOf(
            LinkDiscoveredPeer("fp-a", "Alpha", null, "10.0.0.2", 17800, null, "mdns"),
            LinkDiscoveredPeer("fp-b", "Bravo", null, "10.0.0.3", 17800, null, "mdns"),
            LinkDiscoveredPeer("fp-c", "Charlie", null, "10.0.0.4", 17800, null, "mdns"),
            LinkDiscoveredPeer("fp-d", "Delta", null, "10.0.0.5", 17800, null, "mdns"),
        )
        val paired = listOf(
            // 已配对但只能被它控制（本机不可控制它）：不跳过配对。
            LinkDevice("fp-a", "A", null, online = true, grantsControl = true),
            LinkDevice("fp-c", "C", null, online = true, controllable = true),
            // 对方已解除：即使曾可控制也不算。
            LinkDevice("fp-d", "D", null, online = true, controllable = true, revoked = true),
        )
        val entries = LinkRules.hostEntries(peers, paired)
        assertEquals(listOf("Charlie", "Alpha", "Bravo", "Delta"), entries.map { it.name })
        assertEquals(listOf(true, false, false, false), entries.map { it.controllable })
        assertEquals("fp-c", entries.first().fingerprint)
        assertEquals("10.0.0.4:17800", entries.first().address)
    }
}
