import FluxDomain
import FluxUI
import SwiftUI

/// S2.6 云功能：配置同步（开关 / 状态 / 立即同步 / 同步范围）+ 多设备协同（云连接状态、在线设备数、重连）。
/// 数据全部来自 `agent.sync` / `agent.cloudConnection` / `agent.session` 分区与快照里的云设备，无本地副本。
struct AccountCloudFeatures: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let sections: AgentSections
    let session: AgentSessionDto?

    @State private var optimistic = OptimisticValues<String, Bool>()
    @State private var syncingNow = false
    @State private var reconnecting = false
    @State private var errorText: String?
    @State private var successCount = 0

    private static let syncKey = "sync"
    private static func scopeKey(_ group: SyncGroup) -> String { "scope.\(group.id.rawValue)" }

    /// 同步范围分组；store 构建不展示 P2P 两组（主机上的取值不受影响）。
    private static var syncGroups: [SyncGroup] {
        #if FULL_FLAVOR
        return SyncRules.groups
        #else
        return SyncRules.groups.filter { $0.id != .bt && $0.id != .ed2k }
        #endif
    }

    var body: some View {
        let state = container.store.state
        let readOnly = state.isReadOnly
        let loggedIn = session != nil
        let sync = sections.sync(state)
        let hasSync = state.has(HostCapability.agentSync)
        let active = loggedIn && sync.enabled

        if hasSync {
            Section {
                syncRow(sync: sync, loggedIn: loggedIn, readOnly: readOnly)
                if active {
                    syncNowRow(sync: sync, readOnly: readOnly)
                }
            } header: {
                Text(L("accountGroupCloudFeatures"))
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("accountCloudFeaturesDesc"))
                    if let errorText { AccountErrorLabel(text: errorText) }
                }
            }
            if active {
                scopeSection(sync: sync, readOnly: readOnly)
            }
        }
        if state.has(HostCapability.agentRemoteTasks) {
            multiDeviceSection(state: state, loggedIn: loggedIn, readOnly: readOnly)
        }
    }

    // MARK: 配置同步

    private func syncRow(sync: SyncStatusDto, loggedIn: Bool, readOnly: Bool) -> some View {
        let phase = SyncRules.phase(sync)
        let status = loggedIn ? statusLine(sync: sync, phase: phase) : StatusLine(symbol: FluxSymbol.cloud, tone: .neutral, text: L("cloudSyncLoginRequired"))
        let binding = Binding(
            get: { optimistic.value(Self.syncKey, actual: sync.enabled) },
            set: { setSync($0) }
        )
        return Toggle(isOn: binding) {
            VStack(alignment: .leading, spacing: 4) {
                Text(L("cloudSyncTitle"))
                Label {
                    Text(status.text).fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: status.symbol)
                        .symbolEffect(.pulse, isActive: phase == .syncing && !reduceMotion)
                }
                .font(.footnote)
                .foregroundStyle(status.tone.color)
            }
        }
        .disabled(readOnly || !loggedIn || optimistic.isPending(Self.syncKey))
        .onChange(of: sync.enabled) { optimistic.settle(Self.syncKey) }
    }

    private func syncNowRow(sync: SyncStatusDto, readOnly: Bool) -> some View {
        Button {
            syncNow()
        } label: {
            HStack(spacing: 8) {
                Text(L("cloudSyncNow"))
                Spacer(minLength: 8)
                if syncingNow { ProgressView() }
            }
            .contentShape(.rect)
        }
        .disabled(readOnly || sync.halted || syncingNow)
        .sensoryFeedback(.success, trigger: successCount)
    }

    private func scopeSection(sync: SyncStatusDto, readOnly: Bool) -> some View {
        Section {
            ForEach(Self.syncGroups) { group in
                let state = group.state(localOnlyKeys: sync.localOnlyKeys)
                let key = Self.scopeKey(group)
                Toggle(
                    isOn: Binding(
                        get: { optimistic.value(key, actual: state == .sync) },
                        set: { _ in toggleScope(group, from: state) }
                    )
                ) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(L(group.labelKey))
                        if state == .mixed {
                            StatusBadge(text: L("syncScopeMixed"), tone: .warning)
                        }
                    }
                }
                .disabled(readOnly || optimistic.isPending(key))
                .accessibilityHint(state == .mixed ? L("syncScopeMixed") : "")
            }
        } header: {
            Text(L("syncScopeTitle"))
        } footer: {
            Text(L("syncScopeDesc"))
        }
        .onChange(of: sync.localOnlyKeys) {
            for group in Self.syncGroups { optimistic.settle(Self.scopeKey(group)) }
        }
    }

    // MARK: 多设备协同

    private func multiDeviceSection(state: HostState, loggedIn: Bool, readOnly: Bool) -> some View {
        let connection = sections.connection(state)
        let presenceKnown = CloudPresence.isKnown(connection, localReady: !readOnly)
        let online = state.cloudDevices.filter { !$0.isCurrent && $0.isOnline }.count
        return Section {
            Button {
                container.router.tab = .devices
            } label: {
                HStack(spacing: 12) {
                    SettingsTile(symbol: "network", color: .blue)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(L("multiDeviceTitle")).foregroundStyle(.primary)
                        Text(L("multiDeviceDesc"))
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 8)
                    if loggedIn {
                        Text(presenceKnown ? L("devicesOnlineCount", ["count": online]) : L("devicePresenceUnknown"))
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.trailing)
                    }
                    Image(systemName: "chevron.right")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .accessibilityHidden(true)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .combine)

            if loggedIn {
                connectionRow(connection: connection, presenceKnown: presenceKnown, readOnly: readOnly)
            }
        } footer: {
            if loggedIn, !presenceKnown {
                Text(L("cloudConnectionStatusHint"))
            }
        }
    }

    private func connectionRow(connection: CloudConnectionDto?, presenceKnown: Bool, readOnly: Bool) -> some View {
        let labelKey = CloudPresence.labelKey(connection, localReady: !readOnly)
        let reasonText = connection?.lastErrorReason.flatMap { AccountRules.errorKey(forReason: $0) }.map { L($0) }
            ?? (connection?.lastError != nil ? L("accountErrorNetwork") : nil)
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: presenceKnown ? FluxSymbol.success : FluxSymbol.cloud)
                    .foregroundStyle(presenceKnown ? Color.fdStatusSeedingText : Color.secondary)
                    .accessibilityHidden(true)
                Text(L(labelKey)).font(.subheadline)
                Spacer(minLength: 8)
                if !presenceKnown {
                    Button {
                        reconnect()
                    } label: {
                        HStack(spacing: 6) {
                            if reconnecting { ProgressView() }
                            Text(L("cloudConnectionRetry"))
                        }
                    }
                    .buttonStyle(.borderless)
                    .disabled(readOnly || reconnecting)
                }
            }
            if !readOnly, let reasonText { AccountErrorLabel(text: reasonText) }
        }
    }

    // MARK: 动作

    private func setSync(_ enabled: Bool) {
        let api = container.agent
        errorText = nil
        Task {
            let error = await optimistic.apply(Self.syncKey, enabled) { () async throws(HostError) in
                if enabled { try await api.syncEnable() } else { try await api.syncDisable() }
            }
            if let error { errorText = AccountText.error(error, context: .sync) }
        }
    }

    private func syncNow() {
        guard !syncingNow else { return }
        syncingNow = true
        errorText = nil
        let api = container.agent
        Task {
            defer { syncingNow = false }
            do throws(HostError) {
                try await api.syncNow()
                successCount += 1
            } catch {
                errorText = AccountText.error(error, context: .sync)
            }
        }
    }

    private func toggleScope(_ group: SyncGroup, from state: SyncGroup.State) {
        let params = group.toggleParams(from: state)
        let api = container.agent
        errorText = nil
        Task {
            let error = await optimistic.apply(Self.scopeKey(group), !params.localOnly) { () async throws(HostError) in
                try await api.syncSetLocalOnly(params)
            }
            if let error { errorText = AccountText.error(error, context: .sync) }
        }
    }

    /// 云连接未建立时重试：先请求立即重连，再刷新设备名册（同 Web `DevicesCard.refresh`）。
    private func reconnect() {
        guard !reconnecting else { return }
        reconnecting = true
        errorText = nil
        let api = container.agent
        Task {
            defer { reconnecting = false }
            do throws(HostError) {
                try await api.remoteReconnect()
                _ = try await api.deviceList()
                container.toasts.show(text: L("cloudConnectionRetryStarted"), tone: .success)
            } catch {
                container.toasts.show(text: AccountText.error(error), tone: .error)
            }
        }
    }

    // MARK: 状态行

    private struct StatusLine {
        nonisolated enum Tone {
            case neutral, accent, success, warning, failure

            var color: Color {
                switch self {
                case .neutral: .secondary
                case .accent: .accentColor
                case .success: .fdStatusSeedingText
                case .warning: .fdStatusWarningText
                case .failure: .fdStatusFailedText
                }
            }
        }

        let symbol: String
        let tone: Tone
        let text: String
    }

    private func statusLine(sync: SyncStatusDto, phase: SyncPhase) -> StatusLine {
        let reason = L(SyncRules.reasonKey(sync))
        switch phase {
        case .off:
            return StatusLine(
                symbol: FluxSymbol.cloud,
                tone: .neutral,
                text: L(BuildFlavor.key(full: "cloudSyncDesc", store: "cloudSyncDescGeneric"))
            )
        case .halted:
            return StatusLine(symbol: "pause.circle.fill", tone: .warning, text: L("cloudSyncStatusHalted", ["reason": reason]))
        case .error:
            return StatusLine(symbol: FluxSymbol.failure, tone: .failure, text: L("cloudSyncStatusError", ["reason": reason]))
        case .connecting:
            return StatusLine(symbol: FluxSymbol.cloud, tone: .neutral, text: L("cloudSyncStatusConnecting"))
        case .syncing:
            return StatusLine(symbol: FluxSymbol.syncing, tone: .accent, text: L("cloudSyncStatusSyncing"))
        case .synced:
            guard let at = sync.lastSyncedAtUnixMs else {
                return StatusLine(symbol: FluxSymbol.success, tone: .success, text: L("cloudSyncStatusSynced"))
            }
            let ago = SyncRules.ago(syncedAtMs: at, nowMs: AccountText.nowMs())
            return StatusLine(
                symbol: FluxSymbol.success,
                tone: .success,
                text: L("cloudSyncStatusSyncedAt", ["time": AccountText.syncAgo(ago)])
            )
        }
    }
}
