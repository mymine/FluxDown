package com.fluxdown.core.protocol

import java.net.URI
import java.net.URISyntaxException

/*
 * UI Gateway（API 服务，`agent.gateway.*`）DTO：镜像 `native/protocol/src/agent.rs` 的
 * `GatewayStatusDto` / `GatewayPatchParams`（同 iOS `Gateway.swift`、Web `protocol/agent.ts` + `params.ts`）。
 */

/** 网关运行状态；永远不携带 token 文本（用 `agent.gateway.revealToken` 获取）。 */
data class GatewayStatusDto(
    val takeoverEnabled: Boolean = false,
    val jsonrpcEnabled: Boolean = false,
    val apiEnabled: Boolean = false,
    val mcpEnabled: Boolean = false,
    val corsEnabled: Boolean = false,
    val userTokenConfigured: Boolean = false,
    /** 当前已验证可用的实际监听端口；修改失败时保持原值。 */
    val port: Int = DEFAULT_PORT,
    /** server 模式或环境固定监听地址时为 false。 */
    val portEditable: Boolean = true,
    /** 是否对局域网开放兼容 API；修改后下次 agent 启动生效。 */
    val lanEnabled: Boolean = false,
    /** 本次运行网关是否实际监听非回环地址（`lanEnabled` 是持久化配置，下次启动才生效）。 */
    val lanActive: Boolean = false,
) {
    companion object {
        /** 缺省端口（`default_gateway_port`）。 */
        const val DEFAULT_PORT = 17800

        /** `GatewayPatchParams.port` 的合法范围。 */
        val PORT_RANGE = 1024..65535

        /** 缺省字段按 serde 默认回退（`port` 缺省 17800、`portEditable` 缺省 true）。 */
        fun fromJson(v: JsonValue?): GatewayStatusDto = GatewayStatusDto(
            takeoverEnabled = v.bool("takeoverEnabled"),
            jsonrpcEnabled = v.bool("jsonrpcEnabled"),
            apiEnabled = v.bool("apiEnabled"),
            mcpEnabled = v.bool("mcpEnabled"),
            corsEnabled = v.bool("corsEnabled"),
            userTokenConfigured = v.bool("userTokenConfigured"),
            port = v.int("port", DEFAULT_PORT),
            portEditable = v.bool("portEditable", true),
            lanEnabled = v.bool("lanEnabled"),
            lanActive = v.bool("lanActive"),
        )
    }
}

/**
 * `agent.gateway.patch` 参数（原子）。`null` 字段省略 = 保持不变。
 *
 * `apiEnabled` / `mcpEnabled` 由关转开且 token 仍为空时主机自动生成 token；显式清空 token（且本次未
 * 开启上述开关）时主机同时关闭二者。
 */
data class GatewayPatchParams(
    val takeoverEnabled: Boolean? = null,
    val jsonrpcEnabled: Boolean? = null,
    val apiEnabled: Boolean? = null,
    val mcpEnabled: Boolean? = null,
    val corsEnabled: Boolean? = null,
    val lanEnabled: Boolean? = null,
    /** 1024…65535；验证新服务可用后立即切换，固定监听模式拒绝修改。 */
    val port: Int? = null,
    /** 空串 = 清除用户 token；省略 = 保持。 */
    val userToken: String? = null,
    /** true 生成新的随机 token（优先于 [userToken]）。 */
    val regenerateUserToken: Boolean? = null,
) {
    fun toJson(): JsonValue = jsonObjectOmitNulls(
        "takeoverEnabled" to takeoverEnabled,
        "jsonrpcEnabled" to jsonrpcEnabled,
        "apiEnabled" to apiEnabled,
        "mcpEnabled" to mcpEnabled,
        "corsEnabled" to corsEnabled,
        "lanEnabled" to lanEnabled,
        "port" to port,
        "userToken" to userToken,
        "regenerateUserToken" to regenerateUserToken,
    )
}

/** `agent.gateway.revealToken` 结果（未配置为空串）。 */
data class GatewayRevealTokenResult(val userToken: String = "") {
    companion object {
        fun fromJson(v: JsonValue?): GatewayRevealTokenResult = GatewayRevealTokenResult(v.str("userToken"))
    }
}

/** 网关功能开关（API 服务页「功能开关」分组；顺序同 GPUI / Web）。 */
enum class GatewayFeature {
    Takeover, Jsonrpc, Api, Mcp, Cors,
    ;

    fun isOn(status: GatewayStatusDto): Boolean = when (this) {
        Takeover -> status.takeoverEnabled
        Jsonrpc -> status.jsonrpcEnabled
        Api -> status.apiEnabled
        Mcp -> status.mcpEnabled
        Cors -> status.corsEnabled
    }

    /** 只改本开关的补丁。 */
    fun patch(on: Boolean): GatewayPatchParams = when (this) {
        Takeover -> GatewayPatchParams(takeoverEnabled = on)
        Jsonrpc -> GatewayPatchParams(jsonrpcEnabled = on)
        Api -> GatewayPatchParams(apiEnabled = on)
        Mcp -> GatewayPatchParams(mcpEnabled = on)
        Cors -> GatewayPatchParams(corsEnabled = on)
    }
}

/** 主机地址派生（`http(s)://host:port`；`ws(s)` 视同，路径 / 查询 / 用户信息丢弃）。 */
object GatewayAddress {
    /** 主机 HTTP 根（无尾斜杠）；不是 http(s) / ws(s) 或无主机为 null。 */
    fun base(endpoint: String): String? {
        val uri = parse(endpoint) ?: return null
        val scheme = when (uri.scheme?.lowercase()) {
            "ws", "http" -> "http"
            "wss", "https" -> "https"
            else -> return null
        }
        val host = uri.host?.takeIf { it.isNotEmpty() } ?: return null
        return if (uri.port >= 0) "$scheme://$host:${uri.port}" else "$scheme://$host"
    }

    /** 地址里显式写出的端口。 */
    fun port(endpoint: String): Int? = parse(endpoint)?.port?.takeIf { it >= 0 }

    private fun parse(endpoint: String): URI? = try {
        URI(endpoint.trim())
    } catch (_: URISyntaxException) {
        null
    }
}
