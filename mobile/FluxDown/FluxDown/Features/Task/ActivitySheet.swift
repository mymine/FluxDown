import FluxDomain
import FluxUI
import SwiftUI

/// D9 全局活动面板（02-downloads §10）：上下行大读数、60 秒双序列曲线、活跃 / 排队 / 重试、剩余空间、全部暂停 / 恢复。
///
/// 与 Android `ActivitySheet` 同范围：下载 / 上传限速编辑不在会话端口里（`HostSession` 只有 `patchConfig`，
/// 没有限速命令语义与偏好键约定），因此不提供限速行；「全部完成后提醒」「清除已完成」同理不提供。
/// 系统玻璃由 Sheet 自带（不设 `presentationBackground`）；内容层卡片为不透明分组底，不叠玻璃。
struct ActivitySheet: View {
    @Environment(AppContainer.self) private var container
    @Environment(TaskActions.self) private var actions
    @Environment(\.dismiss) private var dismiss
    @Environment(\.dynamicTypeSize) private var typeSize

    /// D9 默认高度（02-downloads §10.4）；AX 字号默认 `.large`。
    private static let mediumDetent = PresentationDetent.height(560)

    var body: some View {
        let state = container.store.state
        let readOnly = state.isReadOnly
        NavigationStack {
            List {
                if readOnly {
                    Section {
                        Banner(text: L("localServiceDisconnected"), tone: .warning, systemImage: FluxSymbol.offline)
                            .taskDetailBareRow()
                    }
                }
                Section {
                    ActivitySpeedHero(down: state.stats.totalDownloadBps, up: state.stats.totalUploadBps)
                        .taskDetailBareRow()
                        .padding(.horizontal, 20)
                }
                Section {
                    ActivityChart(history: state.speedHistory)
                }
                Section {
                    TaskStatTiles(items: [
                        TaskStatItem(label: L("mobileInstrumentActive"), value: String(state.stats.activeTasks)),
                        TaskStatItem(label: L("mobileActivityQueued"), value: String(state.stats.pendingTasks)),
                        TaskStatItem(label: L("mobileActivityRetry"), value: String(state.stats.retryPendingTasks)),
                    ])
                    .taskDetailBareRow()
                }
                Section {
                    KeyValueRow(
                        key: L("mobileInstrumentFree"),
                        value: state.stats.diskFreeBytes.map { Format.bytes(unsigned: $0).description } ?? Format.dash,
                        monospaced: true
                    )
                }
            }
            .listStyle(.insetGrouped)
            // 让 Sheet 的系统玻璃透出；行仍是不透明分组底（内容层不做玻璃）。
            .scrollContentBackground(.hidden)
            .redacted(reason: state.connection == .connecting ? .placeholder : [])
            .navigationTitle(L("mobileInstrumentLabel"))
            .navigationSubtitle(container.host.displayName)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(role: .close) { dismiss() } label: { Text(L("close")) }
                }
            }
            .toolbar {
                controls(running: state.stats.activeTasks + state.stats.pendingTasks > 0 || state.tasks.hasSeeding, readOnly: readOnly)
            }
        }
        .presentationDetents(typeSize.isAccessibilitySize ? [.large] : [Self.mediumDetent, .large])
        .presentationDragIndicator(.visible)
        // 可边看列表边操作：medium 档位及以下不拦截背后的交互。
        .presentationBackgroundInteraction(.enabled(upThrough: Self.mediumDetent))
    }

    /// 全部暂停 / 全部恢复：Sheet 自带的底部工具栏（系统渲染玻璃，默认样式）。没有运行 / 排队任务时「全部暂停」置灰；断连只读时全部置灰。
    @ToolbarContentBuilder
    private func controls(running: Bool, readOnly: Bool) -> some ToolbarContent {
        ToolbarItem(placement: .bottomBar) {
            Button {
                FluxHaptic.medium.play()
                actions.pauseAll()
            } label: {
                Label(L("pauseAll"), systemImage: FluxSymbol.pause)
            }
            .disabled(readOnly || !running)
        }
        ToolbarSpacer(.flexible, placement: .bottomBar)
        ToolbarItem(placement: .bottomBar) {
            Button {
                FluxHaptic.medium.play()
                actions.resumeAll()
            } label: {
                Label(L("resumeAll"), systemImage: FluxSymbol.resume)
            }
            .disabled(readOnly)
        }
    }
}

/// 速度英雄：左 ↓ 下载（强调色）· 右 ↑ 上传（绿）；圆体等宽数字（AX 下纵排）。
private struct ActivitySpeedHero: View {
    let down: Int64
    let up: Int64

    @Environment(\.dynamicTypeSize) private var typeSize
    @ScaledMetric(relativeTo: .largeTitle) private var points: CGFloat = 40

    var body: some View {
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 10))
            : AnyLayout(HStackLayout(alignment: .firstTextBaseline, spacing: 16))
        layout {
            reading(Format.speedOrZero(down), systemImage: "arrow.down", tint: down > 0 ? Color.accentColor : Color.primary)
            if !typeSize.isAccessibilitySize { Spacer(minLength: 0) }
            reading(Format.speedOrZero(up), systemImage: "arrow.up", tint: up > 0 ? Color.fdStatusSeedingText : Color.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: "\(L("mobileActivityDownload")) \(Format.speedOrZero(down).description), \(L("mobileActivityUpload")) \(Format.speedOrZero(up).description)"))
    }

    private func reading(_ measure: Measure, systemImage: String, tint: Color) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Image(systemName: systemImage)
                .font(.system(size: points * 0.5, weight: .semibold))
                .foregroundStyle(tint)
                .accessibilityHidden(true)
            Text(measure.value)
                .font(.system(size: points, weight: .bold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(tint)
                .lineLimit(1)
                .minimumScaleFactor(0.5)
                .contentTransition(.numericText())
                .fluxAnimation(.smooth, value: measure.value)
            Text(measure.unit)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }
}

/// 近 60 秒下载 + 上传双序列曲线与图例（图表自带音频图描述；图例是装饰，对 VoiceOver 隐藏）。
private struct ActivityChart: View {
    let history: SpeedHistory

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SpeedAreaChart(
                down: history.downSamples.map(Double.init),
                up: history.upSamples.map(Double.init),
                valueFormat: { Format.speedOrZero(Int64($0)).description },
                title: L("mobileInstrumentLabel"),
                downLabel: L("mobileActivityDownload"),
                upLabel: L("mobileActivityUpload"),
                timeLabel: "\(L("mobileSpeedAxisStart")) – \(L("mobileSpeedAxisNow"))"
            )
            .frame(height: 130)
            HStack(spacing: 18) {
                legend(L("mobileActivityDownload"), color: .accentColor)
                legend(L("mobileActivityUpload"), color: .fdStatusSeeding)
                Spacer(minLength: 0)
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
            .accessibilityHidden(true)
        }
        .padding(.vertical, 6)
    }

    private func legend(_ title: String, color: Color) -> some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 8, height: 8)
            Text(title)
        }
    }
}
