import FluxDomain
import Foundation
import Testing

/// iOS 局域网发现（NWBrowser）纯逻辑 + 「配对即授权远程控制」的协议规则。
struct LanDiscoveryTests {
    private static let fp1 = String(repeating: "a1", count: 32)
    private static let fp2 = String(repeating: "b2", count: 32)

    @Test func advertParsesAndSanitizesTxt() {
        let advert = LanDiscovery.advert(
            serviceName: "svc",
            txt: ["fp": Self.fp1, "name": "  NAS\u{202E}\u{0}Box \n", "plat": "linux", "ver": ""]
        )
        #expect(advert.fingerprint == Self.fp1)
        #expect(advert.name == "NASBox")
        #expect(advert.platform == "linux")
        #expect(advert.appVersion == nil)
    }

    @Test func advertRejectsBadFingerprintAndLimitsLength() {
        #expect(LanDiscovery.advert(serviceName: "s", txt: ["fp": "ABCD"]).fingerprint == nil)
        #expect(LanDiscovery.advert(serviceName: "s", txt: ["fp": String(repeating: "A", count: 64)]).fingerprint == nil)
        #expect(LanDiscovery.advert(serviceName: "s", txt: ["fp": String(repeating: "g", count: 64)]).fingerprint == nil)
        let long = LanDiscovery.advert(serviceName: "s", txt: ["name": String(repeating: "设", count: 100)])
        #expect(long.name?.count == LanDiscovery.maxNameChars)
        #expect(LanDiscovery.advert(serviceName: "s", txt: ["name": " \n "]).name == nil)
    }

    @Test func entriesDedupeByFingerprintAndSortByName() {
        let adverts = [
            LanDiscovery.Advert(serviceName: "x1", fingerprint: Self.fp2, name: "nas"),
            LanDiscovery.Advert(serviceName: "x2", fingerprint: Self.fp1, name: "Alpha"),
            // 同一指纹的另一网卡广告：去重。
            LanDiscovery.Advert(serviceName: "x1", fingerprint: Self.fp2, name: "nas"),
            // 无指纹：按服务名去重；无名称回退服务名。
            LanDiscovery.Advert(serviceName: "legacy"),
            LanDiscovery.Advert(serviceName: "legacy"),
        ]
        let entries = LanDiscovery.entries(adverts, paired: [])
        #expect(entries.map(\.name) == ["Alpha", "legacy", "nas"])
        #expect(entries.map(\.id) == [Self.fp1, "legacy", Self.fp2])
        #expect(entries.allSatisfy { !$0.paired && !$0.controllable })
    }

    @Test func entriesMarkPairedAndControllableButIgnoreRevoked() {
        let adverts = [
            LanDiscovery.Advert(serviceName: "a", fingerprint: Self.fp1, name: "A"),
            LanDiscovery.Advert(serviceName: "b", fingerprint: Self.fp2, name: "B"),
        ]
        let paired = [
            LinkDeviceInfo(fingerprint: Self.fp1, name: "A", controllable: true),
            LinkDeviceInfo(fingerprint: Self.fp2, name: "B", revoked: true, controllable: true),
        ]
        let entries = LanDiscovery.entries(adverts, paired: paired)
        #expect(entries[0].paired && entries[0].controllable)
        // 被对端解除的设备不算已配对：可重新配对。
        #expect(!entries[1].paired && !entries[1].controllable)
        let noControl = LanDiscovery.entries(adverts, paired: [LinkDeviceInfo(fingerprint: Self.fp1, name: "A")])
        #expect(noControl[0].paired && !noControl[0].controllable)
    }

    @Test func hostPortFormatting() {
        #expect(LanDiscovery.ipv4Literal([192, 168, 1, 20]) == "192.168.1.20")
        #expect(LanDiscovery.ipv4Literal([1, 2, 3]) == nil)
        #expect(LanDiscovery.hostPort(host: "192.168.1.20", port: 17800) == "192.168.1.20:17800")
        #expect(LanDiscovery.hostPort(host: "fe80::1%en0", port: 17800) == "[fe80::1]:17800")
        #expect(LanDiscovery.hostPort(host: "[fe80::1]", port: 1) == "[fe80::1]:1")
    }

    // MARK: 控制配对协议

    @Test func controlFieldsDecodeWithDefaults() throws {
        func decode<T: Decodable>(_ json: String, as type: T.Type = T.self) throws -> T {
            try ProtocolJSON.decode(T.self, from: Data(json.utf8), what: "test")
        }
        let old = try decode(#"{"fingerprint":"f","name":"n"}"#, as: LinkDeviceInfo.self)
        #expect(!old.grantsControl && !old.controllable)
        let new = try decode(#"{"fingerprint":"f","name":"n","grantsControl":true,"controllable":true}"#, as: LinkDeviceInfo.self)
        #expect(new.grantsControl && new.controllable)

        let request = try decode(
            #"{"sessionId":"s","peerName":"p","peerFingerprint":"f","sas":"111222","expiresAtUnixMs":1,"requestsControl":true,"ttlMs":60000}"#,
            as: LinkPairingRequestDto.self
        )
        #expect(request.requestsControl && request.ttlMs == 60000)
        let legacy = try decode(#"{"sessionId":"s","peerName":"p","peerFingerprint":"f","sas":"1","expiresAtUnixMs":1}"#, as: LinkPairingRequestDto.self)
        #expect(!legacy.requestsControl && legacy.ttlMs == nil)

        #expect(try decode(#"{"token":"fdl1.a.b"}"#, as: LinkControlCredentialDto.self).token == "fdl1.a.b")
        #expect(try decode(#"{"code":"1","expiresAtUnixMs":1,"addresses":[],"fingerprint":"f","deviceName":"d","ttlMs":5}"#, as: LinkPairingCodeDto.self).ttlMs == 5)
    }

    @Test func pairBeginOnlyWritesRequestControlWhenSet() throws {
        func json(_ value: some Encodable) throws -> String { String(decoding: try ProtocolJSON.encode(value, what: "t"), as: UTF8.self) }
        #expect(try json(LinkPairBeginParams(address: "a:1", code: "123456")) == #"{"address":"a:1","code":"123456"}"#)
        #expect(
            try json(LinkPairBeginParams(address: "a:1", code: "123456", requestControl: true))
                == #"{"address":"a:1","code":"123456","requestControl":true}"#
        )
        guard case let .success(params) = LinkRules.validate(address: "a:1", code: "123456", requestControl: true) else {
            Issue.record("validate should succeed")
            return
        }
        #expect(params.requestControl)
        #expect(try json(LinkStopPairingParams(code: "123456")) == #"{"code":"123456"}"#)
    }

    @Test func controlUnsupportedReasons() {
        #expect(LinkRules.isControlUnsupported(HostError(.protocolIncompatible, reason: "pairingVersionMismatch")))
        #expect(LinkRules.isControlUnsupported(HostError(.protocolIncompatible, reason: "peerOutdated")))
        #expect(!LinkRules.isControlUnsupported(HostError(.unavailable, reason: "pairingPeerUnreachable")))
        #expect(!LinkRules.isControlUnsupported(HostError(.unavailable)))
    }

    // MARK: 相对 TTL

    @Test func deadlineUsesTtlWhenPresent() {
        // 主机时钟快 1 小时：以收到时刻 + ttl 为准。
        #expect(LinkRules.deadlineMs(ttlMs: 60000, expiresAtUnixMs: 9_999_999_999_999, receivedAtMs: 1000) == 61000)
        #expect(LinkRules.deadlineMs(ttlMs: nil, expiresAtUnixMs: 5000, receivedAtMs: 1000) == 5000)
        #expect(LinkRules.deadlineMs(ttlMs: Int64.max, expiresAtUnixMs: 5000, receivedAtMs: 1000) == Int64.max)
        // 0 / 负数 = 缺失（旧主机）：回退主机时钟。
        #expect(LinkRules.deadlineMs(ttlMs: 0, expiresAtUnixMs: 5000, receivedAtMs: 1000) == 5000)
        #expect(LinkRules.deadlineMs(ttlMs: -5, expiresAtUnixMs: 5000, receivedAtMs: 1000) == 5000)
    }

    @Test func discoveredEntriesTreatRevokedAsUnpaired() {
        let peers = [LinkDiscoveredPeer(fingerprint: "fp", name: "Mac", host: "10.0.0.2", port: 17800)]
        let revoked = [LinkDeviceInfo(fingerprint: "fp", name: "Mac", revoked: true)]
        #expect(LinkRules.discoveredEntries(peers, paired: revoked)[0].paired == false)
        #expect(LinkRules.discoveredEntries(peers, paired: [LinkDeviceInfo(fingerprint: "fp", name: "Mac")])[0].paired)
    }
}
