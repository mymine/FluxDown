import FluxDomain
import FluxUI
import SwiftUI
#if FULL_FLAVOR

/// 做种时长行：数字 + 单位菜单（分钟 / 小时 / 天）+ ± 步进。
///
/// 存储：数值键（`bt_seed_*_time_limit_minutes`，☁︎ 同步）始终以**分钟**落库；单位键（`…_unit`，仅本机）只记录展示单位。
/// 因此输入框显示 `分钟 ÷ 单位`（最多两位小数），提交时换算回分钟；切换单位保留输入框里的数字
/// （同限速行），并在一次写入里同时写回两个键。0 = 关闭。
struct BtDurationRow: View {
    let item: SettingsItem
    let unitKey: String
    let unitTitleKey: String

    @Environment(ConfigEditor.self) private var editor
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var draft = ""
    @State private var warnTick = 0
    @FocusState private var focused: Bool

    private var minutes: Int64 { max(0, editor.form.long(item.key, default: 0)) }
    private var unit: BtDurationUnit { BtDurationUnit(wire: editor.form.value(unitKey)) }

    var body: some View {
        let title = item.title
        let off = L("autoRetryOff")
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 8))
            : AnyLayout(HStackLayout(spacing: 8))
        VStack(alignment: .leading, spacing: 10) {
            SettingsText(title: title, synced: item.isSynced)
            layout {
                TextField(title, text: $draft, prompt: Text(off))
                    .keyboardType(.decimalPad)
                    .focused($focused)
                    .settingsValueField(width: nil)
                    .onChange(of: draft) { _, new in
                        let clean = SettingsRateLimit.sanitize(new)
                        if clean != new { draft = clean }
                    }
                    .accessibilityLabel(title)
                    .accessibilityValue(minutes == 0 ? off : "\(draft) \(L(unit.titleKey))")
                unitMenu
                Stepper(title, onIncrement: { nudge(up: true) }, onDecrement: { nudge(up: false) })
                    .labelsHidden()
            }
            if let failure = editor.failures[unitKey] {
                Label(failure, systemImage: FluxSymbol.failure)
                    .font(.caption)
                    .foregroundStyle(Color.fdStatusFailedText)
            }
        }
        .settingsRow(item.id, failureKey: item.key)
        .sensoryFeedback(FluxHaptic.warning.sensoryFeedback, trigger: warnTick)
        .onAppear { syncDraft() }
        .onChange(of: minutes) { _, _ in if !focused { syncDraft() } }
        .onChange(of: unit) { _, _ in if !focused { syncDraft() } }
        .onChange(of: focused) { _, isFocused in if !isFocused { commit() } }
        .onDisappear { commit() }
    }

    private var unitMenu: some View {
        Menu {
            Picker(L(unitTitleKey), selection: Binding(get: { unit }, set: { changeUnit($0) })) {
                ForEach(BtDurationUnit.allCases) { Text(L($0.titleKey)).tag($0) }
            }
        } label: {
            HStack(spacing: 4) {
                Text(L(unit.titleKey))
                Image(systemName: "chevron.up.chevron.down").font(.caption2)
            }
            .frame(minHeight: 44)
            .contentShape(.rect)
        }
        .accessibilityLabel(L(unitTitleKey))
        .accessibilityValue(L(unit.titleKey))
    }

    private func syncDraft() {
        draft = BtDuration.text(minutes: minutes, unit: unit)
    }

    private func commit() {
        guard let next = BtDuration.minutes(from: draft, unit: unit) else {
            warnTick += 1
            syncDraft()
            return
        }
        if next != minutes { editor.set(item.key, String(next)) }
        draft = BtDuration.text(minutes: next, unit: unit)
    }

    /// 保留输入框里的数字，换算成新单位下的分钟，并同时写回数值键与单位键。
    private func changeUnit(_ next: BtDurationUnit) {
        guard next != unit else { return }
        guard let converted = BtDuration.minutes(from: draft, unit: next) else {
            warnTick += 1
            return
        }
        focused = false
        if converted == minutes {
            editor.set(unitKey, next.rawValue)
        } else {
            editor.setNow([item.key: String(converted), unitKey: next.rawValue])
        }
        draft = BtDuration.text(minutes: converted, unit: next)
    }

    private func nudge(up: Bool) {
        let next = BtDuration.stepped(minutes: minutes, unit: unit, up: up)
        focused = false
        if next != minutes { editor.set(item.key, String(next)) }
        draft = BtDuration.text(minutes: next, unit: unit)
    }
}
#endif
