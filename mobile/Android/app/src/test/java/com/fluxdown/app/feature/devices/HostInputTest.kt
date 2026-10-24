package com.fluxdown.app.feature.devices

import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.HostRef
import com.fluxdown.core.protocol.LinkDiscoveredPeer
import com.fluxdown.core.protocol.LinkRules
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HostInputTest {
    private fun endpoint(input: String): String? = (parseHostEndpoint(input) as? EndpointParse.Ok)?.endpoint

    @Test
    fun discoveredAddressesMapToTheRpcEndpoint() {
        // 发现列表里的地址（host:port，IPv6 带方括号）= 配对后保存的 `/rpc` 端点（由桥接层补 ws:// 与 /rpc）。
        val ipv4 = LinkDiscoveredPeer("fp", "NAS", null, "192.168.1.20", 17800, null, "mdns")
        val ipv6 = LinkDiscoveredPeer("fp", "NAS", null, "fe80::1", 9000, null, "mdns")
        assertEquals("http://192.168.1.20:17800", endpoint(LinkRules.address(ipv4)))
        assertEquals("http://[fe80::1]:9000", endpoint(LinkRules.address(ipv6)))
    }

    @Test
    fun manualAddressesNormalise() {
        assertEquals("http://192.168.1.20:17800", endpoint("192.168.1.20"))
        assertEquals("http://nas.lan:8080", endpoint(" nas.lan:8080 "))
        assertEquals("https://nas.example.com:443", endpoint("https://nas.example.com"))
        assertEquals("http://10.0.0.2:17800", endpoint("http://10.0.0.2:17800/rpc?x=1"))
        assertEquals(null, endpoint(""))
    }

    @Test
    fun endpointsAlwaysCarryAnExplicitPort() {
        // Rust `normalize_endpoint` 对不带端口的裸主机默认 80 / 443，而配对默认 17800：保存 / 连接用的端点必须显式带端口。
        val explicit = Regex("""^https?://(\[[0-9a-fA-F:.]+]|[^:/\[\]]+):\d+$""")
        listOf(
            "192.168.1.20", "nas.lan", "[fe80::1]", "http://nas.lan", "https://nas.example.com", "ws://10.0.0.2", "wss://nas.example.com/rpc",
            "10.0.0.2:8080", "[::1]:9000", "http://10.0.0.2:17800/rpc",
        ).forEach { input -> assertTrue("$input -> ${endpoint(input)}", endpoint(input)?.let(explicit::matches) == true) }
        assertEquals("http://nas.lan:17800", endpoint("nas.lan"))
        assertEquals("http://[fe80::1]:17800", endpoint("[fe80::1]"))
        assertEquals("http://10.0.0.2:17800", endpoint("ws://10.0.0.2"))
        assertEquals("https://nas.example.com:443", endpoint("wss://nas.example.com/rpc"))
        // 重新配对预填的已保存端点再次解析结果不变。
        assertEquals("http://h:17800", endpoint("http://h:17800"))
    }

    @Test
    fun invalidManualAddressesAreRejected() {
        assertEquals(null, endpoint("10.0.0.2:99999"))
        assertEquals(null, endpoint("a b"))
    }

    @Test
    fun cleartextOnlyWarnsForNonLoopbackHttp() {
        assertTrue((parseHostEndpoint("192.168.1.2") as EndpointParse.Ok).cleartext)
        assertFalse((parseHostEndpoint("127.0.0.1:17800") as EndpointParse.Ok).cleartext)
        assertFalse((parseHostEndpoint("https://nas.example.com") as EndpointParse.Ok).cleartext)
    }

    @Test
    fun pairedHostsAreRepairedNotRekeyedWhenRejected() {
        val linked = HostRef.Remote("id", "PC", "http://h:17800", linkFingerprint = "f".repeat(64))
        val keyed = HostRef.Remote("id2", "NAS", "http://h2:17800")
        val unauthorized = HostException(HostErrorCode.Unauthorized, message = "401")
        val unavailable = HostException(HostErrorCode.Unavailable, message = "down")
        assertTrue(linked.needsRepair(unauthorized))
        assertFalse(linked.needsRepair(unavailable))
        assertFalse(keyed.needsRepair(unauthorized))
        assertFalse(HostRef.Local("phone").needsRepair(unauthorized))
        assertFalse(linked.needsRepair(null))
    }
}
