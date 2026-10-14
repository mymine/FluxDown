import FluxDomain
import Foundation
import Observation

// 诊断页的纯逻辑（过滤 / 文案键 / 报告文本）与设置首页读数缓存。

nonisolated enum DiagnosticsLogic {
    /// 桌面专属检查项（NMH、协议 / 文件关联、自启、系统通知、root 运行）：iOS 与 Web 一样隐藏（`parity.md` §2.16）。
    static let desktopOnlyChecks: Set<String> = [
        "nmh_binary", "nmh_manifest", "nmh_browser", "nmh_relay", "nmh_launch", "nmh_policy", "nmh_ownership",
        "nmh_comspec", "url_protocol", "torrent_association", "autostart", "notifications", "elevated_run",
    ]

    /// 需要桌面会话的修复动作：不提供按钮。
    static let desktopOnlyActions: Set<String> = [
        "reregister", "use_this_install", "register", "open_log_dir", "openLogDir", "test_notification",
        "fix_dir_access", "enable_autostart", "open_settings", "fix_comspec",
    ]

    /// `delete_files` → `DeleteFiles`（同 Web `camel`）：拼 `doctorCheck{Camel(id)}` 等文案键。
    static func camel(_ value: String) -> String {
        value.split(separator: "_", omittingEmptySubsequences: false)
            .map { $0.isEmpty ? "" : $0.prefix(1).uppercased() + $0.dropFirst() }
            .joined()
    }

    static func visibleChecks(_ checks: [DiagnosticCheckDto]) -> [DiagnosticCheckDto] {
        checks.filter { !desktopOnlyChecks.contains($0.id) }
    }

    /// 检查项上可用的修复：非桌面专属，且有动作文案（`doctorAction{Camel}`）——没有文案的动作不做成按钮。
    static func repair(for check: DiagnosticCheckDto, hasLabel: (String) -> Bool) -> DiagnosticRepairParams? {
        guard let repair = check.repair, !desktopOnlyActions.contains(repair.action),
              hasLabel(actionKey(repair.action)) else { return nil }
        return repair
    }

    static func checkKey(_ id: String) -> String { "doctorCheck" + camel(id) }
    static func hintKey(_ hint: String) -> String { "doctorHint" + camel(hint) }
    static func actionKey(_ action: String) -> String { "doctorAction" + camel(action) }

    static func levelKey(_ level: DiagnosticLevel) -> String {
        switch level {
        case .ok: "doctorLevelOk"
        case .warn: "doctorLevelWarn"
        case .error: "doctorLevelError"
        case .info, .unknown: "doctorLevelInfo"
        }
    }

    /// 修复失败的可操作说明键（同 GPUI `doctor::repair_outcome` / Web `REPAIR_REASON_KEYS`）；其余回退通用文案。
    static func repairErrorKey(reason: String?) -> String? {
        switch reason {
        case "elevationCancelled": "doctorRepairCancelled"
        case "elevationUnavailable": "doctorRepairUnavailable"
        case "runningElevated": "doctorRepairRunningElevated"
        case "repairIncomplete": "doctorRepairIncomplete"
        case "repairNotApplicable": "doctorRepairNotApplicable"
        default: nil
        }
    }

    static func issueCount(_ checks: [DiagnosticCheckDto]) -> Int {
        checks.filter { $0.level.isIssue }.count
    }

    // MARK: 报告文本

    nonisolated struct ReportLine: Equatable {
        var level: DiagnosticLevel
        var label: String
        var target: String
        var detail: String
        var hint: String
    }

    nonisolated struct HostPart: Equatable {
        var name: String
        var appVersion: String
        var platform: String
        var dataDir: String
        var daemonConnected: Bool
        /// `daemon.diagnostics.describe` 的一行摘要（版本 · 任务 / 队列 / 任务组 · 日志目录）；取不到为 nil。
        var daemonSummary: String?
        var lines: [ReportLine]
    }

    nonisolated struct ReportInput: Equatable {
        var appVersion: String
        var deviceDescription: String
        var generatedAt: Date
        var device: [ReportLine]
        var host: HostPart?
    }

    /// 纯文本报告（复制到反馈）：版本、平台、时间、数据目录、daemon 连接、逐项 `[LEVEL] 标题 (目标): 详情` + 提示。
    static func renderReport(_ input: ReportInput) -> String {
        var out = "FluxDown \(input.appVersion) · \(input.deviceDescription) · \(isoTime(input.generatedAt))\n"
        out += "\n[This device]\n"
        out += lines(input.device)
        if let host = input.host {
            out += "\n[Download host: \(host.name)]\n"
            out += "host: \(host.appVersion) · \(host.platform)\n"
            out += "agent data dir: \(host.dataDir)\n"
            out += "daemon connected: \(host.daemonConnected)\n"
            if let summary = host.daemonSummary { out += "daemon: \(summary)\n" }
            out += "\n" + lines(host.lines)
        }
        return out
    }

    private static func lines(_ items: [ReportLine]) -> String {
        var out = ""
        for item in items {
            out += "[\(item.level.reportTag)] \(item.label)"
            if !item.target.isEmpty { out += " (\(item.target))" }
            out += ": \(item.detail)\n"
            if !item.hint.isEmpty { out += "  hint: \(item.hint)\n" }
        }
        return out
    }

    private static func isoTime(_ date: Date) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.string(from: date)
    }
}

/// 设置首页「诊断」读数：最近一次运行的问题数（按主机；换主机即清空，由 `NotificationService` 的主机观察触发）。
@MainActor
@Observable
final class DiagnosticsSummary {
    static let shared = DiagnosticsSummary()

    private(set) var hostID: String?
    private(set) var issues: Int?

    private init() {}

    func record(hostID: String, issues: Int) {
        self.hostID = hostID
        self.issues = issues
    }

    /// 当前主机不是记录所属的主机时丢弃读数。
    func reset(ifHostIsNot current: String) {
        guard let hostID, hostID != current else { return }
        self.hostID = nil
        issues = nil
    }

    /// 首页副标题；从未运行 → nil（使用默认说明）。
    var readout: String? {
        guard let issues else { return nil }
        return issues == 0 ? L("doctorAllHealthy") : L("doctorIssuesFound", ["n": issues])
    }
}
