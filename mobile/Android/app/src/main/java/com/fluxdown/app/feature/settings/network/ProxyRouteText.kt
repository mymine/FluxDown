package com.fluxdown.app.feature.settings.network

import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import com.fluxdown.app.R
import com.fluxdown.app.i18n.str
import com.fluxdown.core.protocol.ProxyRouteResponse
import com.fluxdown.core.protocol.ProxyRuleIssue
import com.fluxdown.fluxui.theme.FluxTheme

/** 一行结果文案及其着色。 */
internal class RouteLine(val text: String, val color: Color)

/**
 * `daemon.config.proxyRoute` 结果的两行展示（四端一致）：
 * 第一行（命中 / 未命中规则；`loopback` / `unsupported` / `invalidUrl` 没有），第二行按 `reason`。
 * `reason == ""` 表示没有给出 url，不渲染。
 */
@Composable
internal fun proxyRouteLines(r: ProxyRouteResponse): List<RouteLine> {
    if (r.reason.isEmpty()) return emptyList()
    val c = FluxTheme.colors
    val out = ArrayList<RouteLine>(2)
    val m = r.matched
    if (m != null) {
        val index = m.index + 1
        val text = if (m.name.isEmpty()) {
            str(R.string.proxyRouteRuleHit, "index" to index)
        } else {
            str(R.string.proxyRouteRuleHitNamed, "index" to index, "name" to m.name)
        }
        out += RouteLine(text, c.accentHi)
    } else if (r.reason != "loopback" && r.reason != "unsupported" && r.reason != "invalidUrl") {
        out += RouteLine(str(R.string.proxyRouteNoRule), c.inkMuted)
    }
    val detail = when (r.reason) {
        "direct" -> str(R.string.proxyRouteDirect)
        "proxy" -> str(R.string.proxyRouteProxy, "endpoint" to r.endpoint)
        "systemUndetected" -> str(R.string.proxyRouteSystemUndetected)
        "manualIncomplete" -> str(R.string.proxyRouteManualIncomplete)
        "modeNone" -> str(R.string.proxyRouteModeNone)
        "bypass" -> str(R.string.proxyRouteBypass)
        "pac" -> str(R.string.proxyRouteModePac)
        "auto" -> str(R.string.proxyRouteModeAuto)
        "loopback" -> str(R.string.proxyRouteLoopback)
        "unsupported" -> str(R.string.proxyRouteUnsupported)
        "invalidUrl" -> str(R.string.proxyRouteInvalidUrl)
        else -> r.reason
    }
    val warn = r.reason == "systemUndetected" || r.reason == "manualIncomplete" || r.reason == "invalidUrl"
    out += RouteLine(detail, if (warn) c.amberText else c.inkMuted)
    return out
}

/** issue 文案（四端一致）；`regexTooLarge` 的 detail 为空，填 `1 MiB`。 */
@Composable
internal fun proxyIssueText(issue: ProxyRuleIssue): String = when (issue.code) {
    "emptyPattern" -> str(R.string.proxyRuleErrEmpty)
    "invalidDomain" -> str(R.string.proxyRuleErrDomain, "detail" to issue.detail)
    "invalidRegex" -> str(R.string.proxyRuleErrRegex, "detail" to issue.detail)
    "regexTooLarge" -> str(R.string.proxyRuleErrRegex, "detail" to "1 MiB")
    "tooLong" -> str(R.string.proxyRuleErrTooLong, "max" to issue.detail)
    "invalidProxy" -> str(R.string.proxyRuleErrProxy, "detail" to issue.detail)
    else -> str(R.string.proxyRuleErrGeneric, "detail" to issue.code)
}
