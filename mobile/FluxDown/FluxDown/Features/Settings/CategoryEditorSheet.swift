import FluxDomain
import FluxUI
import SwiftUI

/// S3a · 分类编辑器（新建 / 编辑）。自包含：只依赖分类条目与回调，设置页与下载页的分类菜单都能 present。
/// 校验与条目生成全部走 `CategoryRules.build`；保存 / 删除由调用方写回 `custom_categories`。
struct CategoryEditorSheet: View {
    /// nil = 新建。
    let existing: CustomCategoryDto?
    let onSave: (CustomCategoryDto) -> Void
    /// nil = 不提供删除（新建 / 内置分类）。
    let onDelete: (() -> Void)?

    @Environment(AppContainer.self) private var container
    @Environment(\.dismiss) private var dismiss
    @Environment(\.fluxAccent) private var accent

    @State private var draft: CategoryDraft
    @State private var error: CategoryValidationError?
    @State private var confirmDelete = false
    @State private var confirmDiscard = false
    @State private var showDirectoryPicker = false
    private let initial: CategoryDraft

    init(existing: CustomCategoryDto?, onSave: @escaping (CustomCategoryDto) -> Void, onDelete: (() -> Void)? = nil) {
        self.existing = existing
        self.onSave = onSave
        self.onDelete = existing?.isBuiltin == false ? onDelete : nil
        let start = CategoryDraft(existing: existing)
        _draft = State(initialValue: start)
        initial = start
    }

    private var isDirty: Bool { draft != initial }
    private var showsRules: Bool { existing?.hasMatchRules ?? true }
    private var showsSaveDir: Bool { !(existing?.isAll ?? false) }
    private var isRegex: Bool { draft.matchMode == "regex" }
    private var isUrl: Bool { draft.matchMode == "url" }

    var body: some View {
        NavigationStack {
            Form {
                nameSection
                iconSection
                if showsRules {
                    matchSection
                }
                if showsSaveDir {
                    saveDirSection
                }
                if onDelete != nil {
                    Section {
                        Button(role: .destructive) {
                            confirmDelete = true
                        } label: {
                            Label(L("deleteCategory"), systemImage: FluxSymbol.delete)
                                .frame(minHeight: 44, alignment: .leading)
                                .contentShape(.rect)
                        }
                        .alert(L("deleteCategory"), isPresented: $confirmDelete) {
                            Button(L("delete"), role: .destructive) {
                                onDelete?()
                                dismiss()
                            }
                            Button(L("cancel"), role: .cancel) {}
                        } message: {
                            Text(L("deleteCategoryConfirm"))
                        }
                    }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(existing == nil ? L("addCategory") : L("editCategory"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("cancel")) {
                        if isDirty { confirmDiscard = true } else { dismiss() }
                    }
                    .alert(L("mobileGeneralDiscardTitle"), isPresented: $confirmDiscard) {
                        Button(L("mobileGeneralDiscard"), role: .destructive) { dismiss() }
                        Button(L("mobileGeneralKeepEditing"), role: .cancel) {}
                    }

                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(L("confirm"), action: save)
                }
            }
            .fluxAnimation(.smooth, value: draft.matchMode)
            .fluxAnimation(.smooth, value: error)
            .onChange(of: draft) { error = nil }
            .sheet(isPresented: $showDirectoryPicker) {
                RemoteDirectoryPicker(startPath: draft.saveDir) { draft.saveDir = $0 }
            }
        }
        .presentationDetents([.large])
        .interactiveDismissDisabled(isDirty)
    }

    // MARK: 名称

    private var nameSection: some View {
        let prompt = existing?.builtinType.map { L(CategoryRules.builtinLabelKey($0)) } ?? L("categoryNameHint")
        return Section {
            TextField(L("categoryName"), text: $draft.name, prompt: Text(prompt))
                .submitLabel(.done)
                .frame(minHeight: 44)
        } header: {
            Text(L("categoryName"))
        } footer: {
            errorLabel(for: .nameRequired)
        }
    }

    // MARK: 图标

    private var iconSection: some View {
        Section(L("categoryIcon")) {
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 5), spacing: 8) {
                ForEach(CategoryRules.iconNames, id: \.self) { name in
                    CategoryIconCell(name: name, isSelected: draft.icon == name, accent: accent.color) {
                        draft.icon = name
                    }
                }
            }
            .padding(.vertical, 4)
        }
    }

    // MARK: 匹配规则

    private var matchSection: some View {
        Section {
            Picker(L("matchMode"), selection: $draft.matchMode) {
                Text(L("matchByExtension")).tag("extension")
                Text(L("matchByRegex")).tag("regex")
                Text(L("matchByUrl")).tag("url")
            }
            .pickerStyle(.segmented)
            .accessibilityLabel(L("matchMode"))

            if isRegex {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("regexLabel")).font(.subheadline.weight(.medium))
                    TextField(L("regexLabel"), text: $draft.regexText, prompt: Text(L("regexHint")))
                        .font(.fluxMono)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.done)
                        .frame(minHeight: 44)
                }
                .transition(.opacity)
            } else if isUrl {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("urlPatternLabel")).font(.subheadline.weight(.medium))
                    TextField(L("urlPatternLabel"), text: $draft.urlText, prompt: Text(L("urlPatternHint")))
                        .font(.fluxMono)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .submitLabel(.done)
                        .frame(minHeight: 44)
                }
                .transition(.opacity)
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    Text(L("extensionsLabel")).font(.subheadline.weight(.medium))
                    TextField(L("extensionsLabel"), text: $draft.extensionsText, prompt: Text(L("extensionsHint")))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.done)
                        .frame(minHeight: 44)
                    let tokens = CategoryRules.parseExtensions(draft.extensionsText)
                    if !tokens.isEmpty {
                        CategoryTokenFlow(spacing: 6) {
                            ForEach(Array(tokens.enumerated()), id: \.offset) { _, token in
                                Text(".\(token)")
                                    .font(.footnote.monospaced())
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 5)
                                    .background(.fill.tertiary, in: .capsule)
                            }
                        }
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel(tokens.map { ".\($0)" }.joined(separator: ", "))
                    }
                }
                .transition(.opacity)
            }
        } header: {
            Text(L("matchMode"))
        } footer: {
            VStack(alignment: .leading, spacing: 6) {
                if isUrl {
                    Text(L("urlPatternDesc"))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                errorLabel(for: .extensionsRequired)
                errorLabel(for: .regexInvalid)
                errorLabel(for: .urlPatternRequired)
            }
        }
    }

    // MARK: 保存目录

    private var saveDirSection: some View {
        Section {
            TextField(L("categorySaveDir"), text: $draft.saveDir, prompt: Text(L("mobileSaveDirUnset")))
                .font(.fluxMono)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.done)
                .frame(minHeight: 44)
            if !container.isLocalHost {
                Button {
                    showDirectoryPicker = true
                } label: {
                    Label(L("mobileBrowseServerFolders"), systemImage: "folder.badge.gearshape")
                        .frame(minHeight: 44, alignment: .leading)
                        .contentShape(.rect)
                }
            }
            if !draft.saveDir.isEmpty {
                Button {
                    draft.saveDir = ""
                } label: {
                    Label(L("restoreDefaultPath"), systemImage: "arrow.uturn.backward")
                        .frame(minHeight: 44, alignment: .leading)
                        .contentShape(.rect)
                }
            }
        } header: {
            Text(L("categorySaveDir"))
        } footer: {
            Text(L("categorySaveDirDesc"))
        }
    }

    // MARK: 校验与保存

    @ViewBuilder
    private func errorLabel(for kind: CategoryValidationError) -> some View {
        if error == kind {
            Label(L(kind.i18nKey), systemImage: FluxSymbol.failure)
                .font(.footnote)
                .foregroundStyle(Color.fdStatusFailedText)
                .fixedSize(horizontal: false, vertical: true)
                .transition(.opacity)
        }
    }

    private func save() {
        switch CategoryRules.build(draft, nowMs: Int64(Date().timeIntervalSince1970 * 1000)) {
        case let .failure(failure):
            error = failure
            FluxHaptic.warning.play()
        case let .success(entry):
            onSave(entry)
            dismiss()
        }
    }
}

// MARK: - 图标格

/// 52 pt 圆角方块图标格：选中 = 强调色 2 pt 描边 + 勾角标（形状 + 描边，不只靠颜色）。
private struct CategoryIconCell: View {
    let name: String
    let isSelected: Bool
    let accent: Color
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: GeneralCategoryIcon.symbol(name))
                .font(.title3)
                .foregroundStyle(isSelected ? accent : Color.primary)
                .frame(maxWidth: .infinity, minHeight: 52)
                .background(.fill.tertiary, in: .rect(cornerRadius: 12, style: .continuous))
                .overlay {
                    if isSelected {
                        RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(accent, lineWidth: 2)
                    }
                }
                .overlay(alignment: .topTrailing) {
                    if isSelected {
                        Image(systemName: FluxSymbol.success)
                            .font(.caption)
                            .foregroundStyle(accent)
                            .background(Circle().fill(.background).padding(1))
                            .offset(x: 4, y: -4)
                    }
                }
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(L(GeneralCategoryIcon.nameKey(name)))
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}

// MARK: - 扩展名胶囊换行布局

/// 自动换行的水平流式布局（扩展名 Token 预览）。
private nonisolated struct CategoryTokenFlow: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        arrange(width: proposal.width ?? .infinity, subviews: subviews).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = arrange(width: bounds.width, subviews: subviews)
        for (index, origin) in result.origins.enumerated() {
            subviews[index].place(
                at: CGPoint(x: bounds.minX + origin.x, y: bounds.minY + origin.y), proposal: .unspecified
            )
        }
    }

    private func arrange(width: CGFloat, subviews: Subviews) -> (origins: [CGPoint], size: CGSize) {
        var origins: [CGPoint] = []
        var x: CGFloat = 0
        var y: CGFloat = 0
        var rowHeight: CGFloat = 0
        var maxX: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            origins.append(CGPoint(x: x, y: y))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
            maxX = max(maxX, x - spacing)
        }
        return (origins, CGSize(width: maxX, height: y + rowHeight))
    }
}
