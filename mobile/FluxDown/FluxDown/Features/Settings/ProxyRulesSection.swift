import FluxDomain
import FluxUI
import SwiftUI

// 站点代理规则（`proxy_rules`）：规则列表（点按编辑 / 开关 / 左滑删除 / 排序态拖动）、添加、「测试网址」。
// 规则在全部代理模式下生效，优先于模式，自上而下第一条命中者生效；匹配只在引擎里实现，这里只调用 RPC。

/// 规则编辑器的呈现目标。
nonisolated enum ProxyRuleSheetTarget: Identifiable {
    case new
    case edit(ProxyRule)

    var id: String {
        switch self {
        case .new: "new"
        case let .edit(rule): "edit:\(rule.id)"
        }
    }

    var existing: ProxyRule? {
        switch self {
        case .new: nil
        case let .edit(rule): rule
        }
    }
}

struct ProxyRulesSection: View {
    let model: ProxyRulesModel
    /// 排序态：由页面持有并注入 `\.editMode`，这里的 header 按钮切换它。
    @Binding var editMode: EditMode

    @Environment(AppContainer.self) private var container
    @Environment(HostStore.self) private var store
    @Environment(ConfigEditor.self) private var editor

    @State private var sheet: ProxyRuleSheetTarget?
    @State private var pendingDelete: ProxyRule?
    @State private var testURL = ""

    private var readOnly: Bool { store.state.isReadOnly }

    var body: some View {
        let form = editor.form
        let rulesRaw = form.value("proxy_rules") ?? ""
        let rules = ProxyRule.parseList(rulesRaw)
        let mode = form.string("proxy_mode", default: "none")
        let stamp = "\(mode)|\(rulesRaw)"
        Section {
            if rules.isEmpty {
                emptyRow
            } else {
                countRow(rules.count)
                ForEach(Array(rules.enumerated()), id: \.element.id) { index, rule in
                    row(index: index, rule: rule, form: form)
                }
                .onMove { offsets, destination in
                    guard let from = offsets.first, rules.indices.contains(from) else { return }
                    let rule = rules[from]
                    Task { await model.move(rule, toOffset: destination) }
                }
            }
            SettingsActionRow(title: L("proxyRulesAdd"), systemImage: FluxSymbol.add) { sheet = .new }
                .disabled(readOnly)
                .settingsRow(NetworkRow.rulesAdd.id)
        } header: {
            HStack {
                Text(L("proxyRulesTitle"))
                Spacer()
                if rules.count > 1 {
                    Button(editMode.isEditing ? L("mobileViewDone") : L("mobileGeneralReorder")) {
                        withAnimation { editMode = editMode.isEditing ? .inactive : .active }
                    }
                    .font(.footnote)
                    .textCase(nil)
                    .disabled(readOnly)
                }
            }
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text(L("proxyRulesDesc"))
                Text(L("proxyRulesScope"))
            }
        }
        .disabled(readOnly)
        .fluxAnimation(.smooth, value: rules)
        .onChange(of: rules.count) { _, count in
            if count < 2 { editMode = .inactive }
        }
        .sheet(item: $sheet) { target in
            ProxyRuleEditorSheet(existing: target.existing, model: model)
                .environment(\.editMode, .constant(.inactive))
        }
        .alert(
            L("proxyRuleDeleteConfirm", ["name": pendingDelete.map(ProxyRouteText.displayName) ?? ""]),
            isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } })
        ) {
            Button(L("delete"), role: .destructive) {
                if let rule = pendingDelete { Task { await model.remove(rule) } }
                pendingDelete = nil
            }
            Button(L("cancel"), role: .cancel) { pendingDelete = nil }
        } message: {
            Text(L("proxyRuleDeleteDesc"))
        }

        Section {
            testRows(rulesRaw: rulesRaw, stamp: stamp)
        }
        .disabled(readOnly)
    }

    // MARK: 行

    private var emptyRow: some View {
        VStack(spacing: 6) {
            Image(systemName: "arrow.triangle.branch")
                .font(.title2)
                .foregroundStyle(.tertiary)
                .accessibilityHidden(true)
            Text(L("proxyRulesEmptyTitle"))
                .font(.subheadline.weight(.medium))
                .foregroundStyle(.secondary)
            Text(L("proxyRulesEmptyDesc"))
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, minHeight: 72)
        .accessibilityElement(children: .combine)
        .settingsRow(NetworkRow.rules.id)
    }

    private func countRow(_ count: Int) -> some View {
        Text(L("proxyRulesCount", ["count": count]))
            .font(.footnote)
            .foregroundStyle(.secondary)
            .monospacedDigit()
            .frame(maxWidth: .infinity, alignment: .leading)
            .settingsRow(NetworkRow.rules.id)
    }

    private func row(index: Int, rule: ProxyRule, form: SettingsConfigForm) -> some View {
        HStack(spacing: 10) {
            Button {
                if !editMode.isEditing { sheet = .edit(rule) }
            } label: {
                ProxyRuleRow(index: index, rule: rule, manualMissing: ProxyRouteText.needsManualWarning(rule, form: form))
            }
            .buttonStyle(.plain)
            Toggle(L("proxyRuleEnabled"), isOn: Binding(
                get: { rule.enabled },
                set: { value in Task { await model.setEnabled(rule, value) } }
            ))
            .labelsHidden()
            .tint(Color.fdToggleOn)
        }
        .moveDisabled(readOnly)
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button(role: .destructive) {
                pendingDelete = rule
            } label: {
                Label(L("delete"), systemImage: FluxSymbol.delete)
            }
        }
        .contextMenu {
            Button {
                sheet = .edit(rule)
            } label: {
                Label(L("proxyRuleEdit"), systemImage: FluxSymbol.edit)
            }
            Button(role: .destructive) {
                pendingDelete = rule
            } label: {
                Label(L("delete"), systemImage: FluxSymbol.delete)
            }
        }
    }

    // MARK: 测试网址

    @ViewBuilder
    private func testRows(rulesRaw: String, stamp: String) -> some View {
        let trimmed = testURL.trimmingCharacters(in: .whitespacesAndNewlines)
        let running = model.explain == .running
        VStack(alignment: .leading, spacing: 8) {
            SettingsText(title: L("proxyRouteTest"), detail: L("proxyRouteTestDesc"))
            TextField(L("proxyRouteTest"), text: $testURL, prompt: Text(L("proxyRouteUrlPlaceholder")))
                .font(.fluxMono)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.go)
                .onSubmit { run(url: trimmed, rulesRaw: rulesRaw, stamp: stamp) }
                .textFieldStyle(.roundedBorder)
                .frame(minHeight: 44)
        }
        .settingsRow(NetworkRow.rulesTest.id)
        SettingsActionRow(
            title: L("proxyRouteCheck"), runningTitle: L("proxyRouteChecking"), systemImage: "magnifyingglass",
            isRunning: running
        ) {
            run(url: trimmed, rulesRaw: rulesRaw, stamp: stamp)
        }
        .disabled(trimmed.isEmpty || running)
        // 只在 stamp（模式 + 规则原文）与当前值相同时显示：改了规则或模式后旧结果自动隐藏。
        if model.explain.stamp == stamp {
            result
        }
    }

    @ViewBuilder
    private var result: some View {
        switch model.explain {
        case let .done(_, response):
            if let lines = ProxyRouteText.lines(response) {
                VStack(alignment: .leading, spacing: 4) {
                    if let title = lines.title {
                        SettingsStatusLine(text: title.text, tone: title.tone, systemImage: title.tone == .success ? "checkmark.circle.fill" : "minus.circle")
                    }
                    SettingsStatusLine(text: lines.detail.text, tone: lines.detail.tone)
                }
                .transition(.opacity)
            }
        case let .failed(_, text):
            SettingsStatusLine(text: text, tone: .failure)
        case .idle, .running:
            EmptyView()
        }
    }

    private func run(url: String, rulesRaw: String, stamp: String) {
        guard !url.isEmpty else { return }
        let session = container.session
        Task { await model.runExplain(url: url, rulesRaw: rulesRaw, stamp: stamp, using: session) }
    }
}

// MARK: - 规则行

private struct ProxyRuleRow: View {
    let index: Int
    let rule: ProxyRule
    let manualMissing: Bool

    @Environment(\.fluxAccent) private var accent

    var body: some View {
        let dim = !rule.enabled
        HStack(alignment: .top, spacing: 10) {
            Text("\(index + 1)")
                .font(.footnote.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(minWidth: 18, alignment: .trailing)
                .accessibilityLabel(L("proxyRuleOrder", ["index": index + 1]))
            VStack(alignment: .leading, spacing: 3) {
                Text(ProxyRouteText.displayName(rule))
                    .foregroundStyle(dim ? AnyShapeStyle(HierarchicalShapeStyle.tertiary) : AnyShapeStyle(HierarchicalShapeStyle.primary))
                    .lineLimit(1)
                    .truncationMode(.middle)
                HStack(spacing: 6) {
                    tag(ProxyRouteText.matchLabel(rule), color: .secondary)
                    Text(rule.pattern)
                        .font(.caption.monospaced())
                        .foregroundStyle(dim ? AnyShapeStyle(HierarchicalShapeStyle.tertiary) : AnyShapeStyle(HierarchicalShapeStyle.secondary))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                HStack(spacing: 6) {
                    tag(
                        ProxyRouteText.actionLabel(rule),
                        color: rule.action == ProxyRule.actionDirect ? .secondary : accent.color
                    )
                    if manualMissing {
                        tag(L("proxyNotConfigured"), color: Color.fdStatusWarningText)
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .frame(minHeight: 44)
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
    }

    private func tag(_ text: String, color: Color) -> some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(color)
            .lineLimit(1)
            .truncationMode(.middle)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(color.opacity(0.14), in: .capsule)
    }
}
