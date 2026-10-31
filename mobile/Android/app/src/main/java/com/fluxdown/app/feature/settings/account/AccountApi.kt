package com.fluxdown.app.feature.settings.account

import android.content.Context
import androidx.annotation.StringRes
import com.fluxdown.app.AppContainer
import com.fluxdown.core.host.HostException
import com.fluxdown.core.host.HostSession
import com.fluxdown.core.protocol.AccountErrorContext
import com.fluxdown.core.protocol.AgentLoginResult
import com.fluxdown.core.protocol.ChangeEmailParams
import com.fluxdown.core.protocol.ChangeNicknameParams
import com.fluxdown.core.protocol.ChangeOriginIdParams
import com.fluxdown.core.protocol.ChangePasswordParams
import com.fluxdown.core.protocol.CheckOriginIdParams
import com.fluxdown.core.protocol.CloudEndpointDto
import com.fluxdown.core.protocol.CloudEndpointSetParams
import com.fluxdown.core.protocol.HostMethod
import com.fluxdown.core.protocol.LoginParams
import com.fluxdown.core.protocol.LoginVerifyParams
import com.fluxdown.core.protocol.OriginIdCheckResult
import com.fluxdown.core.protocol.RandomOriginIdResult
import com.fluxdown.core.protocol.RegisterParams
import com.fluxdown.core.protocol.RegisterVerifyParams
import com.fluxdown.core.protocol.ResetPasswordParams
import com.fluxdown.core.protocol.SendCodeParams
import com.fluxdown.core.protocol.SendNewEmailCodeParams
import com.fluxdown.core.protocol.SessionDeviceDto
import com.fluxdown.core.protocol.SyncLocalOnlyParams
import com.fluxdown.core.protocol.TtlResult
import com.fluxdown.core.protocol.VerifyCodeParams
import com.fluxdown.core.protocol.callJson
import com.fluxdown.core.protocol.callUnit
import com.fluxdown.core.protocol.jsonObject

/**
 * 账户 / 同步 / 云连接的类型化 agent 调用：对 `HostSession` 通用通道的薄封装（同 iOS `AgentAPI`）。
 * 每个实例绑定创建时刻的会话快照；调用方每次操作都经 [AccountEnv.api] 取新实例，因此切换主机后的新调用
 * 自然落到新主机，旧会话上发起的多步流程不会悄悄续用到新主机上。
 * 失败抛 [HostException]；文案用 [AccountText.error]。
 */
internal class AccountApi(private val session: HostSession) {
    // ── 认证 ──

    suspend fun register(params: RegisterParams): AgentLoginResult =
        AgentLoginResult.fromJson(session.callJson(HostMethod.agentAuthRegister, params.toJson()))

    suspend fun registerVerify(params: RegisterVerifyParams): AgentLoginResult =
        AgentLoginResult.fromJson(session.callJson(HostMethod.agentAuthRegisterVerify, params.toJson()))

    suspend fun login(params: LoginParams): AgentLoginResult =
        AgentLoginResult.fromJson(session.callJson(HostMethod.agentAuthLogin, params.toJson()))

    suspend fun loginVerify(params: LoginVerifyParams): AgentLoginResult =
        AgentLoginResult.fromJson(session.callJson(HostMethod.agentAuthLoginVerify, params.toJson()))

    suspend fun sendCode(email: String): TtlResult =
        TtlResult.fromJson(session.callJson(HostMethod.agentAuthSendCode, SendCodeParams(email).toJson()))

    suspend fun verifyCode(params: VerifyCodeParams): AgentLoginResult =
        AgentLoginResult.fromJson(session.callJson(HostMethod.agentAuthVerifyCode, params.toJson()))

    suspend fun logout() = session.callUnit(HostMethod.agentAuthLogout)

    /** 刷新云端资料（结果经 `agent.session` 分区推送，这里不解码返回值）。 */
    suspend fun refreshProfile() = session.callUnit(HostMethod.agentAuthRefreshProfile)

    suspend fun sendPasswordResetCode(email: String): TtlResult =
        TtlResult.fromJson(session.callJson(HostMethod.agentAuthSendPasswordResetCode, SendCodeParams(email).toJson()))

    suspend fun resetPassword(params: ResetPasswordParams) =
        session.callUnit(HostMethod.agentAuthResetPassword, params.toJson())

    // ── 资料 ──

    suspend fun sendEmailCode(): TtlResult = TtlResult.fromJson(session.callJson(HostMethod.agentProfileSendEmailCode))

    suspend fun sendNewEmailCode(params: SendNewEmailCodeParams): TtlResult =
        TtlResult.fromJson(session.callJson(HostMethod.agentProfileSendNewEmailCode, params.toJson()))

    suspend fun changeEmail(params: ChangeEmailParams) = session.callUnit(HostMethod.agentProfileChangeEmail, params.toJson())

    suspend fun randomOriginId(): RandomOriginIdResult =
        RandomOriginIdResult.fromJson(session.callJson(HostMethod.agentProfileRandomOriginId))

    suspend fun checkOriginId(value: Long): OriginIdCheckResult =
        OriginIdCheckResult.fromJson(session.callJson(HostMethod.agentProfileCheckOriginId, CheckOriginIdParams(value).toJson()))

    suspend fun changeOriginId(originId: Long) =
        session.callUnit(HostMethod.agentProfileChangeOriginId, ChangeOriginIdParams(originId).toJson())

    suspend fun changeNickname(nickname: String) =
        session.callUnit(HostMethod.agentProfileChangeNickname, ChangeNicknameParams(nickname).toJson())

    suspend fun sendPasswordCode(): TtlResult = TtlResult.fromJson(session.callJson(HostMethod.agentProfileSendPasswordCode))

    suspend fun changePassword(params: ChangePasswordParams) =
        session.callUnit(HostMethod.agentProfileChangePassword, params.toJson())

    // ── 服务地址（仅调试构建 `editable == true`） ──

    suspend fun cloudEndpoint(): CloudEndpointDto = CloudEndpointDto.fromJson(session.callJson(HostMethod.agentCloudEndpointGet))

    /** [baseUrl] 为空 = 恢复默认地址。 */
    suspend fun setCloudEndpoint(baseUrl: String) =
        session.callUnit(HostMethod.agentCloudEndpointSet, CloudEndpointSetParams(baseUrl).toJson())

    // ── 配置同步 ──

    suspend fun syncEnable() = session.callUnit(HostMethod.agentSyncEnable)

    suspend fun syncDisable() = session.callUnit(HostMethod.agentSyncDisable)

    suspend fun syncNow() = session.callUnit(HostMethod.agentSyncNow)

    suspend fun syncSetLocalOnly(params: SyncLocalOnlyParams) =
        session.callUnit(HostMethod.agentSyncSetLocalOnly, params.toJson())

    // ── 云端设备 / 连接 ──

    /** 云端设备完整记录（含删除所需的云端行 id）；同时刷新快照里的云端设备名册。 */
    suspend fun deviceList(): List<SessionDeviceDto> =
        SessionDeviceDto.listFromJson(session.callJson(HostMethod.agentDeviceList))

    /** 删除受信任设备（[id] = 云端行 id，不是 deviceId）；删除本机时 agent 随即清除本机会话。 */
    suspend fun deviceDelete(id: String) = session.callUnit(HostMethod.agentDeviceDelete, jsonObject("id" to id))

    /** 请求立即重连云端任务连接；`accepted` 不表示已连上。 */
    suspend fun remoteReconnect() = session.callUnit(HostMethod.agentRemoteReconnect)
}

/**
 * 账户流程的运行环境：当前会话（每次调用取最新）、错误文案、字符串、时钟。
 * 状态机只依赖它，不碰 Compose / Android 细节。
 */
internal class AccountEnv(private val container: AppContainer, private val context: Context) {
    /** 当前主机会话上的调用封装（调用时刻快照）。 */
    fun api(): AccountApi = AccountApi(container.session)

    fun error(e: HostException, scene: AccountErrorContext = AccountErrorContext.General): String =
        AccountText.error(context, e, scene)

    fun text(@StringRes id: Int): String = context.getString(id)

    fun nowMs(): Long = System.currentTimeMillis()

    /** 验证码记录的作用域：主机 + 账号，切换主机 / 账号后不会串用旧倒计时。 */
    fun scope(userId: String): String = "${container.host.value.id}:$userId"
}
