import FluxDomain
import Foundation

// 站点规则的展示映射：`daemon.config.proxyRoute` 结果 → 两行文案（命中行 + 出口行）、校验 issue → 文案与落点、规则出口标签。
// 与 GPUI / Web / Android 的映射逐条一致（见 `assets/i18n` 的 `proxyRoute*` / `proxyRuleErr*` 键）。

/// 一行结果文案及其色调（iOS 的 `SettingsStatusLine.Tone` 没有 accent，命中行映射为 `.success`）。
nonisolated struct ProxyRouteLine: Equatable {
    var text: String
    var tone: SettingsStatusLine.Tone
}

/// 单条结果的两行：命中行（可无）+ 出口行。
nonisolated struct ProxyRouteLines: Equatable {
    var title: ProxyRouteLine?
    var detail: ProxyRouteLine
}

/// issue 在编辑器里的落点。
nonisolated enum ProxyRuleIssueTarget: Equatable {
    case name, pattern, proxy
    /// 对话框级错误（field 为空或其它字段）。
    case form
}

enum ProxyRouteText {
    /// `reason == ""` 表示没有给出 url：不渲染结果行。
    static func lines(_ response: ProxyRouteResponse) -> ProxyRouteLines? {
        guard !response.reason.isEmpty else { return nil }
        let reason = response.reason
        var title: ProxyRouteLine?
        if let matched = response.matched {
            let index = matched.index + 1
            let text = matched.name.isEmpty
                ? L("proxyRouteRuleHit", ["index": index])
                : L("proxyRouteRuleHitNamed", ["index": index, "name": matched.name])
            title = ProxyRouteLine(text: text, tone: .success)
        } else if !["loopback", "unsupported", "invalidUrl"].contains(reason) {
            title = ProxyRouteLine(text: L("proxyRouteNoRule"), tone: .neutral)
        }
        let detailText: String
        switch reason {
        case "direct": detailText = L("proxyRouteDirect")
        case "proxy": detailText = L("proxyRouteProxy", ["endpoint": response.endpoint])
        case "systemUndetected": detailText = L("proxyRouteSystemUndetected")
        case "manualIncomplete": detailText = L("proxyRouteManualIncomplete")
        case "modeNone": detailText = L("proxyRouteModeNone")
        case "bypass": detailText = L("proxyRouteBypass")
        case "pac": detailText = L("proxyRouteModePac")
        case "auto": detailText = L("proxyRouteModeAuto")
        case "loopback": detailText = L("proxyRouteLoopback")
        case "unsupported": detailText = L("proxyRouteUnsupported")
        case "invalidUrl": detailText = L("proxyRouteInvalidUrl")
        default: detailText = reason
        }
        let warn = ["systemUndetected", "manualIncomplete", "invalidUrl"].contains(reason)
        return ProxyRouteLines(title: title, detail: ProxyRouteLine(text: detailText, tone: warn ? .warning : .neutral))
    }

    static func issueText(_ issue: ProxyRuleIssue) -> String {
        switch issue.code {
        case "emptyPattern": L("proxyRuleErrEmpty")
        case "invalidDomain": L("proxyRuleErrDomain", ["detail": issue.detail])
        case "invalidRegex": L("proxyRuleErrRegex", ["detail": issue.detail])
        case "regexTooLarge": L("proxyRuleErrRegex", ["detail": "1 MiB"])
        case "tooLong": L("proxyRuleErrTooLong", ["max": issue.detail])
        case "invalidProxy": L("proxyRuleErrProxy", ["detail": issue.detail])
        default: L("proxyRuleErrGeneric", ["detail": issue.code])
        }
    }

    static func target(_ issue: ProxyRuleIssue) -> ProxyRuleIssueTarget {
        switch issue.field {
        case "name": .name
        case "pattern": .pattern
        case "proxy": .proxy
        default: .form
        }
    }

    /// 规则标题：名称，名称为空时用 pattern。
    static func displayName(_ rule: ProxyRule) -> String {
        rule.name.isEmpty ? rule.pattern : rule.name
    }

    /// 出口标签：`custom` 附带脱敏端点。
    static func actionLabel(_ rule: ProxyRule) -> String {
        switch rule.action {
        case ProxyRule.actionSystem: L("proxyRuleActionSystem")
        case ProxyRule.actionManual: L("proxyRuleActionManual")
        case ProxyRule.actionCustom: "\(L("proxyRuleActionCustom")) · \(ProxyRule.displayEndpoint(rule.proxy))"
        default: L("proxyRuleActionDirect")
        }
    }

    static func matchLabel(_ rule: ProxyRule, short: Bool = true) -> String {
        if rule.match == ProxyRule.matchRegex {
            return L(short ? "proxyRuleMatchRegexShort" : "proxyRuleMatchRegex")
        }
        return L("proxyRuleMatchDomain")
    }

    /// 手动字段是否完整（host 非空且 port 在 1…65535）。
    static func manualConfigured(_ form: SettingsConfigForm) -> Bool {
        let host = form.string("proxy_host").trimmingCharacters(in: .whitespacesAndNewlines)
        let port = Int(form.string("proxy_port").trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
        return !host.isEmpty && NetworkPortInput.range.contains(port)
    }

    /// 手动代理端点：`{type}://{host}:{port}`（host 含 `:` 时加方括号）；不完整时 nil。
    static func manualEndpoint(_ form: SettingsConfigForm) -> String? {
        guard manualConfigured(form) else { return nil }
        let host = form.string("proxy_host").trimmingCharacters(in: .whitespacesAndNewlines)
        let shown = host.contains(":") && !host.hasPrefix("[") ? "[\(host)]" : host
        let port = form.string("proxy_port").trimmingCharacters(in: .whitespacesAndNewlines)
        return "\(form.string("proxy_type", default: "http"))://\(shown):\(Int(port) ?? 0)"
    }

    /// 规则出口是否需要在行内加「未配置」提示。
    static func needsManualWarning(_ rule: ProxyRule, form: SettingsConfigForm) -> Bool {
        rule.action == ProxyRule.actionManual && !manualConfigured(form)
    }

    /// 出口选项的 hint 文案（`manual` 随表单显示端点或未配置提示）。
    static func actionHint(_ action: String, form: SettingsConfigForm) -> String {
        switch action {
        case ProxyRule.actionSystem:
            return L("proxyRuleActionSystemHint")
        case ProxyRule.actionManual:
            if let endpoint = manualEndpoint(form) { return L("proxyRuleActionManualHint", ["endpoint": endpoint]) }
            return L("proxyRuleActionManualMissing")
        case ProxyRule.actionCustom:
            return L("proxyRuleProxyUrlHint")
        default:
            return L("proxyRuleActionDirectHint")
        }
    }
}
