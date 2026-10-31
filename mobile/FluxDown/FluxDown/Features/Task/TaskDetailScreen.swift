import FluxDomain
import FluxUI
import SwiftUI

/// D3 任务详情（02-downloads §5）。compact：下载栈推入页；regular：右栏根页（无返回钮，由宿主决定）。
/// 任务被删除（或切换主机后消失）→ 显示「任务已不存在」，1.2 s 后自动返回。
struct TaskDetailScreen: View {
    let taskId: String

    var body: some View {
        // 以 taskId 为身份：右栏切换任务时重置页签 / 展开态。
        TaskDetailLoader(taskId: taskId).id(taskId)
    }
}

/// 读取实时模型并处理「加载中 / 已不存在」。
private struct TaskDetailLoader: View {
    let taskId: String

    @Environment(AppContainer.self) private var container
    /// 曾经见过该任务：只有「见过后消失」才自动返回（从未存在的 id 保留页面由用户返回）。
    @State private var seen = false
    @Environment(\.downloadsNavigation) private var navigation

    var body: some View {
        let state = container.store.state
        let model = TaskDetailModel(state: state, taskId: taskId)
        // 任务表分页载入中查不到任务不代表已被删除：显示加载态，不触发「已不存在」自动返回。
        let loading = state.tasksLoading || (state.connection == .connecting && state.tasks.isEmpty)
        let closing = seen && model == nil && !loading
        Group {
            if let model {
                TaskDetailPage(model: model)
            } else if loading {
                ProgressView(L("pluginCommonLoading"))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .navigationTitle(L("detail"))
                    .navigationBarTitleDisplayMode(.inline)
            } else {
                ContentUnavailableView {
                    Label(L("mobileTaskGone"), systemImage: "questionmark.folder")
                } description: {
                    Text(L("mobileTaskGoneSub"))
                }
                .navigationTitle(L("detail"))
                .navigationBarTitleDisplayMode(.inline)
            }
        }
        .onChange(of: model != nil, initial: true) { _, present in
            if present { seen = true }
        }
        .task(id: closing) {
            guard closing else { return }
            do {
                try await Task.sleep(for: .milliseconds(1200))
            } catch {
                return
            }
            close()
        }
    }

    private func close() {
        navigation?.closeTask(taskId)
    }
}

private nonisolated enum DetailPage: Hashable, CaseIterable {
    case general, speed
    #if FULL_FLAVOR
    case seeding
    #endif
    case log, advanced

    var title: String {
        switch self {
        case .general: L("detailTabGeneral")
        case .speed: L("detailTabSpeed")
        #if FULL_FLAVOR
        case .seeding: L("tabSeeding")
        #endif
        case .log: L("detailTabLog")
        case .advanced: L("detailTabAdvanced")
        }
    }
}

private struct TaskDetailPage: View {
    let model: TaskDetailModel

    @Environment(AppContainer.self) private var container
    @Environment(TaskActions.self) private var actions
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var page: DetailPage = .general
    /// 英雄区滚出后，导航栏内联标题淡入文件名。
    @State private var titleVisible = false
    #if FULL_FLAVOR
    /// 「忽略插件重试」确认（插件失败的任务）。
    @State private var confirmingIgnoreRetry = false
    #endif

    private var task: DownloadTask { model.task }

    /// 页签：「做种」仅 BT（store 不含做种）；「日志」所有协议都有。
    private var pages: [DetailPage] {
        #if FULL_FLAVOR
        if task.protocol == .bt { return [.general, .speed, .seeding, .log, .advanced] }
        #endif
        return [.general, .speed, .log, .advanced]
    }

    #if FULL_FLAVOR
    /// 失败且错误以插件前缀开头：提供「忽略插件重试」。
    private var isPluginFailure: Bool {
        PluginFailure.isIgnorable(status: task.status, errorMessage: task.errorMessage, groupId: task.groupId)
    }
    #endif

    private var currentPage: DetailPage { pages.contains(page) ? page : .general }

    var body: some View {
        List {
            Section {
                TaskDetailHero(model: model)
                    .listRowInsets(EdgeInsets(top: 18, leading: 18, bottom: 18, trailing: 18))
            }
            banner
            switch currentPage {
            case .general: TaskDetailGeneralPage(model: model)
            case .speed: TaskDetailSpeedPage(model: model)
            #if FULL_FLAVOR
            case .seeding: TaskDetailSeedingPage(model: model)
            #endif
            case .log: TaskActivityLogPage(taskId: task.taskId)
            case .advanced: TaskDetailAdvancedPage(model: model)
            }
        }
        .listStyle(.insetGrouped)
        .readableContentWidth()
        .fluxAnimation(.smooth, value: currentPage)
        .safeAreaBar(edge: .top) { pagePicker }
        .onScrollGeometryChange(for: Bool.self) { geometry in
            geometry.contentOffset.y + geometry.contentInsets.top > 96
        } action: { _, visible in
            titleVisible = visible
        }
        .navigationTitle(titleVisible ? task.fileName : "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarVisibility(sizeClass == .compact ? .hidden : .automatic, for: .tabBar)
        .toolbar { toolbarContent }
        #if FULL_FLAVOR
        .alert(L("taskIgnorePluginRetryTitle"), isPresented: $confirmingIgnoreRetry) {
            Button(L("cancel"), role: .cancel) {}
            Button(L("taskIgnorePluginRetry")) { ignorePluginRetry() }
        } message: {
            Text(L("taskIgnorePluginRetryMsg"))
        }
        #endif
        .suppressesBottomAccessory("taskDetail", when: sizeClass == .compact || primaryAction != nil || secondaryAction != nil)
    }

    // MARK: 横幅（失败 / 文件已删除）

    @ViewBuilder private var banner: some View {
        switch model.visual {
        case .failed:
            Section {
                Banner(
                    text: errorText,
                    tone: .error,
                    action: failedBannerAction
                )
                .taskDetailBareRow()
            }
        case .missing:
            Section {
                Banner(text: L("mobileFileMissingRedownload"), tone: .warning, systemImage: "exclamationmark.triangle")
                    .taskDetailBareRow()
            }
        default:
            EmptyView()
        }
    }

    private var failedBannerAction: BannerAction? {
        #if FULL_FLAVOR
        if isPluginFailure {
            return BannerAction(title: L("taskIgnorePluginRetry")) { confirmingIgnoreRetry = true }
        }
        #endif
        return task.protocol == .bt ? nil : BannerAction(title: L("mobileChangeUrl")) { actions.changeUrl(task) }
    }

    private var errorText: String {
        let line = TaskDetailFormat.firstLine(TaskErrorMessage.displayText(task.errorMessage))
        return line.isEmpty ? L("subtitleError") : line
    }

    #if FULL_FLAVOR
    /// `daemon.plugin.ignoreRetry`：跳过插件，用原始链接重试。
    private func ignorePluginRetry() {
        let id = task.taskId
        actions.run(onSuccess: { [toasts = container.toasts] in
            toasts.show(text: L("taskIgnorePluginRetryDone"), tone: .success, systemImage: FluxSymbol.done)
        }) { session throws(HostError) in
            try await session.callVoid(HostMethod.daemonPluginIgnoreRetry, params: TaskIdParams(taskId: id))
        }
    }
    #endif

    // MARK: 分页器（吸顶于导航栏下，系统分段控件）

    @ViewBuilder private var pagePicker: some View {
        let picker = Picker(L("detail"), selection: $page) {
            ForEach(pages, id: \.self) { Text($0.title).tag($0) }
        }
        .labelsHidden()
        // 4 项短标签适合系统分段；≥ AX1 放不下，改菜单式（01-foundations §4.4）。
        Group {
            if typeSize.isAccessibilitySize {
                picker.pickerStyle(.menu)
            } else {
                picker.pickerStyle(.segmented)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
    }

    // MARK: 工具栏

    @ToolbarContentBuilder private var toolbarContent: some ToolbarContent {
        ToolbarItemGroup(placement: .topBarTrailing) {
            ShareLink(item: task.shareUrl) {
                Label(L("mobileShareLink"), systemImage: FluxSymbol.share)
            }
            Menu {
                #if FULL_FLAVOR
                if isPluginFailure {
                    Button {
                        confirmingIgnoreRetry = true
                    } label: {
                        Label(L("taskIgnorePluginRetry"), systemImage: "puzzlepiece.extension")
                    }
                    Divider()
                }
                #endif
                TaskMenuItems(task: task, boosted: model.boosted)
            } label: {
                Label(L("moreActions"), systemImage: FluxSymbol.more)
            }
            .taskDeleteHost()
        }
        if let secondary = secondaryAction {
            ToolbarItem(placement: .bottomBar) {
                Button(action: secondary.action) {
                    Label(secondary.title, systemImage: secondary.systemImage)
                }
                .tint(secondary.isOn ? Color.fdBoost : nil)
                .accessibilityAddTraits(secondary.isOn ? .isSelected : [])
            }
        }
        if let primary = primaryAction {
            ToolbarSpacer(.flexible, placement: .bottomBar)
            ToolbarItem(placement: .bottomBar) {
                Button(action: primary.action) {
                    Label(primary.title, systemImage: primary.systemImage)
                        .labelStyle(.titleAndIcon)
                }
                .buttonStyle(.borderedProminent)
            }
        }
    }

    private struct BarAction {
        let title: String
        let systemImage: String
        var isOn = false
        let action: () -> Void
    }

    /// 底栏主按钮（02-downloads §5.8）：下载中 / 排队 / 准备 → 暂停；暂停 → 继续；失败 → 重试；
    /// 已完成（本机文件在）→ 打开；文件已删除 → 重新下载。远端主机的已完成任务无可用主动作。
    private var primaryAction: BarAction? {
        let id = task.taskId
        switch model.visual {
        case .downloading, .queued, .pending, .preparing, .verifying:
            guard task.status != .unknown else { return nil }
            return BarAction(title: L("pause"), systemImage: FluxSymbol.pause) {
                FluxHaptic.light.play()
                actions.pause([id])
            }
        case .paused:
            return BarAction(title: L("resume"), systemImage: FluxSymbol.resume) {
                FluxHaptic.light.play()
                actions.resume([id])
            }
        case .failed:
            return BarAction(title: L("mobileRetry"), systemImage: FluxSymbol.retry) {
                FluxHaptic.light.play()
                actions.resume([id])
            }
        case .completed, .seeding:
            guard actions.hasLocalFile(task) else { return nil }
            return BarAction(title: L("openFile"), systemImage: FluxSymbol.openFile) { actions.open(task) }
        case .missing:
            return BarAction(title: L("redownloadTask"), systemImage: FluxSymbol.retry) { actions.confirmRedownload(task) }
        }
    }

    /// 底栏次按钮：进行中 / 暂停 → Boost；失败 / 文件已删除 → 复制链接；已完成（本机文件在）→ 在「文件」中显示。
    private var secondaryAction: BarAction? {
        switch model.visual {
        case .downloading, .queued, .pending, .preparing, .verifying, .paused:
            let boosted = model.boosted
            return BarAction(
                title: L(boosted ? "cancelBoost" : "boostDownload"),
                systemImage: boosted ? FluxSymbol.boost : "bolt",
                isOn: boosted
            ) {
                (boosted ? FluxHaptic.light : FluxHaptic.success).play()
                actions.boost(task, boosted: boosted)
            }
        case .failed, .missing:
            return BarAction(title: L("copyUrl"), systemImage: FluxSymbol.copy) { actions.copyLink(task) }
        case .completed, .seeding:
            guard actions.hasLocalFile(task) else { return nil }
            return BarAction(title: L("mobileShowInFiles"), systemImage: FluxSymbol.folder) { actions.showInFiles(task) }
        }
    }
}

// MARK: - 英雄区

private struct TaskDetailHero: View {
    let model: TaskDetailModel

    @Environment(\.dynamicTypeSize) private var typeSize

    private var task: DownloadTask { model.task }
    private var visual: TaskDetailVisual { model.visual }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if !visual.isFinished {
                SegmentMapView(
                    spans: model.heroSpans,
                    totalBytes: task.totalBytes,
                    progress: model.progress,
                    tone: visual.segmentTone,
                    height: SegmentMapView.heroHeight
                )
            }
            statusLine
            TaskHeroStats(items: stats)
            if let subline {
                Text(subline)
                    .font(.footnote)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        // 读法：「{文件名}，下载中，62%，速度 18.6 MB/s，剩余 2 分钟」
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(task.fileName)
        .accessibilityValue(spokenValue)
        .accessibilityAddTraits(.isHeader)
    }

    // MARK: 文件头

    private var header: some View {
        HStack(alignment: typeSize.isAccessibilitySize ? .top : .center, spacing: 14) {
            KindIcon(kind: FileKind.from(fileName: task.fileName), size: 72, dimmed: visual == .missing, badge: badge)
            VStack(alignment: .leading, spacing: 6) {
                Text(task.fileName)
                    .font(.title3.bold())
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                metaLine
            }
        }
    }

    private var badge: KindBadge {
        switch visual {
        case .failed: .failed
        case .missing: .warning
        case .completed, .seeding: .completed
        default: .none
        }
    }

    /// `[协议徽标] 站点 [Boost 优先 ⚡]`
    private var metaLine: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 6) { metaItems }
            VStack(alignment: .leading, spacing: 6) { metaItems }
        }
    }

    @ViewBuilder private var metaItems: some View {
        ProtocolBadge(text: TaskDetailFormat.protocolTag(task.protocol))
        Text(TaskDetailFormat.siteLabel(task))
            .font(.footnote)
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .truncationMode(.middle)
        if model.boosted {
            StatusBadge(text: L("detailBoostActive"), tone: .warning, systemImage: FluxSymbol.boost)
        }
    }

    // MARK: 状态胶囊

    private var statusLine: some View {
        HStack(spacing: 8) {
            StatusBadge(
                text: TaskDetailText.statusWord(visual, queuePosition: model.queuePosition),
                tone: visual.badgeTone,
                systemImage: visual.systemImage
            )
            if visual == .seeding {
                Text(verbatim: "↑ \(Format.speedOrZero(model.speedUp).description)")
                    .font(.footnote)
                    .monospacedDigit()
                    .foregroundStyle(Color.fdStatusSeedingText)
            }
        }
    }

    // MARK: 三栏大数字（§5.2）

    private var stats: [TaskStatItem] {
        let dash = Measure(Format.dash, "")
        switch visual {
        case .downloading:
            return [
                TaskStatItem(label: L("infoSpeed"), measure: Format.speedOrZero(model.speedDown), tint: Color.accentColor),
                TaskStatItem(label: L("infoRemaining"), measure: TaskDetailText.etaMeasure(model.etaSeconds)),
                TaskStatItem(label: L("colProgress"), measure: percent(model.progress)),
            ]
        case .completed, .seeding, .missing:
            let size = task.totalBytes > 0 ? task.totalBytes : task.downloadedBytes
            return [
                TaskStatItem(label: L("infoSize"), measure: Format.bytes(size)),
                TaskStatItem(label: L("infoDuration"), measure: TaskDetailText.durationMeasure(model.durationSeconds)),
                TaskStatItem(label: L("infoCompletedAt"), measure: completedAt ?? dash),
            ]
        case .queued, .pending, .preparing, .verifying, .paused, .failed:
            return [
                TaskStatItem(label: L("infoDownloaded"), measure: Format.bytes(task.downloadedBytes)),
                TaskStatItem(label: L("infoRemaining"), measure: model.remainingBytes.map { Format.bytes($0) } ?? dash),
                TaskStatItem(label: L("colProgress"), measure: percent(model.progress)),
            ]
        }
    }

    /// 进度 → 数值 + `%`（总大小未知 → 「—」）。
    private func percent(_ fraction: Double?) -> Measure {
        guard let fraction else { return Measure(Format.dash, "") }
        return Measure(String(Format.percent(fraction).dropLast()), "%")
    }

    private var completedAt: Measure? {
        guard task.completedAt > 0 else { return nil }
        let date = Date(timeIntervalSince1970: TimeInterval(task.completedAt))
        return Measure(date.formatted(.dateTime.month(.abbreviated).day()), "")
    }

    // MARK: 副行与朗读

    /// `已下 / 总量 · 活跃连接 16 · BT 节点 46`（完成一族由三栏承载，不再重复）。
    private var subline: String? {
        guard !visual.isFinished else { return nil }
        let total = task.totalBytes > 0 ? Format.bytes(task.totalBytes).description : L("unknownSize")
        var parts = ["\(Format.bytes(task.downloadedBytes).description) / \(total)"]
        if model.isTransferring, let transfers = model.runtime?.activeTransfers {
            parts.append("\(L("detailActiveTransfers")) \(transfers)")
        }
        if task.protocol == .bt {
            if model.isTransferring, let peers = model.runtime?.connectedPeers {
                parts.append("\(L("detailConnectedPeers")) \(peers)")
            }
            if model.speedUp > 0, visual != .seeding {
                parts.append("\(L("mobileUploadChip")) \(Format.speedOrZero(model.speedUp).description)")
            }
        }
        return parts.joined(separator: " · ")
    }

    private var spokenValue: String {
        var parts = [TaskDetailText.statusWord(visual, queuePosition: model.queuePosition)]
        if let progress = model.progress { parts.append(Format.percent(progress)) }
        if visual == .downloading {
            parts.append("\(L("infoSpeed")) \(Format.speedOrZero(model.speedDown).description)")
            if let eta = model.etaSeconds { parts.append("\(L("infoRemaining")) \(TaskDetailText.eta(eta))") }
        } else if visual == .failed {
            let line = TaskDetailFormat.firstLine(TaskErrorMessage.displayText(task.errorMessage))
            if !line.isEmpty { parts.append(line) }
        } else if visual == .seeding {
            parts.append("\(L("mobileUploadChip")) \(Format.speedOrZero(model.speedUp).description)")
        }
        return parts.joined(separator: ", ")
    }
}
