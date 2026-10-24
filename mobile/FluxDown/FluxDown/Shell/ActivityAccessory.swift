import FluxDomain
import FluxUI
import SwiftUI

/// 全局活动条（02-downloads §2.2，底部附件）：上下行速度 + 活跃数 + 迷你波形 + 全部暂停 / 恢复；
/// 点按打开 D9 活动面板。玻璃由 `tabViewBottomAccessory` 提供，内容不再加玻璃。
struct ActivityAccessory: View {
    @Environment(AppContainer.self) private var container
    @Environment(TaskActions.self) private var actions

    var body: some View {
        let state = container.store.state
        let stats = state.stats
        let history = state.speedHistory
        let active = Int(stats.activeTasks)
        let down = Format.speedOrZero(stats.totalDownloadBps)
        let up = Format.speedOrZero(stats.totalUploadBps)
        let resumable = state.tasks.isAllPaused(active: active, pending: Int(stats.pendingTasks))
        ActivityBarContent(
            downText: active > 0 ? "↓ \(down)" : L("mobileIdleSummary"),
            upText: "↑ \(up)",
            activeCount: active,
            samples: history.downSamples.map(Double.init),
            isPaused: resumable,
            onTogglePause: {
                if resumable { actions.resumeAll() } else { actions.pauseAll() }
            },
            pauseLabel: L("pauseAll"),
            resumeLabel: L("resumeAll")
        )
        // 暂停 / 恢复按钮在内容里自带；整条点按打开面板，不嵌套 Button（VoiceOver 才能单独触达内部按钮）。
        .contentShape(.rect)
        .onTapGesture { container.router.sheet = .activity }
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { container.router.sheet = .activity }
        .disabled(state.connection != .live)
    }
}
