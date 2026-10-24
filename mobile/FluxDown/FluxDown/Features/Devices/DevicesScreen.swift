import FluxDomain
import FluxUI
import SwiftUI
import UIKit

/// V1 设备页（底部 Tab 根页）：当前主机卡 · 已保存的主机（切换 / 移除 / 添加）· 云端已信任设备（登录后）·
/// 局域网已配对设备 · 远程任务。账户 / 局域网 / 远程任务三组各自按主机能力（`agent.auth` / `agent.deviceLink` /
/// `agent.remoteTasks`）整段显示或隐藏；内容层全部是系统 `List` 分组，不上玻璃（玻璃只来自系统导航栏 / Tab 栏 / 浮动按钮）。
struct DevicesScreen: View {
    var body: some View {
        DevicesContent()
    }
}

/// 设备页导航栈内的路由。
nonisolated enum DevicesRoute: Hashable {
    /// V1a 全部已信任设备。
    case allDevices
    /// V3 云端设备详情（`CloudDeviceRecord.id`）。
    case cloudDevice(id: String)
    /// V3 局域网设备详情。
    case linkDevice(fingerprint: String)
}

/// 设备页自己呈现的 sheet（同一时刻一个）。
private nonisolated enum DevicesSheet: Identifiable {
    case addDevice, login

    var id: String {
        switch self {
        case .addDevice: "addDevice"
        case .login: "login"
        }
    }
}

private struct DevicesContent: View {
    @Environment(AppContainer.self) private var container
    @Environment(\.dynamicTypeSize) private var typeSize
    private var model: DevicesModel { container.devices }
    @State private var path: [DevicesRoute] = []

    /// 正在切换到的主机 id；非空时其余主机行不可点。
    @State private var switchingId: String?
    @State private var pendingRemoval: HostRef?
    @State private var filter: RemoteTaskRules.Filter = .all
    @State private var sheet: DevicesSheet?
    /// 「添加设备」sheet 的配对状态机（每次呈现新建，sheet 关闭时收尾）。
    @State private var pairing: PairingModel?
    /// 「添加设备」里点了登录：该 sheet 收起后再呈现登录 sheet。
    @State private var loginAfterDismiss = false
    @State private var renaming: CloudDeviceRecord?
    @State private var deleting: CloudDeviceRecord?
    @State private var pendingUnpair: LinkDevice?
    @State private var renamingLink: LinkRenameTarget?

    /// 已信任设备分组最多直接显示的行数，其余进入「管理全部」。
    private static let cloudPreviewLimit = 5

    var body: some View {
        let state = container.store.state
        let current = container.host
        let readOnly = state.isReadOnly
        let cloudCap = state.has(HostCapability.agentAuth)
        let linkCap = state.has(HostCapability.agentDeviceLink)
        let remoteCap = state.has(HostCapability.agentRemoteTasks)
        let loggedIn = model.isLoggedIn(state)
        let presenceKnown = model.presenceKnown(state)
        NavigationStack(path: $path) {
            List {
                Section {
                    HostCard(host: current, connection: state.connection, facts: HostFacts(state: state))
                }

                Section {
                    ForEach(container.hosts) { ref in
                        hostRow(ref, current: current, connection: state.connection)
                    }
                    Button {
                        container.router.sheet = .addHost
                    } label: {
                        Label(L("mobileHostAdd"), systemImage: "plus.circle.fill")
                    }
                } header: {
                    Text(L("mobileHostSwitchTitle"))
                } footer: {
                    Text(hostsFootnote)
                }

                if cloudCap {
                    cloudSection(state: state, loggedIn: loggedIn, presenceKnown: presenceKnown, readOnly: readOnly, reconnectable: remoteCap)
                }

                if linkCap {
                    linkSection(state: state, readOnly: readOnly)
                }

                if cloudCap, loggedIn, remoteCap {
                    remoteSection(state: state, presenceKnown: presenceKnown, readOnly: readOnly)
                }

                if !cloudCap, !linkCap {
                    // 不在列表行里放 ContentUnavailableView（行内被压成固定行高）：用普通 Label 行。
                    Section {
                        Label(L("mobileDevicesEmptyTitle"), systemImage: "laptopcomputer")
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .center)
                    }
                }

                Section {} footer: {
                    Text(L("mobileDevicesFootnote"))
                }
            }
            .listStyle(.insetGrouped)
            .readableContentWidth()
            .rootNavigationTitle(L("mobileNavDevices"))
            .navigationDestination(for: DevicesRoute.self) { route in
                switch route {
                case .allDevices: AllDevicesScreen()
                case let .cloudDevice(id): DeviceDetailScreen(target: .cloud(id: id))
                case let .linkDevice(fingerprint): DeviceDetailScreen(target: .link(fingerprint: fingerprint))
                }
            }
            .refreshable {
                await model.refreshAll(presenceKnown: presenceKnown, reconnectable: remoteCap)
            }
            .toolbar {
                ToolbarItem(placement: .primaryAction) { GlobalSearchButton() }
                ToolbarSpacer(.fixed, placement: .primaryAction)
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Button(L("mobileHostAdd"), systemImage: FluxSymbol.remoteHost) {
                            container.router.sheet = .addHost
                        }
                        if cloudCap || linkCap {
                            Button(L("addDeviceEntry"), systemImage: FluxSymbol.devices) {
                                presentAddDevice()
                            }
                            .disabled(readOnly)
                        }
                    } label: {
                        Label(L("mobileDevicesAddMenu"), systemImage: FluxSymbol.add)
                    }
                }
            }
            .fluxAnimation(.smooth, value: current.id)
            .sensoryFeedback(.selection, trigger: current.id)
        }
        .cloudDeviceRenameAlert(renaming: $renaming)
        .linkDeviceRenameAlert(renaming: $renamingLink)
        .environment(model)
        .sheet(item: $sheet, onDismiss: sheetDismissed) { item in
            switch item {
            case .addDevice:
                if let pairing {
                    AddDeviceSheet(
                        model: pairing,
                        requestLogin: {
                            loginAfterDismiss = true
                            sheet = nil
                        },
                        onPaired: { Task { await model.loadLink() } }
                    )
                }
            case .login:
                LoginSheet()
            }
        }
        .onChange(of: model.syncKey(state), initial: true) {
            model.sync(container.store.state)
        }
        // 全局搜索「添加设备」：标签被选中且搜索 Sheet 收起后打开配对 Sheet。
        .task(id: container.router.pendingIntent) {
            guard container.router.pendingIntent == .addDevice, await container.router.claim(.addDevice) else { return }
            let current = container.store.state
            guard current.has(HostCapability.agentAuth) || current.has(HostCapability.agentDeviceLink) else { return }
            guard !current.isReadOnly else {
                container.toasts.show(text: L("localServiceDisconnected"), tone: .warning)
                return
            }
            presentAddDevice()
        }
    }

    private var hostsFootnote: String {
        var text = L("mobileHostListFootnote")
        if container.hosts.contains(where: { !$0.isLocal }) { text += "\n" + L("mobileHostRemoveHint") }
        return text
    }

    private func presentAddDevice() {
        // 当前是本机主机时发现走 NWBrowser（iOS 上 Rust mDNS 不可用）；远端主机照旧用它的 agent 发现。
        pairing = PairingModel(container: container, options: .init(agentDiscovery: !container.isLocalHost))
        sheet = .addDevice
    }

    /// sheet 收起：先给配对状态机收尾（关闭发现 / 放弃待核对会话），再按需衔接登录 sheet。
    private func sheetDismissed() {
        pairing?.dismissed()
        pairing = nil
        guard loginAfterDismiss else { return }
        loginAfterDismiss = false
        sheet = .login
    }

    // MARK: 主机行

    @ViewBuilder
    private func hostRow(_ ref: HostRef, current: HostRef, connection: Connection) -> some View {
        let isCurrent = ref.id == current.id
        let row = Button {
            switchTo(ref)
        } label: {
            HostRow(ref: ref, isCurrent: isCurrent, connection: connection, isSwitching: switchingId == ref.id)
        }
        .buttonStyle(.plain)
        .disabled(switchingId != nil && switchingId != ref.id)

        if case let .remote(_, _, endpoint) = ref {
            row
                // 删除只弹确认框：不用 `role: .destructive`（会让 List 先行移除该行而数据未变，导致行数不一致崩溃）。
                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                    Button(L("mobileHostRemoveConfirm"), systemImage: FluxSymbol.delete) { pendingRemoval = ref }
                        .tint(Color.fdStatusFailed)
                }
                .contextMenu {
                    Button(L("mobileDevicesSwitchHost"), systemImage: "arrow.left.arrow.right") { switchTo(ref) }
                    Button(L("webCopy"), systemImage: FluxSymbol.copy) {
                        UIPasteboard.general.string = endpoint
                        container.toasts.show(text: L("webCopied"), tone: .success)
                    }
                    Button(L("mobileHostRemoveConfirm"), systemImage: FluxSymbol.delete, role: .destructive) { pendingRemoval = ref }
                }
                .alert(
                    Text(verbatim: L("mobileHostRemoveTitle", ["name": ref.displayName])),
                    isPresented: Binding(get: { pendingRemoval?.id == ref.id }, set: { if !$0 { pendingRemoval = nil } })
                ) {
                    Button(L("mobileHostRemoveConfirm"), role: .destructive) {
                        Task { await HostFlow.remove(ref, container: container) }
                    }
                    Button(L("cancel"), role: .cancel) {}
                } message: {
                    Text(L("mobileHostRemoveMessage"))
                }
        } else {
            row
        }
    }

    private func switchTo(_ ref: HostRef) {
        guard switchingId == nil else { return }
        switchingId = ref.id
        Task {
            await HostFlow.switchTo(ref, container: container)
            switchingId = nil
        }
    }

    // MARK: §C 已信任设备（云账户）

    @ViewBuilder
    private func cloudSection(state: HostState, loggedIn: Bool, presenceKnown: Bool, readOnly: Bool, reconnectable: Bool) -> some View {
        if loggedIn {
            let records = DeviceRules.sorted(model.cloudRecords, presenceKnown: presenceKnown)
            let counts = state.has(HostCapability.agentRemoteTasks)
                ? RemoteTaskRules.countByTarget(model.remoteTasks(state))
                : [:]
            Section {
                presenceRow(state: state, presenceKnown: presenceKnown, readOnly: readOnly, reconnectable: reconnectable)
                cloudStatusRows(records: records)
                ForEach(records.prefix(Self.cloudPreviewLimit)) { record in
                    NavigationLink(value: DevicesRoute.cloudDevice(id: record.id)) {
                        CloudDeviceRow(record: record, presenceKnown: presenceKnown, remoteCount: counts[record.deviceId] ?? 0)
                    }
                    .cloudDeviceActions(record, enabled: !readOnly, renaming: $renaming, deleting: $deleting)
                }
                if records.count > Self.cloudPreviewLimit {
                    NavigationLink(value: DevicesRoute.allDevices) {
                        Text(L("accountDevicesManageAll", ["count": records.count]))
                    }
                }
            } header: {
                Text(L("accountDevicesTitle"))
            } footer: {
                Text(L("accountDevicesDesc"))
            }
        } else {
            // 未登录：只给一行引导，不显示空分组。
            Section {
                Button {
                    sheet = .login
                } label: {
                    Label(L("mobileDevicesLoginGuide"), systemImage: "person.crop.circle.badge.plus")
                }
            } header: {
                Text(L("accountDevicesTitle"))
            }
        }
    }

    /// 云端连接状态行 + 重试 / 重新连接。presence 未知时（云端连接未建立）设备在线状态不可信，按钮会先请求重连。
    private func presenceRow(state: HostState, presenceKnown: Bool, readOnly: Bool, reconnectable: Bool) -> some View {
        let connection = model.sections.connection(state)
        let labelKey = CloudPresence.labelKey(connection, localReady: state.connection == .live)
        let dot: Color = if presenceKnown {
            .fdStatusSeeding
        } else if connection?.state == .connecting || connection?.state == .reconnecting {
            .fdStatusWarning
        } else {
            .fdStatusPaused
        }
        let errorKey: String? = {
            guard state.connection == .live, let connection else { return nil }
            if let reason = connection.lastErrorReason, let key = AccountRules.errorKey(forReason: reason) { return key }
            return connection.lastError == nil ? nil : "accountErrorNetwork"
        }()
        let retryTitle = L(presenceKnown ? "accountDevicesRetry" : "cloudConnectionRetry")
        return VStack(alignment: .leading, spacing: 6) {
            LeadingTrailingRow(spacing: 10) {
                presenceLabel(labelKey: labelKey, dot: dot, known: presenceKnown)
            } trailing: {
                retryControl(title: retryTitle, presenceKnown: presenceKnown, readOnly: readOnly, reconnectable: reconnectable)
            }
            if !presenceKnown {
                Text(L("cloudConnectionStatusHint"))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let errorKey {
                Label(L(errorKey), systemImage: FluxSymbol.failure)
                    .font(.footnote)
                    .foregroundStyle(Color.fdStatusFailedText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func presenceLabel(labelKey: String, dot: Color, known: Bool) -> some View {
        HStack(spacing: 8) {
            Circle().fill(dot).frame(width: 8, height: 8).accessibilityHidden(true)
            Text(L(labelKey))
                .font(.subheadline.weight(.medium))
                .foregroundStyle(known ? Color.fdStatusSeedingText : Color.secondary)
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private func retryControl(title: String, presenceKnown: Bool, readOnly: Bool, reconnectable: Bool) -> some View {
        if model.retrying {
            ProgressView().accessibilityLabel(title)
        } else {
            Button(title) {
                Task { await model.retryCloud(presenceKnown: presenceKnown, reconnectable: reconnectable) }
            }
            .font(.subheadline)
            .buttonStyle(.borderless)
            .disabled(readOnly)
        }
    }

    /// 设备列表的加载 / 失败 / 空状态行（有记录时只在失败时显示一行提示）。
    @ViewBuilder
    private func cloudStatusRows(records: [CloudDeviceRecord]) -> some View {
        switch model.cloudPhase {
        case .idle, .loading:
            if records.isEmpty {
                HStack(spacing: 12) {
                    ProgressView()
                    Text(L("mobileLoading")).foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            }
        case .loaded:
            if records.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Label(L("accountDevicesEmpty"), systemImage: "laptopcomputer")
                        .foregroundStyle(.secondary)
                    reloadButton
                }
            }
        case let .failed(error):
            VStack(alignment: .leading, spacing: 8) {
                Label(L("accountDevicesLoadFailed"), systemImage: FluxSymbol.warning)
                    .foregroundStyle(Color.fdStatusFailedText)
                Text(AccountText.error(error))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                reloadButton
            }
        }
    }

    private var reloadButton: some View {
        Button {
            Task {
                if let error = await model.loadCloud() {
                    container.toasts.show(text: AccountText.error(error), tone: .error)
                }
            }
        } label: {
            Label(L("accountDevicesRetry"), systemImage: FluxSymbol.retry)
        }
        .buttonStyle(.borderless)
    }

    // MARK: §D 已配对设备（局域网）

    @ViewBuilder
    private func linkSection(state: HostState, readOnly: Bool) -> some View {
        let devices = DevicePresentation.sorted(link: state.linkDevices)
        let attached = DeviceRules.attachedLinkFingerprints(cloud: state.cloudDevices, link: state.linkDevices)
        Section {
            if devices.isEmpty {
                Text(L("linkedDevicesEmpty"))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            ForEach(devices) { device in
                NavigationLink(value: DevicesRoute.linkDevice(fingerprint: device.fingerprint)) {
                    LinkDeviceRow(
                        name: device.name,
                        platform: device.platform,
                        online: device.online,
                        revoked: device.revoked,
                        sameAccount: attached.contains(device.fingerprint),
                        grantsControl: model.linkInfos.first { $0.fingerprint == device.fingerprint }?.grantsControl ?? false
                    )
                }
                // 解除只弹确认框：不用 `role: .destructive`（会让 List 先行移除该行而数据未变，导致行数不一致崩溃）。
                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                    if !readOnly {
                        Button(L("linkedDeviceRemove"), systemImage: "minus.circle") {
                            pendingUnpair = device
                        }
                        .tint(Color.fdStatusFailed)
                        if !device.revoked {
                            Button(L("linkedDeviceRename"), systemImage: FluxSymbol.edit) {
                                renamingLink = LinkRenameTarget(fingerprint: device.fingerprint, customName: device.customName ?? "")
                            }
                            .tint(.accentColor)
                        }
                    }
                }
                .contextMenu {
                    if !readOnly {
                        if !device.revoked {
                            Button(L("linkedDeviceRename"), systemImage: FluxSymbol.edit) {
                                renamingLink = LinkRenameTarget(fingerprint: device.fingerprint, customName: device.customName ?? "")
                            }
                        }
                        Button(L("linkedDeviceRemove"), systemImage: "minus.circle", role: .destructive) {
                            pendingUnpair = device
                        }
                    }
                }
                .alert(
                    L("linkedDeviceRemoveTitle"),
                    isPresented: Binding(
                        get: { pendingUnpair?.fingerprint == device.fingerprint },
                        set: { if !$0 { pendingUnpair = nil } }
                    )
                ) {
                    Button(L("linkedDeviceRemove"), role: .destructive) { unpair(device) }
                    Button(L("cancel"), role: .cancel) {}
                } message: {
                    Text(L("linkedDeviceRemoveDesc", ["name": device.name]))
                }
            }
            if case let .failed(error) = model.linkPhase {
                Label(AccountText.error(error, context: .pairing), systemImage: FluxSymbol.failure)
                    .font(.footnote)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
        } header: {
            HStack(spacing: 4) {
                Text(L("linkedDevicesTitle"))
                Spacer(minLength: 8)
                if model.linkPhase == .loading {
                    ProgressView().accessibilityLabel(L("linkedDeviceRefresh"))
                } else {
                    Button {
                        Task {
                            if let error = await model.loadLink() {
                                container.toasts.show(text: AccountText.error(error, context: .pairing), tone: .error)
                            }
                        }
                    } label: {
                        Image(systemName: FluxSymbol.retry)
                            .frame(minWidth: 44, minHeight: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.borderless)
                    .disabled(readOnly)
                    .accessibilityLabel(L("linkedDeviceRefresh"))
                }
                Button {
                    presentAddDevice()
                } label: {
                    Image(systemName: FluxSymbol.add)
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .disabled(readOnly)
                .accessibilityLabel(L("addDeviceEntry"))
            }
        } footer: {
            Text(L("linkedDevicesDesc"))
        }
    }

    private func unpair(_ device: LinkDevice) {
        Task {
            if let error = await model.unpair(fingerprint: device.fingerprint) {
                container.toasts.show(text: AccountText.error(error, context: .pairing), tone: .error)
            }
        }
    }

    // MARK: §E 远程任务

    @ViewBuilder
    private func remoteSection(state: HostState, presenceKnown: Bool, readOnly: Bool) -> some View {
        let all = model.remoteTasks(state)
        let tasks = RemoteTaskRules.sorted(all.filter { filter.matches($0.status) })
        Section {
            if all.isEmpty {
                VStack(spacing: 6) {
                    Label(L("mobileRemoteTasksEmpty"), systemImage: "arrow.up.forward.square")
                        .foregroundStyle(.secondary)
                    Text(L("mobileRemoteTasksEmptyHint"))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .center)
            } else {
                filterPicker
                if tasks.isEmpty {
                    Text(L("mobileRemoteTasksNoMatch"))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                ForEach(tasks) { task in
                    let target = model.target(for: task.toDevice, state: state)
                    RemoteTaskRow(
                        task: task,
                        targetName: target?.name ?? task.toDevice,
                        targetOnline: presenceKnown ? target?.online : nil,
                        busy: model.isBusy(task),
                        readOnly: readOnly
                    ) { action, deleteFiles in
                        model.issue(action, to: task, deleteFiles: deleteFiles)
                    }
                }
            }
        } header: {
            Text(L("remoteTasksGroup"))
        }
    }

    /// 状态筛选：常规字号分段控件；辅助功能字号下改为菜单（分段标签会被截断）。
    private var filterPicker: some View {
        Group {
            if typeSize.isAccessibilitySize {
                Picker(L("mobileRemoteTaskFilter"), selection: $filter) {
                    ForEach(RemoteTaskRules.Filter.allCases, id: \.self) { Text(filterTitle($0)).tag($0) }
                }
                .pickerStyle(.menu)
            } else {
                Picker(L("mobileRemoteTaskFilter"), selection: $filter) {
                    ForEach(RemoteTaskRules.Filter.allCases, id: \.self) { Text(filterTitle($0)).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }
        }
        .listRowBackground(Color.clear)
        .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
    }

    private func filterTitle(_ filter: RemoteTaskRules.Filter) -> String {
        switch filter {
        case .all: L("tabAll")
        case .downloading: L("tabDownloading")
        case .paused: L("tabPaused")
        case .completed: L("tabCompleted")
        case .failedOrCanceled: L("tabError")
        }
    }
}

// MARK: - 连接状态

/// 连接状态 → 圆点色 / 文字色 / 文案（状态靠文字 + 圆点表达，不只靠颜色）。
private struct ConnectionStyle {
    let dot: Color
    let text: Color
    let title: String
    /// 主机切换列表里当前行的状态词（在线 / 重连中 / 离线）。
    let rowState: String

    init(_ connection: Connection) {
        switch connection {
        case .live:
            dot = .fdStatusSeeding
            text = .fdStatusSeedingText
            title = L("mobileHostConnLive")
            rowState = L("mobileHostOnline")
        case .connecting:
            dot = .fdStatusWarning
            text = .fdStatusWarningText
            title = L("mobileHostConnConnecting")
            rowState = L("mobileHostConnConnecting")
        case .stale:
            dot = .fdStatusWarning
            text = .fdStatusWarningText
            title = L("mobileHostConnStale")
            rowState = L("mobileHostConnStale")
        case .failed:
            dot = .fdStatusFailed
            text = .fdStatusFailedText
            title = L("mobileHostConnFailed")
            rowState = L("mobileHostOffline")
        }
    }
}

// MARK: - 当前主机卡

private struct HostCard: View {
    @Environment(AppContainer.self) private var container
    let host: HostRef
    let connection: Connection
    let facts: HostFacts

    var body: some View {
        let style = ConnectionStyle(connection)
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 14) {
                GlyphTile(systemImage: host.symbolName, tint: .accentColor, size: 44)
                VStack(alignment: .leading, spacing: 4) {
                    Text(host.localizedName)
                        .font(.title3.weight(.semibold))
                        .lineLimit(2)
                    Text(host.localizedSubtitle)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .truncationMode(.middle)
                    HStack(spacing: 6) {
                        Circle().fill(style.dot).frame(width: 8, height: 8).accessibilityHidden(true)
                        Text(style.title)
                            .font(.subheadline.weight(.medium))
                            .foregroundStyle(style.text)
                    }
                    if case let .failed(error) = connection {
                        if error.code == .unauthorized, case let .remote(_, _, endpoint) = host,
                           container.linkFingerprint(ofHost: host.id) != nil
                        {
                            // 经局域网配对添加的主机被拒：授权已被主机侧撤销，引导重新配对（不是「更换访问密钥」）。
                            Text(L("mobileHostRepairNeeded"))
                                .font(.footnote)
                                .foregroundStyle(Color.fdStatusFailedText)
                            Button(L("mobileHostRepair")) { container.router.presentRepair(address: endpoint) }
                                .buttonStyle(.bordered)
                                .controlSize(.small)
                        } else {
                            Text(ErrorText.describe(error))
                                .font(.footnote)
                                .foregroundStyle(Color.fdStatusFailedText)
                        }
                    }
                }
            }
            .accessibilityElement(children: .combine)

            Divider()
            stats
            if facts.hasBreakdown { breakdown }
        }
        .padding(.vertical, 6)
    }

    private var stats: some View {
        let disk = facts.diskFree.map { Format.bytes(unsigned: $0) }
        return StatRow(cells: [
            StatCell(value: disk?.value ?? Format.dash, unit: disk?.unit, label: L("mobileDevicesStatDisk")),
            StatCell(value: facts.version ?? Format.dash, unit: nil, label: L("mobileDevicesStatVersion")),
            StatCell(
                value: String(facts.active),
                unit: nil,
                label: L("statusDownloading"),
                emphasis: facts.active > 0 ? .accent : .none
            ),
        ])
    }

    private var breakdown: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 8, alignment: .leading)], alignment: .leading, spacing: 8) {
            if facts.waiting > 0 { StatusBadge(text: L("mobileBadgePendingCount", ["count": facts.waiting]), tone: .neutral) }
            if facts.paused > 0 { StatusBadge(text: L("mobileBadgePausedCount", ["count": facts.paused]), tone: .neutral) }
            if facts.completed > 0 { StatusBadge(text: L("mobileBadgeCompletedCount", ["count": facts.completed]), tone: .success) }
            if facts.failed > 0 { StatusBadge(text: L("mobileBadgeErrorCount", ["count": facts.failed]), tone: .failure) }
        }
    }
}

// MARK: - 主机行

private struct HostRow: View {
    let ref: HostRef
    let isCurrent: Bool
    let connection: Connection
    let isSwitching: Bool

    var body: some View {
        let style = ConnectionStyle(connection)
        HStack(spacing: 14) {
            GlyphTile(systemImage: ref.symbolName, tint: .secondary, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(ref.localizedName)
                    .font(.body)
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(subtitle)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if isSwitching {
                ProgressView()
            } else if isCurrent {
                Circle().fill(style.dot).frame(width: 8, height: 8).accessibilityHidden(true)
                Image(systemName: FluxSymbol.done)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.tint)
                    .accessibilityHidden(true)
            }
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(isCurrent ? .isSelected : [])
        .accessibilityValue(isCurrent ? "\(L("mobileHostCurrent")), \(style.rowState)" : "")
    }

    private var subtitle: String {
        let base = ref.localizedSubtitle
        guard isCurrent else { return base }
        switch connection {
        case .stale, .failed: return L("mobileHostOfflineSubtitle", ["subtitle": base])
        case .live, .connecting: return base
        }
    }
}
