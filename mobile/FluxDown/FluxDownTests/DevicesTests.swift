import FluxDomain
import Testing
@testable import FluxDown

@MainActor
struct DevicesTests {
    private func ok(_ endpoint: String, _ host: String, cleartext: Bool) -> EndpointParse {
        .ok(endpoint: endpoint, host: host, cleartext: cleartext)
    }

    // MARK: 地址

    @Test func bareHostGetsHttpAndDefaultPort() {
        #expect(HostInput.parseEndpoint("192.168.1.20") == ok("http://192.168.1.20:17800", "192.168.1.20", cleartext: true))
        #expect(HostInput.parseEndpoint("  nas.local  ") == ok("http://nas.local:17800", "nas.local", cleartext: true))
    }

    @Test func explicitPortIsKept() {
        #expect(HostInput.parseEndpoint("192.168.1.20:9000") == ok("http://192.168.1.20:9000", "192.168.1.20", cleartext: true))
        #expect(HostInput.parseEndpoint("host:65535") == ok("http://host:65535", "host", cleartext: true))
        #expect(HostInput.parseEndpoint("host:1") == ok("http://host:1", "host", cleartext: true))
    }

    @Test func httpsAndWssAreSecureWith443() {
        #expect(HostInput.parseEndpoint("https://nas.example.com") == ok("https://nas.example.com:443", "nas.example.com", cleartext: false))
        #expect(HostInput.parseEndpoint("WSS://nas.example.com:8443/rpc") == ok("https://nas.example.com:8443", "nas.example.com", cleartext: false))
    }

    @Test func httpAndWsAreCleartext() {
        #expect(HostInput.parseEndpoint("http://nas.example.com") == ok("http://nas.example.com:17800", "nas.example.com", cleartext: true))
        #expect(HostInput.parseEndpoint("ws://nas.example.com:80") == ok("http://nas.example.com:80", "nas.example.com", cleartext: true))
    }

    @Test func loopbackIsNotFlaggedCleartext() {
        #expect(HostInput.parseEndpoint("localhost") == ok("http://localhost:17800", "localhost", cleartext: false))
        #expect(HostInput.parseEndpoint("127.0.0.1:1234") == ok("http://127.0.0.1:1234", "127.0.0.1", cleartext: false))
        #expect(HostInput.parseEndpoint("http://[::1]:17800") == ok("http://[::1]:17800", "[::1]", cleartext: false))
    }

    @Test func pathQueryAndFragmentAreDropped() {
        #expect(HostInput.parseEndpoint("http://nas:17800/rpc?x=1#f") == ok("http://nas:17800", "nas", cleartext: true))
        #expect(HostInput.parseEndpoint("nas/anything") == ok("http://nas:17800", "nas", cleartext: true))
    }

    @Test func ipv6Literals() {
        #expect(HostInput.parseEndpoint("[fe80::1]") == ok("http://[fe80::1]:17800", "[fe80::1]", cleartext: true))
        #expect(HostInput.parseEndpoint("https://[2001:db8::1]:8443") == ok("https://[2001:db8::1]:8443", "[2001:db8::1]", cleartext: false))
        #expect(HostInput.parseEndpoint("[]") == .badAddress)
        #expect(HostInput.parseEndpoint("[::1]x") == .badAddress)
        #expect(HostInput.parseEndpoint("[::g]") == .badAddress)
    }

    @Test func emptyAndBadAddresses() {
        #expect(HostInput.parseEndpoint("") == .empty)
        #expect(HostInput.parseEndpoint("   ") == .empty)
        #expect(HostInput.parseEndpoint("ftp://nas") == .badAddress)
        #expect(HostInput.parseEndpoint("://nas") == .badAddress)
        #expect(HostInput.parseEndpoint("user@nas") == .badAddress)
        #expect(HostInput.parseEndpoint("a b") == .badAddress)
        #expect(HostInput.parseEndpoint("a:b:c") == .badAddress)
        #expect(HostInput.parseEndpoint("-nas") == .badAddress)
        #expect(HostInput.parseEndpoint(".nas") == .badAddress)
        #expect(HostInput.parseEndpoint("nas.") == .badAddress)
        #expect(HostInput.parseEndpoint("nä.local") == .badAddress)
        #expect(HostInput.parseEndpoint("http:///") == .badAddress)
        #expect(HostInput.parseEndpoint("nas:") == .badAddress)
        #expect(HostInput.parseEndpoint("nas:12a") == .badAddress)
    }

    @Test func portRange() {
        #expect(HostInput.parseEndpoint("nas:0") == .badPort)
        #expect(HostInput.parseEndpoint("nas:65536") == .badPort)
        #expect(HostInput.parseEndpoint("nas:99999999999999999999") == .badPort)
    }

    @Test func hostNameLengthLimit() {
        let long = String(repeating: "a", count: 254)
        #expect(HostInput.parseEndpoint(long) == .badAddress)
        let max = String(repeating: "a", count: 253)
        #expect(HostInput.parseEndpoint(max) == ok("http://\(max):17800", max, cleartext: true))
    }

    // MARK: 访问密钥

    @Test func accessKeyPolicy() {
        #expect(HostInput.validateAccessKey("abcd1234") == nil)
        #expect(HostInput.validateAccessKey("Abc12345!?") == nil)
        #expect(HostInput.validateAccessKey("") == .tooShort)
        #expect(HostInput.validateAccessKey("abc123") == .tooShort)
        #expect(HostInput.validateAccessKey("abcd efg1") == .badChars)
        #expect(HostInput.validateAccessKey("密钥abcd1234") == .badChars)
        #expect(HostInput.validateAccessKey("abcdefgh") == .needsMix)
        #expect(HostInput.validateAccessKey("12345678") == .needsMix)
        #expect(HostInput.validateAccessKey("!!!!!!!!") == .needsMix)
    }

    @Test func accessKeyLengthBounds() {
        #expect(HostInput.validateAccessKey(String(repeating: "a", count: 127) + "1") == nil)
        #expect(HostInput.validateAccessKey(String(repeating: "a", count: 127) + "1" + "x") == .tooLong)
        #expect(HostInput.validateAccessKey(String(repeating: "a", count: 7) + "1") == nil)
    }

    @Test func badCharsWinsOverLength() {
        #expect(HostInput.validateAccessKey("a 1") == .badChars)
    }

    // MARK: 设备展示

    @Test func platformSymbols() {
        #expect(DevicePresentation.symbol(platform: "iOS") == "smartphone")
        #expect(DevicePresentation.symbol(platform: "android") == "smartphone")
        #expect(DevicePresentation.symbol(platform: "macos") == "laptopcomputer")
        #expect(DevicePresentation.symbol(platform: "Windows") == "desktopcomputer")
        #expect(DevicePresentation.symbol(platform: "linux") == "apple.terminal")
        #expect(DevicePresentation.symbol(platform: "web") == "globe")
        #expect(DevicePresentation.symbol(platform: nil) == "network")
        #expect(DevicePresentation.symbol(platform: "plan9") == "network")
    }

    @Test func unknownPlatformLabelPassesThroughAndEmptyIsNil() {
        #expect(DevicePresentation.label(platform: "plan9") == "plan9")
        #expect(DevicePresentation.label(platform: nil) == nil)
        #expect(DevicePresentation.label(platform: "") == nil)
        #expect(DevicePresentation.label(platform: "macos") == L("accountDevicePlatformMacos"))
    }

    @Test func subtitleJoinsStatePlatformAndVersion() {
        let text = DevicePresentation.subtitle(online: true, platform: "linux", version: " 1.2.3 ")
        #expect(text == [L("deviceOnline"), L("accountDevicePlatformLinux"), "v1.2.3"].joined(separator: " · "))
        #expect(DevicePresentation.subtitle(online: false, platform: nil, version: "  ") == L("deviceOffline"))
    }

    @Test func cloudDevicesSortCurrentThenOnlineThenName() {
        let devices = [
            CloudDevice(deviceId: "1", name: "zeta", platform: nil, isOnline: true, isCurrent: false, appVersion: nil),
            CloudDevice(deviceId: "2", name: "Alpha", platform: nil, isOnline: false, isCurrent: false, appVersion: nil),
            CloudDevice(deviceId: "3", name: "beta", platform: nil, isOnline: true, isCurrent: false, appVersion: nil),
            CloudDevice(deviceId: "4", name: "me", platform: nil, isOnline: false, isCurrent: true, appVersion: nil),
            CloudDevice(deviceId: "5", name: "alpha", platform: nil, isOnline: false, isCurrent: false, appVersion: nil),
        ]
        #expect(DevicePresentation.sorted(cloud: devices).map(\.deviceId) == ["4", "3", "1", "2", "5"])
    }

    @Test func linkDevicesSortOnlineThenName() {
        let devices = [
            LinkDevice(fingerprint: "a", name: "Charlie", platform: nil, online: false),
            LinkDevice(fingerprint: "b", name: "bravo", platform: nil, online: true),
            LinkDevice(fingerprint: "c", name: "Alpha", platform: nil, online: false),
        ]
        #expect(DevicePresentation.sorted(link: devices).map(\.fingerprint) == ["b", "c", "a"])
    }

    @Test func hostFactsCountTaskStatuses() {
        var state = HostState()
        state.tasks = [
            DownloadTask(taskId: "1", url: "u", fileName: "a", status: .completed),
            DownloadTask(taskId: "2", url: "u", fileName: "b", status: .completed),
            DownloadTask(taskId: "3", url: "u", fileName: "c", status: .paused),
            DownloadTask(taskId: "4", url: "u", fileName: "d", status: .failed),
            DownloadTask(taskId: "5", url: "u", fileName: "e", status: .downloading),
        ]
        state.stats = RuntimeStats(activeTasks: 1, pendingTasks: 2, diskFreeBytes: 4096)
        let facts = HostFacts(state: state)
        #expect(facts.completed == 2 && facts.paused == 1 && facts.failed == 1)
        #expect(facts.active == 1 && facts.waiting == 2 && facts.diskFree == 4096)
        #expect(facts.hasBreakdown)
        #expect(!HostFacts(state: HostState()).hasBreakdown)
    }

    // MARK: 错误映射

    @Test func hostErrorTextMapsCodes() {
        #expect(HostFlow.errorText(HostError(.unauthorized)) == L("webLoginInvalidKey"))
        #expect(HostFlow.errorText(HostError(.timeout)) == L("mobileHostErrTimeout"))
        #expect(HostFlow.errorText(HostError(.unavailable)) == L("webLoginUnreachable"))
        #expect(HostFlow.errorText(HostError(.protocolIncompatible)) == L("mobileHostErrIncompatible"))
        #expect(HostFlow.errorText(HostError(.conflict)) == L("localServiceActionFailed"))
        // 经局域网配对添加的主机被拒：授权已撤销，提示重新配对而不是「密钥错误」。
        #expect(HostFlow.errorText(HostError(.unauthorized), paired: true) == L("mobileHostRepairNeeded"))
        #expect(HostFlow.errorText(HostError(.timeout), paired: true) == L("mobileHostErrTimeout"))
    }
}
