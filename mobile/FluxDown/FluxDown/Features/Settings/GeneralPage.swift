import FluxDomain
import FluxUI
import SwiftUI

/// S3 · 通用。分组（03-settings §4）：后台下载 → 系统 → 下载页显示 → 入口。
/// 偏好键一律经 `ConfigEditor` 写入（agent 偏好，☁︎ 键同步）；`mobile.bg_continue` 是设备本地（`UserDefaults`）。
struct GeneralPage: View {
    @Environment(HostStore.self) private var store

    var body: some View {
        let readOnly = store.state.isReadOnly
        SettingsPage(title: L("settingsCatGeneral"), showsReadOnlyBanner: true, showsSyncLegend: true) {
            backgroundSection(readOnly: readOnly)
            systemSection(readOnly: readOnly)
            downloadsViewSection(readOnly: readOnly)
            entriesSection(readOnly: readOnly)
        }
    }

    // MARK: 后台下载

    private func backgroundSection(readOnly: Bool) -> some View {
        Section {
            Toggle(isOn: Binding(
                get: { DeviceSettings.shared.continueInBackground },
                set: { DeviceSettings.shared.continueInBackground = $0 }
            )) {
                SettingsText(title: L(GeneralRow.bgContinue.titleKey), detail: L("mobileGeneralBgContinueDesc"))
            }
            .tint(Color.fdToggleOn)
            .settingsRow(GeneralRow.bgContinue.id)

            if let item = GeneralRow.keepAwake.item {
                ConfigToggleRow(item: item).disabled(readOnly)
            }
        } footer: {
            Text(L("bgDownloadFootnote"))
        }
    }

    // MARK: 系统

    private func systemSection(readOnly: Bool) -> some View {
        Section {
            if let item = GeneralRow.analytics.item {
                ConfigToggleRow(item: item).disabled(readOnly)
            }
            #if FULL_FLAVOR
            GeneralLinkHandlingRows()
            #endif
        } header: {
            Text(L(GeneralRow.Group.system.titleKey))
        }
    }

    // MARK: 下载页显示

    private func downloadsViewSection(readOnly: Bool) -> some View {
        Section {
            ForEach([GeneralRow.sidebarStatus, .sidebarQueues, .sidebarCategory], id: \.id) { row in
                if let item = row.item { ConfigToggleRow(item: item) }
            }
        } header: {
            Text(L(GeneralRow.Group.downloadsView.titleKey))
        } footer: {
            Text(L("mobileGeneralDownloadsViewFooter"))
        }
        .disabled(readOnly)
    }

    // MARK: 入口

    private func entriesSection(readOnly: Bool) -> some View {
        Section {
            ForEach(GeneralRow.entriesRows, id: \.id) { row in
                if let item = row.item { ConfigToggleRow(item: item) }
            }
        } header: {
            Text(L(GeneralRow.Group.entries.titleKey))
        } footer: {
            Text(L("mobileGeneralEntriesFooter"))
        }
        .disabled(readOnly)
    }
}

#if FULL_FLAVOR
// MARK: - 链接与文件打开方式（只读）

/// 读取 Info.plist 的静态声明：`magnet:` / `ed2k://` / `.torrent`。iOS 无法检测是否被其它 App 抢占，只展示本 App 是否声明。
private struct GeneralLinkHandlingRows: View {
    private let declared = GeneralLinkHandling.declared(in: Bundle.main.infoDictionary ?? [:])

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsText(
                title: L(GeneralRow.linkHandling.titleKey),
                detail: GeneralRow.linkHandling.detailKey.map { L($0) }
            )
        }
        .settingsRow(GeneralRow.linkHandling.id)

        ForEach(GeneralLinkKind.allCases) { kind in
            let isDeclared = declared.contains(kind)
            LabeledContent {
                SettingsStatusLine(
                    text: L(isDeclared ? "mobileGeneralLinkDeclared" : "mobileGeneralLinkNotDeclared"),
                    tone: isDeclared ? .success : .warning,
                    systemImage: isDeclared ? FluxSymbol.success : FluxSymbol.warning
                )
            } label: {
                Text(kind.label).font(.fluxMono)
            }
            .accessibilityElement(children: .combine)
        }
    }
}
#endif

// MARK: - 搜索 / 读数

extension GeneralPage {
    /// 设置搜索索引：本页所有行（偏好键恒可用，没有条件隐藏的行）。
    static func searchEntries(_ ctx: SettingsSearchContext) -> [SettingsEntry] {
        let name = L("settingsCatGeneral")
        return GeneralRow.allCases.filter { $0.isAvailable && (FeatureFlags.webhooks || $0 != .activityWebhooks) }.map { row in
            SettingsEntry(
                id: row.id, route: .general, title: L(row.titleKey), detail: row.detailKey.map { L($0) } ?? "",
                breadcrumb: "\(name) › \(L(row.group.titleKey))", symbol: row.symbol
            )
        }
    }
}
