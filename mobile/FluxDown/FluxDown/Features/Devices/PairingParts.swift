import FluxDomain
import FluxUI
import SwiftUI

/// SAS 短认证串（V4 核对页 / V5 入站确认页共用）：3+3 分组，圆体等宽大数字，两组依次淡入放大（间隔 80 ms，减弱动态效果下直接显示）。
/// 字号随 Dynamic Type 缩放并封顶；放不下时两组纵排。VoiceOver 逐位朗读，数字之间空格分隔。
struct SasDigitsView: View {
    let sas: String
    let size: CGFloat

    @ScaledMetric private var points: CGFloat
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var revealed = 0

    init(sas: String, size: CGFloat) {
        self.sas = sas
        self.size = size
        _points = ScaledMetric(wrappedValue: size, relativeTo: .largeTitle)
    }

    var body: some View {
        let groups = LinkRules.groupSAS(sas).split(separator: " ").map(String.init)
        let font = Font.system(size: min(points, size * 1.5), weight: .semibold, design: .rounded)
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 20) { digitGroups(groups, font: font) }
            VStack(spacing: 8) { digitGroups(groups, font: font) }
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(L("localPairingSasTitle"))
        .accessibilityValue(sas.filter(\.isNumber).map(String.init).joined(separator: " "))
        .task(id: sas) { await reveal(count: groups.count) }
    }

    @ViewBuilder
    private func digitGroups(_ groups: [String], font: Font) -> some View {
        ForEach(Array(groups.enumerated()), id: \.offset) { index, group in
            Text(group)
                .font(font)
                .monospacedDigit()
                .tracking(2)
                .lineLimit(1)
                .minimumScaleFactor(0.6)
                .opacity(revealed > index ? 1 : 0)
                .scaleEffect(revealed > index ? 1 : 0.85)
        }
    }

    private func reveal(count: Int) async {
        guard count > 0 else { return }
        if reduceMotion {
            revealed = count
            return
        }
        revealed = 0
        for index in 1 ... count {
            do {
                try await Task.sleep(for: .milliseconds(80))
            } catch {
                return
            }
            withFluxAnimation(.smooth, reduceMotion: reduceMotion) { revealed = index }
        }
    }
}

// MARK: - 核对 / 完成 / 拒绝（添加设备与添加主机共用）

/// SAS 核对页：大数字 + 对端名；`finishing` 时追加「等待对方……」。文案由调用方给（添加设备 vs 添加主机措辞不同）。
struct PairingVerifySection: View {
    let verify: PairingModel.Verify
    let finishing: Bool
    let message: String?
    let waitingText: String
    let hint: String

    var body: some View {
        Section {
            VStack(spacing: 16) {
                Text(L("localPairingSasTitle"))
                    .font(.headline)
                SasDigitsView(sas: verify.sas, size: 48)
                Text(verify.peerName)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                if finishing {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text(waitingText)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
        } footer: {
            Text(hint)
        }
        if let message {
            Section {
                Banner(text: message, tone: .error, systemImage: FluxSymbol.failure, slim: true)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
            }
        }
    }
}

/// 核对页的底部操作（浮动功能控件，系统玻璃按钮）：核对中 = 拒绝 / 确认配对（提交中禁用，防重复点击）；
/// 等待对方确认 = 取消（取消会让 agent 中止对端的在途确认）。其它步骤不显示。
struct PairingVerifyBar: View {
    let model: PairingModel

    var body: some View {
        switch model.step {
        case .finishing:
            Button {
                model.cancelFinishing()
            } label: {
                Text(L("cancel"))
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.glass)
            .controlSize(.large)
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
        case .verify:
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 12) { rejectButton; confirmButton }
                VStack(spacing: 12) { confirmButton; rejectButton }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
        default:
            EmptyView()
        }
    }

    private var rejectButton: some View {
        Button(role: .destructive) {
            Task { await model.finish(accept: false) }
        } label: {
            Text(L("localPairingReject"))
                .frame(maxWidth: .infinity)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(.glass)
        .controlSize(.large)
        .disabled(model.submitting)
    }

    private var confirmButton: some View {
        Button {
            Task { await model.finish(accept: true) }
        } label: {
            Text(L("localPairingConfirm"))
                .frame(maxWidth: .infinity)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(.glassProminent)
        .controlSize(.large)
        .disabled(model.submitting)
    }
}

/// 配对完成页（大对勾 + 文案 + 关闭）。
struct PairingDoneSection: View {
    let text: String
    let close: () -> Void

    @ScaledMetric(relativeTo: .largeTitle) private var doneIcon: CGFloat = 56
    @State private var celebrate = false

    var body: some View {
        Section {
            VStack(spacing: 12) {
                Image(systemName: FluxSymbol.success)
                    .font(.system(size: doneIcon))
                    .foregroundStyle(Color.fdStatusSeeding)
                    .symbolEffect(.bounce, value: celebrate)
                    .accessibilityHidden(true)
                Text(text)
                    .font(.headline)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
            .onAppear { celebrate = true }
        }
        Section {
            Button(L("close"), action: close)
        }
    }
}

/// 对端拒绝页。
struct PairingRejectedSection: View {
    let back: () -> Void

    var body: some View {
        Section {
            Banner(text: L("errReasonPairingRejected"), tone: .warning, systemImage: "hand.raised.fill", slim: true)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
        }
        Section {
            Button(L("back"), action: back)
        }
    }
}

/// 发现列表的一行（Rust 发现 / NWBrowser 发现共用）：名称 + 「平台 · 地址」+ 已配对标签 / 选中勾 / 解析中转圈。
struct PairingPeerRow: View {
    let name: String
    let platform: String?
    /// 地址 / 补充说明（NWBrowser 条目在解析前没有地址）。
    var detail: String?
    var paired = false
    var selected = false
    var busy = false
    var pairedTag = L("localDevicePairedTag")

    var body: some View {
        HStack(spacing: 12) {
            GlyphTile(systemImage: DevicePresentation.symbol(platform: platform), tint: .secondary, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(name)
                    .font(.body)
                    .foregroundStyle(.primary)
                    .lineLimit(2)
                Text([DevicePresentation.label(platform: platform), detail].compactMap { $0 }.joined(separator: " · "))
                    .font(.footnote.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if busy {
                ProgressView()
            } else if paired {
                StatusBadge(text: pairedTag, tone: .neutral)
            } else if selected {
                Image(systemName: FluxSymbol.done)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.tint)
                    .accessibilityHidden(true)
            }
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
