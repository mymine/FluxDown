package com.fluxdown.core.protocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NetworkTest {
    private fun form(host: Map<String, String>) = SettingsForm(host)

    // ── 模式 ──

    @Test
    fun unknownOrMissingModeFallsBackToNone() {
        assertEquals(NetworkProxyMode.None, NetworkProxyMode.of(null))
        assertEquals(NetworkProxyMode.None, NetworkProxyMode.of("bogus"))
        assertEquals(NetworkProxyMode.Auto, NetworkProxyMode.of("auto"))
        assertEquals(listOf("none", "system", "manual", "auto"), NetworkProxyMode.entries.map { it.wire })
    }

    // ── 端口 ──

    @Test
    fun portKeepsDigitsOnlyAndClampsIntoRange() {
        assertEquals("8080", NetworkPortInput.digits(":80a80"))
        assertEquals("", NetworkPortInput.digits("１２３")) // 全角数字不算

        fun check(text: String, wire: String, adjusted: Int?) {
            val result = NetworkPortInput.commit(text)
            assertEquals(text, wire, result.wire)
            assertEquals(text, adjusted, result.adjusted)
        }
        check("1080", "1080", null)
        check("0080", "80", null)
        check("0", "1", 1)
        check("70000", "65535", 65535)
        check("99999999999999999999999", "65535", 65535)
        check("abc", "", null)
        check("", "", null)
    }

    // ── 测试请求 ──

    @Test
    fun manualRequestUsesTheFiveProxyKeys() {
        val f = form(
            mapOf(
                "proxy_type" to "socks5", "proxy_host" to "10.0.0.2", "proxy_port" to "1080",
                "proxy_username" to "u", "proxy_password" to "p",
            ),
        )
        assertEquals(
            ProxyTestRequest("socks5", "10.0.0.2", "1080", "u", "p"),
            NetworkProxyTest.request(NetworkProxyMode.Manual, f, null),
        )
    }

    @Test
    fun manualRequestDefaultsTypeToHttp() {
        val f = form(mapOf("proxy_host" to "h", "proxy_port" to "1"))
        assertEquals("http", NetworkProxyTest.request(NetworkProxyMode.Manual, f, null)?.proxyType)
    }

    @Test
    fun systemRequestNeedsADetectedProxyAndSendsNoCredentials() {
        val f = form(mapOf("proxy_username" to "u", "proxy_password" to "p"))
        val detected = SystemProxyDto(true, "http", "192.168.1.2", 7890, "localhost")
        assertEquals(
            ProxyTestRequest("http", "192.168.1.2", "7890"),
            NetworkProxyTest.request(NetworkProxyMode.System, f, detected),
        )
        assertNull(NetworkProxyTest.request(NetworkProxyMode.System, f, SystemProxyDto()))
        assertNull(NetworkProxyTest.request(NetworkProxyMode.System, f, null))
    }

    @Test
    fun noneAndAutoHaveNothingToTest() {
        val f = form(mapOf("proxy_host" to "h", "proxy_port" to "1"))
        assertNull(NetworkProxyTest.request(NetworkProxyMode.None, f, null))
        assertNull(NetworkProxyTest.request(NetworkProxyMode.Auto, f, null))
    }

    @Test
    fun tlsHandshakeFailureGetsTheActionableHint() {
        assertEquals(
            "hint",
            NetworkProxyTest.failureDetail("proxy: peer Did Not Accept A TLS Handshake", "fallback", "hint"),
        )
        assertEquals("connect refused", NetworkProxyTest.failureDetail(" connect refused \n", "fallback", "hint"))
        assertEquals("fallback", NetworkProxyTest.failureDetail("  ", "fallback", "hint"))
    }

    // ── 行目录 ──

    @Test
    fun rowVisibilityFollowsModeAndConfigLoad() {
        val siteAuth = listOf(NetworkRow.SiteAuth, NetworkRow.SiteAuthAdd, NetworkRow.SiteAuthClear)
        assertEquals(siteAuth, NetworkRow.visible(NetworkProxyMode.None, false))

        val none = NetworkRow.visible(NetworkProxyMode.None, true)
        assertEquals(
            listOf(NetworkRow.Mode, NetworkRow.Rules, NetworkRow.RulesAdd, NetworkRow.RulesTest) + siteAuth,
            none,
        )
        assertEquals(none, NetworkRow.visible(NetworkProxyMode.System, true))
        assertEquals(none, NetworkRow.visible(NetworkProxyMode.Auto, true))

        val manual = NetworkRow.visible(NetworkProxyMode.Manual, true)
        assertTrue(NetworkRow.Host in manual && NetworkRow.Password in manual && NetworkRow.Test in manual)
    }

    @Test
    fun configRowsMapToProxyCatalogKeysThatAreNotSynced() {
        for (row in NetworkRow.entries) {
            val key = row.configKey ?: continue
            assertEquals(key, SettingField.Store.Daemon, SettingsCatalog.field(key)?.store)
            assertFalse(key, SettingsCatalog.isSynced(key))
        }
        assertEquals("proxy_mode", NetworkRow.Mode.configKey)
        assertNull(NetworkRow.Test.configKey)
    }

    // ── 站点凭据 ──

    @Test
    fun searchBoxAppearsFromSixEntries() {
        assertFalse(SiteAuthFilter.showsSearch(5))
        assertTrue(SiteAuthFilter.showsSearch(6))
    }

    @Test
    fun filterMatchesSiteOrUserCaseInsensitively() {
        val entries = listOf(SiteAuthEntryDto("Example.com", "alice"), SiteAuthEntryDto("nas.local:8080", "Bob"))
        assertEquals(entries, SiteAuthFilter.filter(entries, ""))
        assertEquals(entries, SiteAuthFilter.filter(entries, "  "))
        assertEquals(listOf(entries[0]), SiteAuthFilter.filter(entries, "EXAMPLE"))
        assertEquals(listOf(entries[1]), SiteAuthFilter.filter(entries, "bob"))
        assertEquals(listOf(entries[1]), SiteAuthFilter.filter(entries, "8080"))
        assertTrue(SiteAuthFilter.filter(entries, "zzz").isEmpty())
    }

    @Test
    fun filterIgnoresDiacriticsAndWidth() {
        val entries = listOf(SiteAuthEntryDto("café.example", "renée"))
        assertEquals(entries, SiteAuthFilter.filter(entries, "CAFE"))
        assertEquals(entries, SiteAuthFilter.filter(entries, "ｒｅｎｅｅ"))
    }

    @Test
    fun formRequiresSiteAndUser() {
        assertTrue(SiteAuthFilter.canSave("example.com", "alice"))
        assertFalse(SiteAuthFilter.canSave("  ", "alice"))
        assertFalse(SiteAuthFilter.canSave("example.com", " "))
    }
}
