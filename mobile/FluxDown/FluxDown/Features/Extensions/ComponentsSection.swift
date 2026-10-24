import FluxDomain
import FluxUI
import SwiftUI
#if FULL_FLAVOR

/// S11.6 · 组件（ffmpeg / yt-dlp）：生效状态、系统 PATH、手动路径、托管安装（版本 / 安装 / 更新 / 卸载 / 进度）。
/// 仅在连接远端 `--server` 主机时由 `ExtensionsPage` 呈现（iOS 本机没有可执行文件）。
struct ComponentsSections: View {
    var body: some View {
        ForEach(ComponentKind.known, id: \.wire) { kind in
            ComponentCardSections(kind: kind)
        }
    }
}

/// 单个组件的状态与操作。进度来自一次性通知 `componentProgress` / `componentResult`
/// （也包含其他客户端发起的安装）。
@MainActor
@Observable
final class ComponentController {
    nonisolated struct Progress: Equatable {
        var installing = false
        var downloaded: Int64 = 0
        var total: Int64 = 0

        var fraction: Double? { total > 0 ? min(1, max(0, Double(downloaded) / Double(total))) : nil }
    }

    let kind: ComponentKind
    /// `daemon.component.get` 回流：仅在快照里的该条目仍是当时那个值时覆盖，快照一更新即让位。
    private(set) var probed: (base: ComponentStatusDto?, status: ComponentStatusDto)?
    private(set) var versions: [String] = []
    private(set) var versionsLoading = false
    private(set) var versionsError: String?
    private(set) var versionsRequested = false
    var selected: String?
    private(set) var progress = Progress()
    private(set) var uninstalling = false
    private(set) var savingPath = false

    @ObservationIgnored private let container: AppContainer
    @ObservationIgnored private var installPending = false
    @ObservationIgnored private var lastResult: ComponentResultNotice?
    @ObservationIgnored private var handledNotice: Int?

    init(kind: ComponentKind, container: AppContainer) {
        self.kind = kind
        self.container = container
    }

    var title: String { ComponentTitles.titleKey(kind).map { L($0) } ?? kind.wire }
    private var session: any HostSession { container.session }

    func effective(snapshot: ComponentStatusDto?) -> ComponentStatusDto? {
        if let probed, probed.base == snapshot { return probed.status }
        return snapshot
    }

    // MARK: 通知

    func consume(_ notices: [HostNotice]) {
        let last = handledNotice ?? (notices.last?.id ?? 0)
        handledNotice = notices.last?.id ?? last
        for notice in notices where notice.id > last {
            switch notice.name {
            case HostNoticeName.componentProgress:
                guard let payload = notice.decode(ComponentProgressNotice.self), payload.component == kind else { continue }
                progress = Progress(installing: true, downloaded: payload.downloadedBytes, total: payload.totalBytes)
            case HostNoticeName.componentResult:
                guard let payload = notice.decode(ComponentResultNotice.self), payload.component == kind else { continue }
                lastResult = payload
                // 非本页发起的安装（如另一个客户端）：结果到达即结束进度展示。
                if !installPending { progress.installing = false }
            default:
                continue
            }
        }
    }

    // MARK: 版本

    func fetchVersions() async {
        guard !versionsLoading else { return }
        versionsRequested = true
        versionsLoading = true
        versionsError = nil
        do throws(HostError) {
            let result: ComponentVersions = try await session.call(HostMethod.daemonComponentListVersions, params: ComponentParams(component: kind))
            versions = result.versions
            selected = result.defaultSelection(current: selected)
        } catch {
            versionsError = ExtensionErrorText.describe(error)
        }
        versionsLoading = false
    }

    // MARK: 安装 / 卸载

    func install() async {
        guard !installPending, !uninstalling else { return }
        installPending = true
        lastResult = nil
        progress = Progress(installing: true)
        do throws(HostError) {
            try await session.callVoid(
                HostMethod.daemonComponentInstall,
                params: ComponentInstallParams(component: kind, version: selected)
            )
            container.toasts.show(text: L("componentsInstallSuccess", ["name": title]), tone: .success)
        } catch {
            // 引擎推送的结果携带真实错误说明；RPC 错误只有错误码。
            let detail: String
            if let pushed = lastResult, !pushed.ok, !pushed.message.isEmpty {
                detail = pushed.message
            } else {
                detail = ExtensionErrorText.describe(error)
            }
            container.toasts.show(text: L("componentsInstallFailed", ["message": detail]), tone: .error)
        }
        installPending = false
        progress.installing = false
    }

    func uninstall() async {
        guard !uninstalling, !installPending else { return }
        uninstalling = true
        do throws(HostError) {
            try await session.callVoid(HostMethod.daemonComponentUninstall, params: ComponentParams(component: kind))
            container.toasts.show(text: L("componentsUninstallSuccess", ["name": title]), tone: .success)
        } catch {
            container.toasts.show(text: L("componentsUninstallFailed", ["message": ExtensionErrorText.describe(error)]), tone: .error)
        }
        uninstalling = false
    }

    // MARK: 手动路径

    /// 写 `component.<kind>.path`（`ConfigEditor` 负责冲突重放 / 回滚 / 失败提示），随后重新探测生效状态。
    func savePath(_ raw: String, snapshot: ComponentStatusDto?, editor: ConfigEditor) async {
        guard let key = kind.manualPathConfigKey, !savingPath else { return }
        savingPath = true
        editor.setNow([key: raw.trimmingCharacters(in: .whitespacesAndNewlines)])
        await editor.settle()
        do throws(HostError) {
            let status: ComponentStatus = try await session.call(HostMethod.daemonComponentGet, params: ComponentParams(component: kind))
            probed = (snapshot, ComponentStatusDto(component: kind, status: status))
        } catch {
            // 探测失败：快照的 `componentsChanged` 事件随后会带来最新状态。
        }
        savingPath = false
    }
}

private struct ComponentCardSections: View {
    let kind: ComponentKind

    @Environment(AppContainer.self) private var container
    @Environment(HostStore.self) private var store
    @State private var controller: ComponentController?

    var body: some View {
        Group {
            if let controller {
                ComponentCardBody(controller: controller)
            }
        }
        .onAppear {
            if controller == nil {
                let created = ComponentController(kind: kind, container: container)
                created.consume(store.notices)
                controller = created
            }
        }
    }
}

private struct ComponentCardBody: View {
    let controller: ComponentController

    @Environment(HostStore.self) private var store
    @Environment(ConfigEditor.self) private var editor
    @State private var components = SectionMemo<[ComponentStatusDto]>(empty: [])
    @State private var pathDraft = ""
    @State private var confirmUninstall = false
    @FocusState private var pathFocused: Bool

    var body: some View {
        @Bindable var controller = controller
        let state = store.state
        let kind = controller.kind
        let snapshot = components.value(state.sections[HostSection.daemonComponents]).first { $0.component == kind }
        let dto = controller.effective(snapshot: snapshot)
        let status = dto?.status
        let readOnly = state.isReadOnly
        let configPath = (kind.manualPathConfigKey.flatMap { editor.form.value($0) } ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let busy = readOnly || controller.progress.installing || controller.uninstalling
        let title = controller.title

        Group {
            Section {
                statusRow(status, title: title, readOnly: readOnly)
                if let status {
                    LabeledContent(L("componentsSystemPathLabel").trimmingCharacters(in: CharacterSet(charactersIn: ": ："))) {
                        Text(status.systemPath.isEmpty ? L("componentsSystemPathNotFound") : status.systemPath)
                            .font(.fluxMono)
                            .multilineTextAlignment(.trailing)
                            .textSelection(.enabled)
                    }
                }
            } header: {
                Text(title)
            } footer: {
                Text(L(descKey(kind)))
            }

            Section {
                TextField(L("componentsManualPathLabel"), text: $pathDraft, prompt: Text(L(pathHintKey(kind))))
                    .font(.fluxMono)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.done)
                    .focused($pathFocused)
                    .onSubmit { save(snapshot: snapshot, configPath: configPath) }
                    .disabled(readOnly || controller.savingPath)
                HStack {
                    Button(L("componentsManualPathSave")) { save(snapshot: snapshot, configPath: configPath) }
                        .disabled(readOnly || controller.savingPath || pathDraft.trimmingCharacters(in: .whitespacesAndNewlines) == configPath)
                    Spacer()
                    if controller.savingPath { ProgressView() }
                    Button(L("componentsManualPathClear"), role: .destructive) {
                        pathDraft = ""
                        Task { await controller.savePath("", snapshot: snapshot, editor: editor) }
                    }
                    .disabled(readOnly || controller.savingPath || (pathDraft.isEmpty && configPath.isEmpty))
                }
                .buttonStyle(.borderless)
            } header: {
                Text(L("componentsManualPathLabel"))
            } footer: {
                Text(L("componentsManualPathDesc", ["name": title]))
            }

            installSection(status, title: title, busy: busy, readOnly: readOnly)
        }
        .onAppear { pathDraft = configPath }
        .onChange(of: configPath) { _, new in if !pathFocused { pathDraft = new } }
        .onChange(of: store.notices.last?.id) { _, _ in controller.consume(store.notices) }
        .task(id: LazyKey(supported: status?.managedSupported == true, ready: !readOnly)) {
            if status?.managedSupported == true, !readOnly, !controller.versionsRequested {
                await controller.fetchVersions()
            }
        }
    }

    /// 版本列表只在「受支持且连接就绪」首次成立时懒拉一次，之后靠刷新按钮。
    private nonisolated struct LazyKey: Hashable {
        let supported: Bool
        let ready: Bool
    }

    private func save(snapshot: ComponentStatusDto?, configPath: String) {
        pathFocused = false
        guard pathDraft.trimmingCharacters(in: .whitespacesAndNewlines) != configPath else { return }
        Task { await controller.savePath(pathDraft, snapshot: snapshot, editor: editor) }
    }

    @ViewBuilder
    private func statusRow(_ status: ComponentStatus?, title: String, readOnly: Bool) -> some View {
        if let status {
            if status.source == "none" {
                Label(
                    L(status.managedSupported ? "componentsStatusNotFound" : "componentsStatusNotFoundUnsupported", ["name": title]),
                    systemImage: FluxSymbol.warning
                )
                .foregroundStyle(Color.fdStatusWarningText)
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 8) {
                        Image(systemName: FluxSymbol.success)
                            .foregroundStyle(Color.fdStatusSeedingText)
                            .accessibilityHidden(true)
                        StatusBadge(text: sourceLabel(status.source), tone: .neutral)
                        if !status.version.isEmpty {
                            Text(verbatim: "v\(status.version)").font(.footnote.monospacedDigit())
                        }
                    }
                    Text(status.path)
                        .font(.fluxMono)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }
        } else if readOnly {
            HStack(spacing: 10) {
                ProgressView()
                Text(L("componentsStatusLoading"))
            }
        } else {
            // 快照已到但没有该组件：主机未编译组件支持（或当前平台不可用）。
            Label(L("settingsUnsupportedOnPlatform"), systemImage: FluxSymbol.warning)
                .foregroundStyle(Color.fdStatusWarningText)
        }
    }

    @ViewBuilder
    private func installSection(_ status: ComponentStatus?, title: String, busy: Bool, readOnly: Bool) -> some View {
        let managedSupported = status?.managedSupported ?? true
        if !managedSupported {
            Section {
                Label(L("componentsManagedUnsupported", ["name": title]), systemImage: "info.circle")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        } else {
            let hasManaged = status?.hasManagedInstall ?? false
            @Bindable var controller = controller
            Section {
                if let status, hasManaged {
                    Text(L("componentsManagedVersionLabel", ["version": status.managedVersion]))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                if let error = controller.versionsError {
                    VStack(alignment: .leading, spacing: 8) {
                        Label(L("componentsVersionsLoadFailed", ["message": error]), systemImage: FluxSymbol.failure)
                            .font(.footnote)
                            .foregroundStyle(Color.fdStatusFailedText)
                        Button(L("componentsRetryVersions")) { Task { await controller.fetchVersions() } }
                            .disabled(readOnly || controller.versionsLoading)
                    }
                }
                Picker(L("componentsInstallSectionTitle"), selection: $controller.selected) {
                    if controller.selected == nil {
                        Text(L(controller.versionsLoading ? "componentsVersionsLoading" : "componentsVersionSelectPlaceholder"))
                            .tag(String?.none)
                    }
                    ForEach(controller.versions, id: \.self) { Text($0).tag(String?.some($0)) }
                }
                .pickerStyle(.menu)
                .disabled(controller.versions.isEmpty || busy)

                Button {
                    Task { await controller.install() }
                } label: {
                    HStack {
                        Text(L(controller.progress.installing ? "componentsInstalling" : (hasManaged ? "componentsReinstallButton" : "componentsInstallButton")))
                        if controller.progress.installing {
                            Spacer()
                            ProgressView()
                        }
                    }
                }
                .disabled(busy)

                if hasManaged {
                    Button(L("componentsUninstallButton"), role: .destructive) { confirmUninstall = true }
                        .disabled(busy)
                        .alert(
                            L("componentsUninstallConfirmTitle", ["name": title]),
                            isPresented: $confirmUninstall
                        ) {
                            Button(L("componentsUninstallButton"), role: .destructive) { Task { await controller.uninstall() } }
                            Button(L("cancel"), role: .cancel) {}
                        } message: {
                            Text(L("componentsUninstallConfirmMsg", ["name": title]))
                        }
                }

                if controller.progress.installing {
                    VStack(alignment: .leading, spacing: 4) {
                        ProgressView(value: controller.progress.fraction)
                        Text(progressText(controller.progress))
                            .font(.footnote.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                    .accessibilityElement(children: .combine)
                }
            } header: {
                HStack {
                    Text(L("componentsInstallSectionTitle"))
                    Spacer()
                    Button(L("componentsFetchVersionsButton")) { Task { await controller.fetchVersions() } }
                        .textCase(nil)
                        .font(.footnote)
                        .disabled(readOnly || controller.versionsLoading)
                }
            } footer: {
                Text(L(installDescKey(controller.kind)))
            }
        }
    }

    private func progressText(_ progress: ComponentController.Progress) -> String {
        guard let fraction = progress.fraction else {
            return "\(Format.bytes(progress.downloaded).description) · \(L("componentsInstallUnknownSize"))"
        }
        return "\(Format.percent(fraction))  \(Format.bytes(progress.downloaded).description) / \(Format.bytes(progress.total).description)"
    }

    private func sourceLabel(_ source: String) -> String {
        switch source {
        case "manual": L("componentsSourceManual")
        case "managed": L("componentsSourceManaged")
        default: L("componentsSourceSystem")
        }
    }

    private func descKey(_ kind: ComponentKind) -> String {
        kind == .ytdlp ? "componentsYtdlpDesc" : "componentsFfmpegDesc"
    }

    private func pathHintKey(_ kind: ComponentKind) -> String {
        kind == .ytdlp ? "componentsManualPathHintYtdlpLinux" : "componentsManualPathHintFfmpegLinux"
    }

    private func installDescKey(_ kind: ComponentKind) -> String {
        kind == .ytdlp ? "componentsInstallSectionDescYtdlp" : "componentsInstallSectionDescFfmpeg"
    }
}
#endif
