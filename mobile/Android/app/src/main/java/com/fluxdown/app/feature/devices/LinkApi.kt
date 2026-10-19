package com.fluxdown.app.feature.devices

import com.fluxdown.core.host.HostSession
import com.fluxdown.core.protocol.HostMethod
import com.fluxdown.core.protocol.LinkApproveParams
import com.fluxdown.core.protocol.LinkDeviceParams
import com.fluxdown.core.protocol.LinkDiscoveryParams
import com.fluxdown.core.protocol.LinkPairBegin
import com.fluxdown.core.protocol.LinkPairBeginParams
import com.fluxdown.core.protocol.LinkPairFinish
import com.fluxdown.core.protocol.LinkPairFinishParams
import com.fluxdown.core.protocol.LinkPairingCode
import com.fluxdown.core.protocol.LinkRenameParams
import com.fluxdown.core.protocol.callJson
import com.fluxdown.core.protocol.callUnit

/**
 * 局域网直连（`agent.link.*`）的类型化调用：对 `HostSession` 通用通道的薄封装（同 iOS `AgentAPI` 的 link 部分）。
 * 绑定创建时刻的会话；失败抛 `HostException`，文案用 `AccountText.error(.., AccountErrorContext.Pairing)`。
 * 已配对设备列表经 `LinkedDevicesChanged` 事件推送，这里不解码 refresh / rename 的返回值。
 */
internal class LinkApi(private val session: HostSession) {
    suspend fun pairingCode(): LinkPairingCode =
        LinkPairingCode.fromJson(session.callJson(HostMethod.agentLinkPairingCode))

    suspend fun stopPairing() = session.callUnit(HostMethod.agentLinkStopPairing)

    suspend fun discoverySet(enabled: Boolean) =
        session.callUnit(HostMethod.agentLinkDiscoverySet, LinkDiscoveryParams(enabled).toJson())

    suspend fun pairBegin(params: LinkPairBeginParams): LinkPairBegin =
        LinkPairBegin.fromJson(session.callJson(HostMethod.agentLinkPairBegin, params.toJson()))

    suspend fun pairFinish(token: String, accept: Boolean): LinkPairFinish =
        LinkPairFinish.fromJson(session.callJson(HostMethod.agentLinkPairFinish, LinkPairFinishParams(token, accept).toJson()))

    suspend fun approve(sessionId: String, accept: Boolean) =
        session.callUnit(HostMethod.agentLinkApprove, LinkApproveParams(sessionId, accept).toJson())

    suspend fun remove(fingerprint: String) =
        session.callUnit(HostMethod.agentLinkRemove, LinkDeviceParams(fingerprint).toJson())

    /** [name] 为 null = 清除备注名。 */
    suspend fun rename(fingerprint: String, name: String?) =
        session.callUnit(HostMethod.agentLinkRename, LinkRenameParams(fingerprint, name).toJson())

    /** 探测全部已配对设备在线状态（结果经 `LinkedDevicesChanged` 推送）。 */
    suspend fun refresh() = session.callUnit(HostMethod.agentLinkRefresh)
}
