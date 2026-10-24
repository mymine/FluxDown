package com.fluxdown.core.protocol

import com.fluxdown.core.model.LinkDevice

// 局域网直连（L1，`agent.link.*`，无需登录）。镜像 `native/protocol/src/agent.rs`（`LinkPairingRequestDto` /
// `LinkPairingCodeDto` / 各 Params）与 `daemon.rs`（`LinkDiscoveredPeer` / `LinkPair*Response`）；
// 规则镜像 iOS `FluxDomain/Protocol/Link.swift`、Web `account/pairing.ts` 与 GPUI `crates/account/src/link.rs`。

/** `agent.linkDiscovered` 分区元素 / `agent.link.probe` 结果：被发现、尚未配对的设备。 */
data class LinkDiscoveredPeer(
    /** 对端指纹（经 `/ping` TOFU 获得；mDNS 未探测到时为 null）。 */
    val fingerprint: String?,
    val name: String,
    val platform: String?,
    val host: String,
    val port: Int,
    val appVersion: String?,
    /** `mdns` | `manual`。 */
    val source: String,
) {
    companion object {
        fun fromJson(v: JsonValue?): LinkDiscoveredPeer? {
            val host = v.strOrNull("host") ?: return null
            return LinkDiscoveredPeer(
                fingerprint = v.strOrNull("fingerprint")?.takeIf { it.isNotEmpty() },
                name = v.str("name"),
                platform = v.strOrNull("platform"),
                host = host,
                port = v.int("port"),
                appVersion = v.strOrNull("appVersion"),
                source = v.str("source", "mdns"),
            )
        }

        /** 分区值（数组）；缺失 / 损坏为空。 */
        fun listFromJson(v: JsonValue?): List<LinkDiscoveredPeer> = v.arrayOrNull.orEmpty().mapNotNull(::fromJson)
    }
}

/** `agent.linkPairingRequests` 分区元素：等待本机确认的入站配对请求（对端已输入本机配对码）。 */
data class LinkPairingRequest(
    val sessionId: String,
    val peerName: String,
    val peerFingerprint: String,
    val peerPlatform: String?,
    /** 双方肉眼核对的短认证串（6 位数字）。 */
    val sas: String,
    /** 本机时钟下的到期时刻（Unix 毫秒）：收到 DTO 的本机时刻 + `ttlMs`；旧主机无 `ttlMs` 才回退主机时钟的 `expiresAtUnixMs`。 */
    val expiresAtUnixMs: Long,
    /** 对端请求完全控制本机（批准 = 授予对端远程控制本机 FluxDown 的权限）。 */
    val requestsControl: Boolean = false,
) {
    companion object {
        /** [nowMs] = 收到 DTO 的本机时刻（相对 TTL 的起点）。 */
        fun fromJson(v: JsonValue?, nowMs: Long = System.currentTimeMillis()): LinkPairingRequest? {
            val sessionId = v.strOrNull("sessionId") ?: return null
            return LinkPairingRequest(
                sessionId = sessionId,
                peerName = v.str("peerName"),
                peerFingerprint = v.str("peerFingerprint"),
                peerPlatform = v.strOrNull("peerPlatform"),
                sas = v.str("sas"),
                expiresAtUnixMs = LinkRules.localDeadline(v.longOrNull("ttlMs"), v.long("expiresAtUnixMs"), nowMs),
                requestsControl = v.bool("requestsControl"),
            )
        }

        fun listFromJson(v: JsonValue?, nowMs: Long = System.currentTimeMillis()): List<LinkPairingRequest> =
            v.arrayOrNull.orEmpty().mapNotNull { fromJson(it, nowMs) }
    }
}

/** `agent.link.pairingCode` 结果：本机当前展示的配对码。 */
data class LinkPairingCode(
    val code: String,
    /** 本机时钟下的到期时刻（Unix 毫秒），算法同 [LinkPairingRequest.expiresAtUnixMs]。 */
    val expiresAtUnixMs: Long,
    /** 本机可被对端直连的地址（`http(s)://host:port`）；空 = 网关仅回环（局域网未开启）。 */
    val addresses: List<String>,
    val fingerprint: String,
    val deviceName: String,
) {
    companion object {
        fun fromJson(v: JsonValue?, nowMs: Long = System.currentTimeMillis()): LinkPairingCode = LinkPairingCode(
            code = v.str("code"),
            expiresAtUnixMs = LinkRules.localDeadline(v.longOrNull("ttlMs"), v.long("expiresAtUnixMs"), nowMs),
            addresses = v.strings("addresses"),
            fingerprint = v.str("fingerprint"),
            deviceName = v.str("deviceName"),
        )
    }
}

/** `agent.link.pairBegin` 结果：进入 SAS 核对。 */
data class LinkPairBegin(
    val token: String,
    /** 供双方肉眼核对的短认证串。 */
    val sas: String,
    val peerName: String,
    val peerFingerprint: String,
) {
    companion object {
        fun fromJson(v: JsonValue?): LinkPairBegin = LinkPairBegin(
            token = v.str("token"),
            sas = v.str("sas"),
            peerName = v.str("peerName"),
            peerFingerprint = v.str("peerFingerprint"),
        )
    }
}

/** `agent.link.pairFinish` 结果：`accept=false` 或对端拒绝时 [paired] = false，[deviceName] 为 null。 */
data class LinkPairFinish(val paired: Boolean, val deviceName: String?) {
    companion object {
        fun fromJson(v: JsonValue?): LinkPairFinish =
            LinkPairFinish(paired = v.bool("paired"), deviceName = v["device"].strOrNull("name"))
    }
}

data class LinkDiscoveryParams(val enabled: Boolean) {
    fun toJson(): JsonValue = jsonObject("enabled" to enabled)
}

/** `agent.link.probe` 参数：`address` 接受 `host`、`host:port`、`http(s)://host[:port][/base]`。 */
data class LinkAddressParams(val address: String) {
    fun toJson(): JsonValue = jsonObject("address" to address)
}

/** [requestControl] = 本次配对同时请求完全控制对端；false 时省略字段（旧主机 / 普通设备配对的 wire 与以前一致）。 */
data class LinkPairBeginParams(val address: String, val code: String, val requestControl: Boolean = false) {
    fun toJson(): JsonValue =
        if (requestControl) jsonObject("address" to address, "code" to code, "requestControl" to true) else jsonObject("address" to address, "code" to code)
}

data class LinkPairFinishParams(val token: String, val accept: Boolean) {
    fun toJson(): JsonValue = jsonObject("token" to token, "accept" to accept)
}

data class LinkApproveParams(val sessionId: String, val accept: Boolean) {
    fun toJson(): JsonValue = jsonObject("sessionId" to sessionId, "accept" to accept)
}

/** `agent.link.remove` 的设备定位参数。 */
data class LinkDeviceParams(val fingerprint: String) {
    fun toJson(): JsonValue = jsonObject("fingerprint" to fingerprint)
}

/** `agent.link.controlCredential` 结果：本机对某已配对主机的设备专属控制凭据（连接该主机 `/rpc` 的 Bearer）。 */
data class LinkControlCredential(val token: String) {
    companion object {
        fun fromJson(v: JsonValue?): LinkControlCredential = LinkControlCredential(token = v.str("token"))
    }
}

/** `agent.link.rename` 参数：[name] 为 null = 清除备注名（恢复对方自报名）。 */
data class LinkRenameParams(val fingerprint: String, val name: String?) {
    fun toJson(): JsonValue = jsonObjectOmitNulls("fingerprint" to fingerprint, "name" to name)
}

/** 本机直连服务的局域网可达状态：决定「本机配对码」区三态（契约「入站 / 发起配对 UI 通用要求」）。 */
enum class LinkLan {
    /** 局域网访问未开启：隐藏配对码，只给「前往设置」。 */
    Disabled,

    /** 已开启但本次运行尚未生效：隐藏配对码，提示重启。 */
    RestartRequired,

    /** 网关实际监听非回环地址：正常显示码 + 地址。 */
    Active,
    ;

    companion object {
        /**
         * 由 `agent.gateway` 分区判定；分区缺失（主机没有网关能力 / 尚未收到）按 [Active] 处理——
         * 远端主机 / server 模式 `lanActive` 恒为 true，由 agent 负责。
         */
        fun of(gateway: GatewayStatusDto?): LinkLan = when {
            gateway == null || gateway.lanActive -> Active
            gateway.lanEnabled -> RestartRequired
            else -> Disabled
        }
    }
}

/** 直连配对的纯规则。 */
object LinkRules {
    const val PAIRING_CODE_LENGTH = 6

    /** 「重新搜索」/ 开启发现后，列表为空时仍显示「正在搜索」的窗口。 */
    const val SEARCH_WINDOW_MS = 8_000L

    /** 备注名长度上限（字符数）。 */
    const val NAME_MAX_LEN = 64

    /** 发现的对端 → `agent.link.pairBegin` 接受的 `host:port`（IPv6 加方括号）。 */
    fun address(peer: LinkDiscoveredPeer): String {
        val host = if (':' in peer.host && !peer.host.startsWith("[")) "[${peer.host}]" else peer.host
        return "$host:${peer.port}"
    }

    /** 配对码只保留 ASCII 数字并截断到 6 位。 */
    fun normalizePairingCode(raw: String): String = raw.filter { it in '0'..'9' }.take(PAIRING_CODE_LENGTH)

    fun isPairingCodeComplete(code: String): Boolean = code.length == PAIRING_CODE_LENGTH && code.all { it in '0'..'9' }

    /** 手动地址只做非空 / 无空白检查：`host`、`host:port`、`http(s)://…` 的解析与端口校验由 agent 负责。 */
    fun isManualAddressValid(address: String): Boolean {
        val text = address.trim()
        return text.isNotEmpty() && text.none { it.isWhitespace() }
    }

    /** 手动输入无法提交的原因。 */
    enum class InputError { AddressMissing, CodeIncomplete }

    sealed interface Validation {
        data class Ok(val params: LinkPairBeginParams) : Validation

        data class Failed(val error: InputError) : Validation
    }

    /** 校验并规整 `pairBegin` 输入：地址原样交给 agent 解析，配对码去掉空白 / 连字符后必须是 6 位数字。 */
    fun validate(address: String, code: String, requestControl: Boolean = false): Validation {
        val trimmed = address.trim()
        if (!isManualAddressValid(trimmed)) return Validation.Failed(InputError.AddressMissing)
        val digits = code.filter { !it.isWhitespace() && it != '-' }
        if (!isPairingCodeComplete(digits)) return Validation.Failed(InputError.CodeIncomplete)
        return Validation.Ok(LinkPairBeginParams(trimmed, digits, requestControl))
    }

    /** 发现列表里的一台设备（已去重、标注是否已配对）。 */
    data class DiscoveredEntry(
        val name: String,
        val platform: String?,
        /** 交给 `pairBegin` / `probe` 的地址。 */
        val address: String,
        /** 已与本机配对（按指纹判断，对方已解除的记录不算），不可再选。 */
        val paired: Boolean,
        /** 对端指纹（mDNS 未探测到时为 null）。 */
        val fingerprint: String? = null,
        /** 已配对且本机可控制它：添加主机时可跳过配对，直接取控制凭据连接。 */
        val controllable: Boolean = false,
    )

    /** 发现快照 → 列表：已配对标记、同一地址只保留一条、按名称（不区分大小写）排序。 */
    fun discoveredEntries(peers: List<LinkDiscoveredPeer>, paired: List<LinkDevice>): List<DiscoveredEntry> {
        val live = paired.filter { !it.revoked }
        val known = live.mapTo(HashSet()) { it.fingerprint }
        val controllable = live.filter { it.controllable }.mapTo(HashSet()) { it.fingerprint }
        val seen = HashSet<String>()
        val entries = ArrayList<DiscoveredEntry>()
        for (peer in peers) {
            val hostPort = address(peer)
            if (!seen.add(hostPort)) continue
            entries += DiscoveredEntry(
                name = peer.name.trim().ifEmpty { peer.host },
                platform = peer.platform,
                address = hostPort,
                paired = peer.fingerprint?.let { it in known } ?: false,
                fingerprint = peer.fingerprint,
                controllable = peer.fingerprint?.let { it in controllable } ?: false,
            )
        }
        return entries.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name })
    }

    /** 添加主机的发现列表：同 [discoveredEntries]，但可直接连接的已配对主机（[DiscoveredEntry.controllable]）排在最前。 */
    fun hostEntries(peers: List<LinkDiscoveredPeer>, paired: List<LinkDevice>): List<DiscoveredEntry> =
        discoveredEntries(peers, paired).sortedByDescending { it.controllable }

    /** 以「请求控制」发起配对时对端不支持：配对协议版本不符 / 对端过旧（无 `linkControl` 能力）→ 提示更新主机或改用访问密钥。 */
    fun isControlUnsupported(reason: String?): Boolean = reason == "pairingVersionMismatch" || reason == "peerOutdated"

    /**
     * 相对 TTL → 本机到期时刻：[ttlMs]（主机生成 DTO 时的剩余毫秒）从本机收到 DTO 的 [nowMs] 起算；
     * 缺失 / 0（旧主机的 serde 缺省值；真到期的 DTO 本就马上被清理）才回退主机时钟的 [wireExpiresAtUnixMs]。
     */
    fun localDeadline(ttlMs: Long?, wireExpiresAtUnixMs: Long, nowMs: Long): Long =
        if (ttlMs != null && ttlMs > 0) nowMs + ttlMs else wireExpiresAtUnixMs

    /** 配对码 / 入站请求距离过期还剩多少秒（向上取整，已过期为 0）。 */
    fun secondsUntil(expiresAtUnixMs: Long, nowMs: Long): Long {
        val ms = expiresAtUnixMs - nowMs
        return if (ms <= 0) 0 else (ms + 999) / 1000
    }

    /** 短认证串按 3 位一组展示（`123456` → `123 456`）。 */
    fun groupSas(sas: String): String = sas.filter { !it.isWhitespace() }.chunked(3).joinToString(" ")

    /** 配对码按 3-3 分组展示。 */
    fun groupCode(code: String): String = groupSas(code)

    /** `LinkDevice.pairedAt / lastSeenAt` 时间戳 → Unix 毫秒：量级 < 1e12 视为秒；≤ 0 = null。 */
    fun unixMillis(value: Long): Long? = when {
        value <= 0 -> null
        value < 1_000_000_000_000L -> value * 1000
        else -> value
    }

    /** 入站请求排序：先到期的在前（最紧迫先处理），到期相同保持原顺序。 */
    fun queue(requests: List<LinkPairingRequest>): List<LinkPairingRequest> = requests.sortedBy { it.expiresAtUnixMs }

    /** 备注名输入的校验结果（`agent.link.rename`）。 */
    sealed interface RenameCheck {
        /** 空白：清除备注名。 */
        data object Clear : RenameCheck

        /** 合法（已 trim，1–[NAME_MAX_LEN] 字符、无控制字符）。 */
        data class Name(val name: String) : RenameCheck

        data object Invalid : RenameCheck
    }

    fun checkRename(input: String): RenameCheck {
        val name = input.trim()
        if (name.isEmpty()) return RenameCheck.Clear
        val length = name.codePointCount(0, name.length)
        val control = name.any { it.isISOControl() }
        return if (length in 1..NAME_MAX_LEN && !control) RenameCheck.Name(name) else RenameCheck.Invalid
    }
}
