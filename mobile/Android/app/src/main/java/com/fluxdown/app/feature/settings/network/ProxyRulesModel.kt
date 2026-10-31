package com.fluxdown.app.feature.settings.network

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.fluxdown.app.AppContainer
import com.fluxdown.app.feature.settings.ContainerDaemonConfigPort
import com.fluxdown.core.host.HostException
import com.fluxdown.core.host.HostSession
import com.fluxdown.core.protocol.HostMethod
import com.fluxdown.core.protocol.ProxyRouteRequest
import com.fluxdown.core.protocol.ProxyRouteResponse
import com.fluxdown.core.protocol.ProxyRule
import com.fluxdown.core.protocol.ProxyRuleWriter
import com.fluxdown.core.protocol.callJson

/*
 * 站点代理规则的异步状态：规则写入（读-改-写 + 冲突重放）与 `daemon.config.proxyRoute`（慢 RPC）。
 * 匹配只在引擎里实现，这里只调用 RPC，不在本地匹配。
 */

@Stable
internal class ProxyRulesModel(container: AppContainer) {
    val writer = ProxyRuleWriter(ContainerDaemonConfigPort(container))

    /** 「测试网址」的状态；[stamp] = 发送时的 `proxy_mode|proxy_rules`，渲染时只在与当前值相同时显示。 */
    sealed interface Explain {
        data object Idle : Explain
        data object Running : Explain
        data class Done(val stamp: String, val response: ProxyRouteResponse) : Explain
        data class Failed(val stamp: String, val text: String) : Explain
    }

    var explain by mutableStateOf<Explain>(Explain.Idle)
        private set

    /** 每次新检测 +1：丢弃过期结果。 */
    private var generation = 0

    /**
     * 编辑器检测：发送 `{url, rules: encode(rules)}`。失败 / 试一试只传 `[draft]`，保存前传 `upsert(当前列表, draft)`。
     * 失败抛 [HostException]，由调用方显示在对话框底部。
     */
    suspend fun check(session: HostSession, rules: List<ProxyRule>, url: String): ProxyRouteResponse =
        ProxyRouteResponse.fromJson(
            session.callJson(HostMethod.daemonConfigProxyRoute, ProxyRouteRequest(url, ProxyRule.encodeList(rules)).toJson()),
        )

    /** 「测试网址」：[rulesRaw] 是界面渲染所用的 `proxy_rules` 原文，原样透传（乐观写入还没落到 daemon 时结果仍与界面一致）。 */
    suspend fun explain(
        session: HostSession,
        url: String,
        rulesRaw: String,
        stamp: String,
        errorText: (HostException) -> String,
    ) {
        val mine = ++generation
        explain = Explain.Running
        val next: Explain = try {
            Explain.Done(
                stamp,
                ProxyRouteResponse.fromJson(session.callJson(HostMethod.daemonConfigProxyRoute, ProxyRouteRequest(url, rulesRaw).toJson())),
            )
        } catch (e: HostException) {
            Explain.Failed(stamp, errorText(e))
        } catch (c: kotlinx.coroutines.CancellationException) {
            if (mine == generation) explain = Explain.Idle
            throw c
        }
        if (mine == generation) explain = next
    }

    /** 切换主机后旧结果作废。 */
    fun reset() {
        generation++
        explain = Explain.Idle
    }
}
