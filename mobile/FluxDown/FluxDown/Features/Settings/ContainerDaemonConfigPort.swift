import FluxDomain
import Foundation

/// 列表型配置键写入（`WebhookEndpointWriter` / `ProxyRuleWriter`）的生产端口：读 `HostStore` 快照，写 `daemon.config.*`。
@MainActor
final class ContainerDaemonConfigPort: DaemonConfigPort {
    private let container: AppContainer

    init(_ container: AppContainer) {
        self.container = container
    }

    var cachedConfig: DaemonConfigSnapshotDto? {
        let state = container.store.state
        guard !state.isReadOnly else { return nil }
        return DaemonConfigSnapshotDto(revision: state.configRevision, values: state.config)
    }

    func fetchConfig() async throws(HostError) -> DaemonConfigSnapshotDto {
        try await container.session.call(HostMethod.daemonConfigGet)
    }

    func patchConfig(expectedRevision: UInt64, values: [String: String]) async throws(HostError) {
        try await container.session.patchConfig(expectedRevision: expectedRevision, values: values)
    }
}
