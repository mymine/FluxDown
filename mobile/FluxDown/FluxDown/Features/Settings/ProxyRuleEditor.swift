import FluxDomain
import FluxUI
import SwiftUI

/// 站点规则编辑器（`.sheet`）：名称、匹配方式、pattern、出口、自定义代理地址、「用网址试一试」。
/// 检测语义与 GPUI / Web / Android 一致：失焦与「试一试」发送 `[draft]` + 样例网址，只看下标 0；
/// 保存前发送 `upsert(当前列表, draft)`（`url = ""`），有任何 issue 都不写入、不关闭。
struct ProxyRuleEditorSheet: View {
    let existing: ProxyRule?
    let model: ProxyRulesModel

    @Environment(AppContainer.self) private var container
    @Environment(ConfigEditor.self) private var editor
    @Environment(\.dismiss) private var dismiss

    @State private var ruleId: String
    @State private var name: String
    @State private var match: String
    @State private var pattern: String
    @State private var action: String
    @State private var proxy: String
    @State private var sample = ""

    @State private var nameError: String?
    @State private var patternError: String?
    @State private var proxyError: String?
    @State private var formError: String?
    @State private var tryResult: Bool?

    @State private var checking = false
    @State private var saving = false
    @State private var confirmDiscard = false
    @State private var confirmDelete = false
    /// 打开时的草稿：与之相比判断是否有未保存修改。
    @State private var baseline: ProxyRule?
    @FocusState private var focus: Field?

    private nonisolated enum Field: Hashable { case name, pattern, proxy, sample }

    init(existing: ProxyRule?, model: ProxyRulesModel) {
        self.existing = existing
        self.model = model
        let seed = existing ?? ProxyRule(id: "pr_\(Int(Date().timeIntervalSince1970 * 1000))")
        _ruleId = State(initialValue: seed.id)
        _name = State(initialValue: seed.name)
        _match = State(initialValue: seed.match)
        _pattern = State(initialValue: seed.pattern)
        _action = State(initialValue: seed.action)
        _proxy = State(initialValue: seed.proxy)
    }

    // MARK: 草稿

    private func buildDraft() -> ProxyRule {
        ProxyRule(
            id: ruleId,
            enabled: existing?.enabled ?? true,
            name: name.trimmingCharacters(in: .whitespacesAndNewlines),
            match: match,
            pattern: pattern.trimmingCharacters(in: .whitespacesAndNewlines),
            action: action,
            proxy: action == ProxyRule.actionCustom ? proxy.trimmingCharacters(in: .whitespacesAndNewlines) : ""
        )
    }

    private var isNew: Bool { existing == nil }
    private var isBusy: Bool { checking || saving }

    private var isDirty: Bool {
        guard let baseline else { return false }
        return buildDraft() != baseline
    }

    private var isRegex: Bool { match == ProxyRule.matchRegex }

    // MARK: 视图

    var body: some View {
        let form = editor.form
        NavigationStack {
            Form {
                nameSection
                patternSection
                actionSection(form: form)
                if action == ProxyRule.actionCustom { proxySection }
                trySection
                if let formError {
                    Section {
                        Label(formError, systemImage: FluxSymbol.failure)
                            .font(.footnote)
                            .foregroundStyle(Color.fdStatusFailedText)
                    }
                }
                if !isNew {
                    Section {
                        Button(role: .destructive) {
                            confirmDelete = true
                        } label: {
                            Label(L("delete"), systemImage: FluxSymbol.delete)
                        }
                        .disabled(isBusy)
                    }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(L(isNew ? "proxyRuleNew" : "proxyRuleEdit"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("cancel")) {
                        if isDirty { confirmDiscard = true } else { dismiss() }
                    }
                    .disabled(saving)
                    .alert(L("mobileWebhookDiscardTitle"), isPresented: $confirmDiscard) {
                        Button(L("mobileWebhookDiscard"), role: .destructive) { dismiss() }
                        Button(L("mobileWebhookKeepEditing"), role: .cancel) {}
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if saving || (checking && saveRequested) {
                        ProgressView()
                    } else {
                        Button(L("proxyRuleSave")) { Task { await check(thenSave: true) } }
                            .fontWeight(.semibold)
                            .disabled(isBusy)
                    }
                }
                ToolbarItemGroup(placement: .keyboard) {
                    Spacer()
                    Button(L("confirm")) { focus = nil }
                }
            }
            .alert(
                L("proxyRuleDeleteConfirm", ["name": existing.map(ProxyRouteText.displayName) ?? ""]),
                isPresented: $confirmDelete
            ) {
                Button(L("delete"), role: .destructive) { Task { await delete() } }
                Button(L("cancel"), role: .cancel) {}
            } message: {
                Text(L("proxyRuleDeleteDesc"))
            }
        }
        .presentationDetents([.large])
        .interactiveDismissDisabled(isDirty || saving)
        .task { if baseline == nil { baseline = buildDraft() } }
        .onChange(of: name) { _, _ in nameError = nil; formError = nil }
        .onChange(of: pattern) { _, _ in patternError = nil; formError = nil; tryResult = nil }
        .onChange(of: proxy) { _, _ in proxyError = nil; formError = nil }
        .onChange(of: match) { _, _ in patternError = nil; tryResult = nil }
        .onChange(of: sample) { _, _ in tryResult = nil }
        .onChange(of: focus) { old, _ in
            switch old {
            case .pattern where !pattern.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                 .proxy where !proxy.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty:
                Task { await check(thenSave: false) }
            default: break
            }
        }
    }

    @State private var saveRequested = false

    private var nameSection: some View {
        Section {
            TextField(L("proxyRuleName"), text: $name, prompt: Text(L("proxyRuleNamePlaceholder")))
                .focused($focus, equals: .name)
        } footer: {
            if let nameError { errorLabel(nameError) }
        }
    }

    private var patternSection: some View {
        Section {
            Picker(L("proxyRuleMatch"), selection: $match) {
                Text(L("proxyRuleMatchDomain")).tag(ProxyRule.matchDomain)
                Text(L("proxyRuleMatchRegex")).tag(ProxyRule.matchRegex)
            }
            .pickerStyle(.segmented)
            TextField(
                L(isRegex ? "proxyRuleMatchRegex" : "proxyRuleMatchDomain"),
                text: $pattern,
                prompt: Text(L(isRegex ? "proxyRulePatternRegexPlaceholder" : "proxyRulePatternDomainPlaceholder")),
                axis: .vertical
            )
            .font(.fluxMono)
            .keyboardType(.URL)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .focused($focus, equals: .pattern)
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text(L(isRegex ? "proxyRulePatternRegexHint" : "proxyRulePatternDomainHint"))
                if let patternError { errorLabel(patternError) }
            }
        }
    }

    private func actionSection(form: SettingsConfigForm) -> some View {
        Section {
            Picker(L("proxyRuleAction"), selection: $action) {
                ForEach(ProxyRule.actions, id: \.self) { wire in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(actionTitle(wire))
                        Text(ProxyRouteText.actionHint(wire, form: form))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .tag(wire)
                }
            }
            .pickerStyle(.navigationLink)
        } footer: {
            if action == ProxyRule.actionManual, !ProxyRouteText.manualConfigured(form) {
                Label(L("proxyRuleActionManualMissing"), systemImage: FluxSymbol.warning)
                    .foregroundStyle(Color.fdStatusWarningText)
            } else {
                Text(ProxyRouteText.actionHint(action, form: form))
            }
        }
    }

    private var proxySection: some View {
        Section {
            TextField(L("proxyRuleProxyUrl"), text: $proxy, prompt: Text(L("proxyRuleProxyUrlPlaceholder")))
                .font(.fluxMono)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($focus, equals: .proxy)
        } header: {
            Text(L("proxyRuleProxyUrl"))
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text(L("proxyRuleProxyUrlHint"))
                if let proxyError { errorLabel(proxyError) }
            }
        }
    }

    private var trySection: some View {
        Section {
            TextField(L("proxyRuleTry"), text: $sample, prompt: Text(L("proxyRouteUrlPlaceholder")))
                .font(.fluxMono)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($focus, equals: .sample)
                .submitLabel(.go)
                .onSubmit { Task { await check(thenSave: false) } }
            SettingsActionRow(
                title: L("proxyRuleTry"), systemImage: "magnifyingglass", isRunning: checking && !saveRequested
            ) {
                Task { await check(thenSave: false) }
            }
            .disabled(isBusy || sample.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            if let tryResult {
                if tryResult {
                    SettingsStatusLine(text: L("proxyRuleTryHit"), tone: .success)
                } else {
                    SettingsStatusLine(text: L("proxyRuleTryMiss"), tone: .neutral, systemImage: "minus.circle")
                }
            }
        }
    }

    private func errorLabel(_ text: String) -> some View {
        Label(text, systemImage: FluxSymbol.failure)
            .foregroundStyle(Color.fdStatusFailedText)
    }

    private func actionTitle(_ wire: String) -> String {
        switch wire {
        case ProxyRule.actionSystem: L("proxyRuleActionSystem")
        case ProxyRule.actionManual: L("proxyRuleActionManual")
        case ProxyRule.actionCustom: L("proxyRuleActionCustom")
        default: L("proxyRuleActionDirect")
        }
    }

    // MARK: 动作

    private func currentList() -> [ProxyRule] {
        ProxyRule.parseList(editor.form.value("proxy_rules"))
    }

    /// 检测并（`thenSave`）保存。无 issue 才写入；有 issue 显示在对应字段 / 底部，不关闭。
    private func check(thenSave: Bool) async {
        guard !checking, !saving else { return }
        let draft = buildDraft()
        // 单条检测强制启用：停用规则在引擎编译时被跳过，否则「试一试」恒为未命中（四端一致）。
        var probe = draft
        probe.enabled = true
        let list = thenSave ? ProxyRule.upserting(draft, in: currentList()) : [probe]
        let draftIndex = list.firstIndex { $0.id == draft.id } ?? 0
        let url = thenSave ? "" : sample.trimmingCharacters(in: .whitespacesAndNewlines)
        checking = true
        saveRequested = thenSave
        formError = nil
        let response: ProxyRouteResponse
        do throws(HostError) {
            response = try await model.check(rules: list, url: url, using: container.session)
        } catch {
            checking = false
            saveRequested = false
            formError = ErrorText.describe(error)
            return
        }
        checking = false
        saveRequested = false
        // 检测期间草稿又被改过：结果已不对应当前输入，丢弃。
        guard buildDraft() == draft else { return }
        let mine = response.issues.filter { $0.index == draftIndex || $0.field.isEmpty }
        nameError = nil
        patternError = nil
        proxyError = nil
        for issue in mine {
            let text = ProxyRouteText.issueText(issue)
            let target: ProxyRuleIssueTarget = issue.field.isEmpty ? .form : ProxyRouteText.target(issue)
            switch target {
            case .name: nameError = nameError ?? text
            case .pattern: patternError = patternError ?? text
            case .proxy: proxyError = proxyError ?? text
            case .form: formError = formError ?? text
            }
        }
        if !thenSave {
            tryResult = (url.isEmpty || !mine.isEmpty) ? nil : response.matched != nil
        }
        guard thenSave, mine.isEmpty else { return }
        saving = true
        let ok = await model.save(draft)
        saving = false
        if ok { dismiss() }
    }

    private func delete() async {
        guard let existing else { return }
        saving = true
        let ok = await model.remove(existing)
        saving = false
        if ok { dismiss() }
    }
}
