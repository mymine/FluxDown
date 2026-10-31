import FluxDomain
import Foundation
import Testing
@testable import FluxDown

@MainActor
struct SettingsNetworkTests {
    private func form(_ host: [String: String]) -> SettingsConfigForm {
        SettingsConfigForm(host: host)
    }

    // MARK: 模式

    @Test func unknownOrMissingModeFallsBackToNone() {
        #expect(NetworkProxyMode(wire: nil) == .none)
        #expect(NetworkProxyMode(wire: "bogus") == .none)
        #expect(NetworkProxyMode(wire: "auto") == .auto)
        #expect(NetworkProxyMode.allCases.map(\.rawValue) == ["none", "system", "manual", "auto"])
    }

    // MARK: 端口

    @Test func portKeepsDigitsOnlyAndClampsIntoRange() {
        #expect(NetworkPortInput.digits(":80a80") == "8080")
        #expect(NetworkPortInput.digits("１２３") == "") // 全角数字不算
        func check(_ text: String, _ wire: String, _ adjusted: Int?) {
            let result = NetworkPortInput.commit(text)
            #expect(result.wire == wire, "\(text)")
            #expect(result.adjusted == adjusted, "\(text)")
        }
        check("1080", "1080", nil)
        check("0", "1", 1)
        check("70000", "65535", 65535)
        check("99999999999999999999999", "65535", 65535)
        check("abc", "", nil)
        check("", "", nil)
    }

    // MARK: 测试请求

    @Test func manualRequestUsesTheFiveProxyKeys() {
        let f = form([
            "proxy_type": "socks5", "proxy_host": "10.0.0.2", "proxy_port": "1080",
            "proxy_username": "u", "proxy_password": "p",
        ])
        #expect(NetworkProxyTest.request(mode: .manual, form: f, system: nil)
            == ProxyTestRequest(proxyType: "socks5", host: "10.0.0.2", port: "1080", username: "u", password: "p"))
    }

    @Test func manualRequestDefaultsTypeToHttp() {
        let f = form(["proxy_host": "h", "proxy_port": "1"])
        #expect(NetworkProxyTest.request(mode: .manual, form: f, system: nil)?.proxyType == "http")
    }

    @Test func systemRequestNeedsADetectedProxyAndSendsNoCredentials() {
        let f = form(["proxy_username": "u", "proxy_password": "p"])
        let detected = SystemProxyDto(detected: true, proxyType: "http", host: "192.168.1.2", port: 7890, noList: "localhost")
        #expect(NetworkProxyTest.request(mode: .system, form: f, system: detected)
            == ProxyTestRequest(proxyType: "http", host: "192.168.1.2", port: "7890"))
        #expect(NetworkProxyTest.request(mode: .system, form: f, system: SystemProxyDto()) == nil)
        #expect(NetworkProxyTest.request(mode: .system, form: f, system: nil) == nil)
    }

    @Test func noneAndAutoHaveNothingToTest() {
        let f = form(["proxy_host": "h", "proxy_port": "1"])
        #expect(NetworkProxyTest.request(mode: .none, form: f, system: nil) == nil)
        #expect(NetworkProxyTest.request(mode: .auto, form: f, system: nil) == nil)
    }

    @Test func tlsHandshakeFailureGetsTheActionableHint() {
        let detail = NetworkProxyTest.failureDetail(
            message: "proxy: peer Did Not Accept A TLS Handshake", fallback: "fallback", tlsHint: "hint"
        )
        #expect(detail == "hint")
        #expect(NetworkProxyTest.failureDetail(message: " connect refused \n", fallback: "fallback", tlsHint: "hint") == "connect refused")
        #expect(NetworkProxyTest.failureDetail(message: "  ", fallback: "fallback", tlsHint: "hint") == "fallback")
    }

    // MARK: 行目录

    @Test func rowVisibilityFollowsModeAndConfigLoad() {
        let unloaded = NetworkRow.visible(mode: .none, isLoaded: false)
        #expect(unloaded == [.siteAuth, .siteAuthAdd, .siteAuthClear])

        let none = NetworkRow.visible(mode: .none, isLoaded: true)
        #expect(none == [.mode, .rules, .rulesAdd, .rulesTest, .siteAuth, .siteAuthAdd, .siteAuthClear])
        #expect(NetworkRow.visible(mode: .system, isLoaded: true) == none)
        #expect(NetworkRow.visible(mode: .auto, isLoaded: true) == none)

        let manual = NetworkRow.visible(mode: .manual, isLoaded: true)
        #expect(manual.contains(.host))
        #expect(manual.contains(.password))
        #expect(manual.contains(.test))
    }

    @Test func configRowsMapToProxyCatalogKeysThatAreNotSynced() throws {
        for row in NetworkRow.allCases {
            guard let key = row.configKey else { continue }
            #expect(SettingsCatalog.field(key)?.store == .daemon, "\(key)")
            #expect(!SettingsCatalog.isSynced(key), "\(key)")
        }
        #expect(NetworkRow.mode.item?.key == "proxy_mode")
        #expect(NetworkRow.test.item == nil)
    }

    @Test func searchEntriesOnlyIndexVisibleRows() {
        func ctx(_ host: [String: String]) -> SettingsSearchContext {
            SettingsSearchContext(form: form(host), isLocalHost: true, capabilities: [])
        }
        let manual = NetworkPage.searchEntries(ctx(["proxy_mode": "manual"]))
        #expect(manual.map(\.id).contains("network.host"))
        #expect(manual.map(\.id).contains("network.test"))
        #expect(manual.allSatisfy { $0.route == .network })

        let none = NetworkPage.searchEntries(ctx(["proxy_mode": "none"]))
        #expect(!none.map(\.id).contains("network.host"))
        #expect(none.map(\.id).contains("network.mode"))
        #expect(none.map(\.id).contains("network.siteAuth"))

        #expect(NetworkPage.readout(ctx(["proxy_mode": "auto"])) == L("proxyModeAuto"))
        #expect(NetworkPage.readout(ctx([:])) == nil)
    }

    // MARK: 站点凭据

    @Test func searchBoxAppearsFromSixEntries() {
        #expect(!SiteAuthFilter.showsSearch(count: 5))
        #expect(SiteAuthFilter.showsSearch(count: 6))
    }

    @Test func filterMatchesSiteOrUserCaseInsensitively() {
        let entries = [
            SiteAuthEntryDto(site: "Example.com", user: "alice"),
            SiteAuthEntryDto(site: "nas.local:8080", user: "Bob"),
        ]
        #expect(SiteAuthFilter.filter(entries, query: "") == entries)
        #expect(SiteAuthFilter.filter(entries, query: "  ") == entries)
        #expect(SiteAuthFilter.filter(entries, query: "EXAMPLE") == [entries[0]])
        #expect(SiteAuthFilter.filter(entries, query: "bob") == [entries[1]])
        #expect(SiteAuthFilter.filter(entries, query: "8080") == [entries[1]])
        #expect(SiteAuthFilter.filter(entries, query: "zzz").isEmpty)
    }

    @Test func formRequiresSiteAndUser() {
        #expect(SiteAuthFilter.canSave(site: "example.com", user: "alice"))
        #expect(!SiteAuthFilter.canSave(site: "  ", user: "alice"))
        #expect(!SiteAuthFilter.canSave(site: "example.com", user: " "))
    }
}
