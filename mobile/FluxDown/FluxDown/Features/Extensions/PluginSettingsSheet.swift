import FluxDomain
import FluxUI
import SwiftUI
#if FULL_FLAVOR

/// S11.4 · 插件设置表单：按 manifest `SettingFieldDto` 动态生成控件，提交前做
/// required / number / min-max / select 前置校验，全部通过才发起 `daemon.plugin.updateSettings`
/// （`pattern` 由主机校验，失败文案显示在顶部）。表单在打开瞬间建立，之后快照更新不覆盖用户输入。
struct PluginSettingsSheet: View {
    let plugin: PluginDto

    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @State private var values: [String: String]
    @State private var errors: [String: PluginFieldError] = [:]
    @State private var serverError: String?
    @State private var saving = false
    @State private var shownSecrets: Set<String> = []
    @State private var failTick = 0
    @FocusState private var focused: String?

    init(plugin: PluginDto) {
        self.plugin = plugin
        _values = State(initialValue: PluginSettings.initialValues(plugin.settings, saved: plugin.settingsValues))
    }

    /// 连续的开关合并进同一个 Section；其余字段各占一个 Section。
    private nonisolated enum Block: Identifiable {
        case toggles([PluginSettingField])
        case field(PluginSettingField)

        var id: String {
            switch self {
            case let .toggles(fields): "toggles:" + (fields.first?.key ?? "")
            case let .field(field): field.key
            }
        }
    }

    private var blocks: [Block] {
        var result: [Block] = []
        var toggles: [PluginSettingField] = []
        for field in plugin.settings {
            if field.widget == "toggle" {
                toggles.append(field)
            } else {
                if !toggles.isEmpty {
                    result.append(.toggles(toggles))
                    toggles = []
                }
                result.append(.field(field))
            }
        }
        if !toggles.isEmpty { result.append(.toggles(toggles)) }
        return result
    }

    var body: some View {
        NavigationStack {
            Form {
                if let serverError {
                    Section {
                        Banner(text: L("pluginSettingsSaveFailed", ["message": serverError]), tone: .error, systemImage: FluxSymbol.warning, slim: true)
                            .listRowInsets(EdgeInsets())
                            .listRowBackground(Color.clear)
                    }
                }
                ForEach(blocks) { block in
                    switch block {
                    case let .toggles(fields):
                        Section {
                            ForEach(fields) { toggleRow($0) }
                        } footer: {
                            ForEach(fields.filter { errors[$0.key] != nil || $0.helperScript != nil }) { field in
                                fieldFooter(field)
                            }
                        }
                    case let .field(field):
                        Section {
                            control(field)
                            helperButton(field)
                        } header: {
                            Text(field.title.isEmpty ? field.key : field.title)
                        } footer: {
                            fieldFooter(field)
                        }
                    }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(L("pluginSettingsDialogTitle", ["name": plugin.name]))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("cancel")) { dismiss() }
                        .disabled(saving)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if saving {
                        ProgressView()
                            .accessibilityLabel(L("pluginSettingsSaving"))
                    } else {
                        Button(L("pluginSettingsSaveButton")) { Task { await save() } }
                            .fontWeight(.semibold)
                    }
                }
                ToolbarItemGroup(placement: .keyboard) {
                    Spacer()
                    Button(L("confirm")) { focused = nil }
                }
            }
            .disabled(saving)
            .sensoryFeedback(FluxHaptic.warning.sensoryFeedback, trigger: failTick)
        }
        .presentationDetents([.large])
        .interactiveDismissDisabled(saving)
    }

    // MARK: 控件

    private func binding(_ field: PluginSettingField) -> Binding<String> {
        Binding(
            get: { values[field.key] ?? "" },
            set: { next in
                values[field.key] = next
                errors[field.key] = nil
            }
        )
    }

    private func toggleRow(_ field: PluginSettingField) -> some View {
        let title = field.title.isEmpty ? field.key : field.title
        return Toggle(isOn: Binding(
            get: { values[field.key] == "true" },
            set: { on in
                values[field.key] = on ? "true" : "false"
                errors[field.key] = nil
            }
        )) {
            SettingsText(title: title, detail: field.description)
        }
        .tint(Color.fdToggleOn)
    }

    @ViewBuilder
    private func control(_ field: PluginSettingField) -> some View {
        let title = field.title.isEmpty ? field.key : field.title
        switch field.widget {
        case "select":
            let current = values[field.key] ?? ""
            let known = field.options.contains { $0.value == current }
            Picker(title, selection: binding(field)) {
                if current.isEmpty { Text(L("pluginSelectPlaceholder")).tag("") }
                ForEach(field.options, id: \.value) { Text($0.label).tag($0.value) }
                // 已保存值不在选项里时照原样显示（提交前会被 select 校验拦下并提示）。
                if !known, !current.isEmpty { Text(current).tag(current) }
            }
            .pickerStyle(.menu)
        case "textarea":
            TextField(title, text: binding(field), axis: .vertical)
                .lineLimit(3 ... 6)
                .focused($focused, equals: field.key)
        case "password":
            HStack {
                Group {
                    if shownSecrets.contains(field.key) {
                        TextField(title, text: binding(field))
                    } else {
                        SecureField(title, text: binding(field))
                    }
                }
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($focused, equals: field.key)
                Button {
                    if shownSecrets.contains(field.key) { shownSecrets.remove(field.key) } else { shownSecrets.insert(field.key) }
                } label: {
                    Image(systemName: shownSecrets.contains(field.key) ? "eye.slash" : "eye")
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(.rect)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel(L(shownSecrets.contains(field.key) ? "webHidePassword" : "webShowPassword"))
            }
        case "folder":
            TextField(title, text: binding(field), prompt: Text(L("pluginFolderPickPlaceholder")))
                .font(.fluxMono)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($focused, equals: field.key)
        default:
            let isNumber = field.settingType == "number"
            TextField(
                title,
                text: binding(field),
                prompt: Text(isNumber ? (PluginSettings.rangeHint(field) ?? "") : (field.defaultValue ?? ""))
            )
            .keyboardType(isNumber ? ((field.min ?? -1) >= 0 ? .decimalPad : .numbersAndPunctuation) : .default)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .focused($focused, equals: field.key)
        }
    }

    @ViewBuilder
    private func helperButton(_ field: PluginSettingField) -> some View {
        if let script = field.helperScript, !script.isEmpty {
            Button {
                ExtensionsClipboard.copy(script)
                container.toasts.show(text: L("pluginHelperScriptCopied"), tone: .success)
            } label: {
                Label(field.helperLabel.flatMap { $0.isEmpty ? nil : $0 } ?? L("pluginCopyHelperScript"), systemImage: FluxSymbol.copy)
            }
        }
    }

    @ViewBuilder
    private func fieldFooter(_ field: PluginSettingField) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if field.widget != "toggle", !field.description.isEmpty {
                Text(field.description)
            }
            if field.widget == "toggle", let script = field.helperScript, !script.isEmpty {
                Button(field.helperLabel.flatMap { $0.isEmpty ? nil : $0 } ?? L("pluginCopyHelperScript")) {
                    ExtensionsClipboard.copy(script)
                    container.toasts.show(text: L("pluginHelperScriptCopied"), tone: .success)
                }
                .font(.footnote)
            }
            if let error = errors[field.key] {
                Label(errorText(error), systemImage: FluxSymbol.failure)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
        }
    }

    private func errorText(_ error: PluginFieldError) -> String {
        switch error {
        case .required: L("pluginErrRequired")
        case .number: L("pluginErrNumber")
        case let .min(min): L("pluginErrMin", ["min": min])
        case let .max(max): L("pluginErrMax", ["max": max])
        case .select: L("pluginErrSelect")
        case let .server(message): message
        }
    }

    // MARK: 提交

    private func save() async {
        guard !saving else { return }
        focused = nil
        serverError = nil
        let found = PluginSettings.validateAll(plugin.settings, values: values)
        errors = found
        guard found.isEmpty else {
            failTick += 1
            return
        }
        saving = true
        defer { saving = false }
        do throws(HostError) {
            try await container.session.callVoid(
                HostMethod.daemonPluginUpdateSettings,
                params: PluginUpdateSettingsParams(identity: plugin.identity, entries: PluginSettings.submitEntries(plugin.settings, values: values))
            )
            dismiss()
        } catch {
            serverError = ExtensionErrorText.describe(error)
            failTick += 1
        }
    }
}
#endif
