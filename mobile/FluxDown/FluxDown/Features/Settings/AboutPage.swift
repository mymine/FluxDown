import FluxDomain
import FluxUI
import SwiftUI

enum SettingsLinks {
    static let siteHost = "fluxdown.com"
    static let site = "https://fluxdown.com"
    static let privacy = "https://fluxdown.com/privacy"
    static let license = "https://github.com/zerx-lab/FluxDown/blob/main/LICENSE"
    static let dependencies = "https://github.com/zerx-lab/FluxDown/blob/main/Cargo.lock"
    static let changelog = "https://fluxdown.com/changelog"
    /// 远端 `--server` 主机的发布页（升级由服务器管理员在服务器上完成，App 内不提供）。
    static let serverReleases = "https://github.com/zerx-lab/FluxDown/releases"
}

/// 品牌点阵 “FD”：11×7，右下一枚强调点（与 Android 同图）。
private let brandGlyph: [[Character]] = [
    "#####.####.",
    "#.....#...#",
    "#.....#...#",
    "####..#...#",
    "#.....#...#",
    "#.....#...#",
    "#.....####o",
].map { Array($0) }

/// S14 · 关于：品牌头、版本信息、官网 / 隐私 / 开源许可。
/// iOS 没有应用内更新器（更新走 App Store），也不提供捐赠入口（见 App Store 审核指南 3.1.1），因此不含这两项。
struct AboutPage: View {
    @Environment(AppContainer.self) private var container
    @Environment(HostStore.self) private var store
    @Environment(\.openURL) private var openURL

    @State private var showLicenses = false

    var body: some View {
        let version = SettingsAppVersion.current
        let protocolVersion = store.state.info?.protocolVersion
        SettingsPage(title: L("settingsCatAbout")) {
            Section {
                BrandHeader(version: version, protocolVersion: protocolVersion)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }

            Section {
                SettingsInfoRow(title: L("currentVersion"), value: "v\(version)")
                    .settingsRow("about.version")
                if let protocolVersion {
                    SettingsInfoRow(title: L("protocolVersionLabel"), value: "v\(protocolVersion)")
                        .settingsRow("about.protocol")
                }
                if !container.isLocalHost, let info = store.state.info, !info.serviceVersion.isEmpty {
                    SettingsInfoRow(title: L("mobileServiceVersion"), value: "v\(info.serviceVersion)")
                        .settingsRow("about.serviceVersion")
                }
            }

            LogsSection()

            Section {
                externalLink(L("officialWebsite"), symbol: "globe", color: .blue, value: SettingsLinks.siteHost, url: SettingsLinks.site)
                    .settingsRow("about.website")
                externalLink(L("mobileReleaseNotes"), symbol: "list.bullet.rectangle.fill", color: .orange, value: nil, url: SettingsLinks.changelog)
                    .settingsRow("about.changelog")
                if !container.isLocalHost {
                    externalLink(L("webServerReleases"), symbol: "server.rack", color: .indigo, value: nil, url: SettingsLinks.serverReleases)
                        .settingsRow("about.serverReleases")
                }
                externalLink(L("mobilePrivacyPolicy"), symbol: "checkmark.shield.fill", color: .green, value: nil, url: SettingsLinks.privacy)
                    .settingsRow("about.privacy")
                Button {
                    showLicenses = true
                } label: {
                    HStack {
                        SettingsTileLabel(title: L("mobileOpenSource"), symbol: "scroll.fill", color: .gray)
                        Spacer(minLength: 8)
                        Image(systemName: "chevron.right")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(.tertiary)
                            .accessibilityHidden(true)
                    }
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .settingsRow("about.licenses")
            } footer: {
                Text(L("mobileFooter"))
                    .frame(maxWidth: .infinity)
                    .multilineTextAlignment(.center)
                    .padding(.top, 12)
            }
        }
        .sheet(isPresented: $showLicenses) { LicensesSheet() }
    }

    private func externalLink(_ title: String, symbol: String, color: Color, value: String?, url: String) -> some View {
        Button {
            guard let target = URL(string: url) else { return }
            openURL(target) { accepted in
                if !accepted { container.toasts.show(text: L("mobileNoBrowser"), tone: .error) }
            }
        } label: {
            HStack {
                SettingsTileLabel(title: title, symbol: symbol, color: color)
                Spacer(minLength: 8)
                if let value { Text(value).foregroundStyle(.secondary) }
                Image(systemName: "arrow.up.forward")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(.isLink)
    }
}

extension AboutPage {
    static func searchEntries(_ ctx: SettingsSearchContext) -> [SettingsEntry] {
        let name = L("settingsCatAbout")
        func entry(_ id: String, _ title: String, _ detail: String = "") -> SettingsEntry {
            SettingsEntry(id: id, route: .about, title: title, detail: detail, breadcrumb: name, symbol: FluxSymbol.info)
        }
        var list = [
            entry("about.version", L("currentVersion")),
            entry("about.protocol", L("protocolVersionLabel")),
        ]
        list += LogsSection.searchEntries(ctx, route: .about, breadcrumb: "\(name) › \(L("mobileLogsTitle"))")
        if !ctx.isLocalHost { list.append(entry("about.serviceVersion", L("mobileServiceVersion"))) }
        list += [
            entry("about.website", L("officialWebsite"), SettingsLinks.siteHost),
            entry("about.changelog", L("mobileReleaseNotes")),
        ]
        if !ctx.isLocalHost { list.append(entry("about.serverReleases", L("webServerReleases"))) }
        list += [
            entry("about.privacy", L("mobilePrivacyPolicy")),
            entry("about.licenses", L("mobileOpenSource")),
        ]
        return list
    }
}

// MARK: - 品牌头

private struct BrandHeader: View {
    let version: String
    let protocolVersion: UInt32?

    var body: some View {
        VStack(spacing: 10) {
            BrandMark()
                .padding(.top, 8)
            Text(verbatim: "FluxDown")
                .font(.title.weight(.bold))
            Text(L("appDescription"))
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            HStack(spacing: 8) {
                Text(verbatim: "v\(version)").font(.fluxMono).foregroundStyle(.secondary)
                if let protocolVersion {
                    StatusBadge(text: L("mobileProtocolPill", ["v": protocolVersion]), tone: .accent)
                }
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
    }
}

/// 点阵 “FD” 品牌标：`Canvas` 绘制，点 = 主色，强调点 = 当前强调色；随 Dynamic Type 轻度缩放。
private struct BrandMark: View {
    @Environment(\.fluxAccent) private var accent
    @ScaledMetric(relativeTo: .largeTitle) private var dot: CGFloat = 6

    var body: some View {
        let d = min(dot, 10)
        let gap = d / 2
        let glyph = brandGlyph
        let rows = CGFloat(glyph.count)
        let cols = CGFloat(glyph.first?.count ?? 0)
        let accentColor = accent.color
        // Canvas 渲染闭包在 SwiftUI 渲染线程执行：标 `@Sendable`（nonisolated），只捕获主线程先取好的 Sendable 值，
        // 不碰 `self` / MainActor 全局，否则默认 MainActor 隔离会让它在渲染线程触发隔离断言。
        return Canvas { @Sendable context, _ in
            for (r, line) in glyph.enumerated() {
                for (c, ch) in line.enumerated() where ch != "." {
                    let rect = CGRect(x: CGFloat(c) * (d + gap), y: CGFloat(r) * (d + gap), width: d, height: d)
                    let color: Color = ch == "o" ? accentColor : .primary
                    context.fill(Path(ellipseIn: rect), with: .color(color))
                }
            }
        }
        .frame(width: cols * (d + gap) - gap, height: rows * (d + gap) - gap)
        .accessibilityHidden(true)
    }
}

// MARK: - 开源许可

/// 开源许可（Sheet）。iOS 版不随包携带 Android 的 Geist / Lucide（使用系统字体与 SF Symbols），
/// 因此这里如实只列：FluxDown 自身的许可证，以及下载引擎所依赖的开源库清单链接。
private struct LicensesSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @Environment(AppContainer.self) private var container

    var body: some View {
        NavigationStack {
            List {
                Section {
                    link(L("mobileLicenseViewFull"), url: SettingsLinks.license)
                } header: {
                    Text(verbatim: "FluxDown")
                } footer: {
                    Text(verbatim: "GNU Affero General Public License v3.0")
                }
                Section {
                    link(L("mobileLicenseEngineDeps"), url: SettingsLinks.dependencies)
                } footer: {
                    Text(L("mobileLicenseEngineDepsDesc"))
                }
            }
            .navigationTitle(L("mobileOpenSource"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(L("close")) { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func link(_ title: String, url: String) -> some View {
        Button {
            guard let target = URL(string: url) else { return }
            openURL(target) { accepted in
                if !accepted { container.toasts.show(text: L("mobileNoBrowser"), tone: .error) }
            }
        } label: {
            HStack {
                Text(title)
                Spacer(minLength: 8)
                Image(systemName: "arrow.up.forward")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(.isLink)
    }
}
