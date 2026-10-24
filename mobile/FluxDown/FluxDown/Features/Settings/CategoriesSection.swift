import FluxDomain
import FluxUI
import SwiftUI

// S3 · 自定义分类（偏好 `custom_categories`，☁︎）。
// 列表 = `CustomCategoryDto.fromPreference(form.pref)`；每次变更写回整张重排后的列表（position = 下标）。

/// 分类编辑器的呈现目标（新建 / 编辑某条）。
nonisolated enum GeneralCategoryTarget: Identifiable {
    case new
    case edit(CustomCategoryDto)

    var id: String {
        switch self {
        case .new: "new"
        case let .edit(entry): "edit:\(entry.id)"
        }
    }

    var existing: CustomCategoryDto? {
        switch self {
        case .new: nil
        case let .edit(entry): entry
        }
    }
}

/// 分类列表的读写（唯一写路径：`ConfigEditor.setPreference`）。
@MainActor
struct GeneralCategoryStore {
    let editor: ConfigEditor
    let toasts: ToastCenter

    var list: [CustomCategoryDto] {
        CustomCategoryDto.fromPreference(editor.form.pref(CustomCategoryDto.preferenceKey))
    }

    /// 「一键分类目录」的基准目录（本机主机 = 沙盒 Documents 路径，同样有效）。
    var defaultSaveDir: String {
        editor.form.string("default_save_dir").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func write(_ next: [CustomCategoryDto]) {
        editor.setPreference(CustomCategoryDto.preferenceKey, CustomCategoryDto.preferenceValue(next), immediate: true)
    }

    func save(_ entry: CustomCategoryDto) {
        write(CategoryRules.upserting(entry, in: list))
    }

    func move(from offsets: IndexSet, to destination: Int) {
        guard let next = CategoryRules.move(list, from: Array(offsets), to: destination) else { return }
        write(next)
    }

    /// 删除（仅非内置，不弹确认）；toast 提供撤销，恢复删除前的整张列表。
    func delete(_ entry: CustomCategoryDto) {
        guard !entry.isBuiltin else { return }
        let previous = list
        write(previous.filter { $0.id != entry.id })
        toasts.show(
            text: L("mobileGeneralCategoryDeleted", ["name": GeneralCategoryText.displayName(entry)]),
            tone: .info, systemImage: FluxSymbol.delete,
            actionTitle: L("mobileGeneralUndo")
        ) {
            write(previous)
        }
    }

    func applyAutoDirs() {
        let current = list
        let base = defaultSaveDir
        guard let updated = CategoryRules.autoDirs(current, baseDir: base) else { return }
        let changed = CategoryRules.autoDirsChangeCount(current, baseDir: base)
        guard changed > 0 else {
            toasts.show(text: L("mobileGeneralAutoDirsUnchanged"), tone: .info)
            return
        }
        write(updated)
        toasts.show(text: L("mobileGeneralAutoDirsDone", ["n": changed]), tone: .success)
    }

    func resetBuiltin() {
        write(CustomCategoryDto.builtinDefaults)
    }
}

/// 「自定义分类」行目录（下载设置页）；行 id 供设置搜索定位。
nonisolated enum CategoriesRow {
    static let id = "download.categories"
    static let titleKey = "customCategories"
    static let detailKey = "categoryPriorityDragNote"
}

/// 「自定义分类」分组：列表（点按编辑 / 左滑删除 / 长按菜单 / 编辑模式拖动重排）+ 页脚三按钮。
struct CategoriesSection: View {
    @Binding var editing: GeneralCategoryTarget?
    @State private var confirmReset = false
    let readOnly: Bool

    @Environment(AppContainer.self) private var container
    @Environment(ConfigEditor.self) private var editor
    @Environment(\.editMode) private var editMode

    private var isEditingList: Bool { editMode?.wrappedValue.isEditing == true }

    var body: some View {
        let store = GeneralCategoryStore(editor: editor, toasts: container.toasts)
        let list = store.list
        let hasBase = !store.defaultSaveDir.isEmpty
        Section {
            SettingsText(
                title: L(CategoriesRow.titleKey),
                detail: L(CategoriesRow.detailKey),
                synced: true
            )
            .settingsRow(CategoriesRow.id, failureKey: CustomCategoryDto.preferenceKey)

            ForEach(list) { entry in
                Button {
                    if !isEditingList { editing = .edit(entry) }
                } label: {
                    CategoryRow(entry: entry)
                }
                .buttonStyle(.plain)
                .moveDisabled(readOnly)
                .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                    if !entry.isBuiltin {
                        Button(role: .destructive) {
                            store.delete(entry)
                        } label: {
                            Label(L("delete"), systemImage: FluxSymbol.delete)
                        }
                    }
                }
                .contextMenu {
                    Button {
                        editing = .edit(entry)
                    } label: {
                        Label(L("editCategory"), systemImage: FluxSymbol.edit)
                    }
                    if !entry.isBuiltin {
                        Button(role: .destructive) {
                            store.delete(entry)
                        } label: {
                            Label(L("delete"), systemImage: FluxSymbol.delete)
                        }
                    }
                }
            }
            .onMove { offsets, destination in
                store.move(from: offsets, to: destination)
            }
        }
        .disabled(readOnly)
        Section {
            actions(hasBase: hasBase, store: store)
        } footer: {
            if !hasBase {
                Text(L("mobileGeneralAutoDirsNeedDefault"))
            }
        }
        .disabled(readOnly)
    }

    @ViewBuilder
    private func actions(hasBase: Bool, store: GeneralCategoryStore) -> some View {
        SettingsActionRow(title: L("addCategory"), systemImage: FluxSymbol.add) {
            editing = .new
        }
        SettingsActionRow(title: L("autoCategoryDirs"), systemImage: "folder.badge.gearshape") {
            store.applyAutoDirs()
        }
        .disabled(!hasBase)
        SettingsActionRow(title: L("resetBuiltinCategories"), systemImage: "arrow.counterclockwise", role: .destructive) {
            confirmReset = true
        }
        .alert(L("resetBuiltinCategories"), isPresented: $confirmReset) {
            Button(L("resetBuiltinCategories"), role: .destructive) { store.resetBuiltin() }
            Button(L("cancel"), role: .cancel) {}
        } message: {
            Text(L("resetAllCategoriesConfirm"))
        }
    }
}

// MARK: - 分类行

private struct CategoryRow: View {
    let entry: CustomCategoryDto

    @Environment(\.fluxAccent) private var accent

    var body: some View {
        HStack(spacing: 12) {
            SettingsTile(symbol: GeneralCategoryIcon.symbol(entry.icon), color: tint)
            VStack(alignment: .leading, spacing: 3) {
                titleLine
                if let detail = GeneralCategoryText.detail(entry) {
                    Text(detail)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .lineLimit(3)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if !entry.saveDir.isEmpty {
                    Text(entry.saveDir)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .truncationMode(.middle)
                }
            }
            Spacer(minLength: 0)
        }
        .frame(minHeight: 44)
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
    }

    private var titleLine: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(GeneralCategoryText.displayName(entry))
                badge
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(GeneralCategoryText.displayName(entry))
                badge
            }
        }
    }

    private var badge: some View {
        Text(entry.isBuiltin ? L("builtinCategory") : L("customCategory"))
            .font(.caption2.weight(.semibold))
            .foregroundStyle(entry.isBuiltin ? Color.secondary : accent.color)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(
                entry.isBuiltin ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(accent.color.opacity(0.15)),
                in: .capsule
            )
    }

    /// 内置分类取类别色（§3.C），自定义分类取强调色。
    private var tint: Color {
        guard entry.isBuiltin else { return accent.color }
        switch entry.builtinType {
        case "video": return .fdKindVideo
        case "audio": return .fdKindAudio
        case "document": return .fdKindDocument
        case "image": return .fdKindImage
        case "program": return .fdKindProgram
        case "archive": return .fdKindArchive
        default: return .fdKindOther
        }
    }
}
