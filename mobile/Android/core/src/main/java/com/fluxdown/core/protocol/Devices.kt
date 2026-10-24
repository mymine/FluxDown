package com.fluxdown.core.protocol

import com.fluxdown.core.model.CloudDevice
import com.fluxdown.core.model.LinkDevice

// 远程下发（新建下载「下载到」）。镜像 iOS `FluxDomain/Protocol/Devices.swift`、Web `pages/downloads/dialogs/target.ts`
// 与 GPUI `crates/downloads/src/model/{devices,dispatch}.rs`。

/** 设备本地文件路径的书写风格，决定远程下发时保存目录的合法形态；未知值原样保留。 */
sealed interface PathStyle {
    val wire: String

    /** `C:\dir` / `\\server\share`。 */
    data object Windows : PathStyle {
        override val wire = "windows"
    }

    /** `/dir`。 */
    data object Posix : PathStyle {
        override val wire = "posix"
    }

    /** 对端发送了本端不认识的风格。 */
    data class Unknown(override val wire: String) : PathStyle

    /** [path] 是否为该风格下的绝对路径（[Unknown] 恒 false）。 */
    fun isAbsolute(path: String): Boolean {
        val p = path.trim()
        return when (this) {
            Windows -> {
                val drive = p.length >= 3 && p[0].isAsciiLetter() && p[1] == ':' && (p[2] == '\\' || p[2] == '/')
                drive || p.startsWith("\\\\")
            }
            Posix -> p.startsWith("/")
            is Unknown -> false
        }
    }

    companion object {
        fun fromWire(wire: String): PathStyle = when (wire) {
            "windows" -> Windows
            "posix" -> Posix
            else -> Unknown(wire)
        }

        /** 按设备平台名推断（`windows` / `macos` / `linux` / `android` / `ios` …）；未知平台 null。 */
        fun fromPlatform(platform: String): PathStyle? = when (platform.trim().lowercase()) {
            "windows", "win32" -> Windows
            "macos", "darwin", "linux", "android", "ios", "freebsd", "openbsd", "netbsd" -> Posix
            else -> null
        }

        /** 设备自报的路径风格（wire 名）；缺失 / 不认识时按平台推断。 */
        fun effective(reported: String?, platform: String?): PathStyle? =
            when (val style = reported?.let(::fromWire)) {
                Windows, Posix -> style
                else -> platform?.let(::fromPlatform)
            }

        /** 校验失败提示里的路径示例。 */
        fun example(style: PathStyle?): String = if (style == Windows) "D:\\Downloads" else "/home/user/Downloads"
    }
}

private fun Char.isAsciiLetter(): Boolean = this in 'a'..'z' || this in 'A'..'Z'

/**
 * 远程下发目标：云账号其他设备（经 FluxCloud）、局域网已配对设备（直连），或二者合并的同一台机器
 * （[linkFingerprint] 非空：云条目挂接了同机的直连记录）。
 */
data class DispatchTarget(
    val kind: Kind,
    /** 云设备 `deviceId`（[RemoteDispatchParams.toDevice]）/ 独立直连设备指纹（[LinkDispatchParams.fingerprint]）。 */
    val deviceId: String,
    val name: String,
    /** 在线状态；null = 未知（云端 presence 不可信 / 本地服务未就绪）。挂接条目 = 云 / 直连任一在线。 */
    val online: Boolean?,
    /** 目标自报的默认下载目录（已去首尾空白；空 = null）。 */
    val defaultSaveDir: String?,
    /** 目标的有效路径风格（自报 → 平台推断）；未知为 null。 */
    val pathStyle: PathStyle?,
    /** 云条目挂接的直连记录指纹；独立条目恒为 null。 */
    val linkFingerprint: String? = null,
    /** 挂接直连记录的在线状态；null = 未知（本地服务未就绪）。 */
    val linkOnline: Boolean? = null,
) {
    enum class Kind { Cloud, Link }

    /** 选择键：`cloud:<deviceId>` / `link:<fingerprint>`（同 GPUI `DispatchTarget::to_pref`）；挂接条目恒为 `cloud:<id>`。 */
    val id: String get() = (if (kind == Kind.Cloud) "cloud:" else "link:") + deviceId

    val isCloud: Boolean get() = kind == Kind.Cloud

    /** 下发路由：独立直连条目 / 挂接且直连在线的云条目走 `agent.link.dispatch(fp)`，其余走云 `agent.remote.dispatch`。 */
    val linkRouteFingerprint: String?
        get() = when {
            kind == Kind.Link -> deviceId
            linkFingerprint != null && linkOnline == true -> linkFingerprint
            else -> null
        }

    /** 本次下发走局域网直连（一次性投递，提示用 `linkDispatchedToDevice`）。 */
    val routesViaLink: Boolean get() = linkRouteFingerprint != null

    /** 「下载到」菜单里带「直连」标签：仅独立直连条目（挂接条目不带）。 */
    val isLinkOnly: Boolean get() = kind == Kind.Link

    /** 已知离线：云设备由云端排队、上线后执行；局域网设备直连送不到。 */
    val isOffline: Boolean get() = online == false
}

/** 下发目标的纯规则。 */
object DeviceRules {
    /** 远端保存目录输入的校验结果（`dispatch.rs::check_remote_save_dir`）。 */
    sealed interface SaveDirCheck {
        /** 输入为空：提交时不带 `saveDir`，目标设备用它自己的默认目录。 */
        data object UseDefault : SaveDirCheck

        /** 合法目录（已去首尾空白）。 */
        data class Explicit(val dir: String) : SaveDirCheck

        /** 不是目标路径风格下的绝对路径。 */
        data object Invalid : SaveDirCheck
    }

    /** 风格未知（Web 端 / 新平台）时无法判断，交给目标设备回退默认目录，只要求非空即放行。 */
    fun checkSaveDir(input: String, style: PathStyle?): SaveDirCheck {
        val dir = input.trim()
        return when {
            dir.isEmpty() -> SaveDirCheck.UseDefault
            style == null || style is PathStyle.Unknown || style.isAbsolute(dir) -> SaveDirCheck.Explicit(dir)
            else -> SaveDirCheck.Invalid
        }
    }

    /**
     * 设备页云设备排序（iOS `DeviceRules.sorted`）：本机在前 → 在线 → 其余，同组按名称（忽略大小写）；稳定。
     * [presenceKnown] = false 时在线状态不可信，不按在线分组（未知不当离线沉底）。
     */
    fun sorted(devices: List<CloudDevice>, presenceKnown: Boolean): List<CloudDevice> =
        devices.sortedWith(
            compareByDescending<CloudDevice> { it.isCurrent }
                .thenByDescending { presenceKnown && it.isOnline }
                .thenBy(String.CASE_INSENSITIVE_ORDER) { it.name },
        )

    /**
     * 「下载到」候选（`crates/downloads/src/model/devices.rs::other_devices`），按「其他设备」合并规则：
     *
     * 1. 云条目：去掉本机与空 id，按 id 去重；
     * 2. 直连记录：去掉空指纹与 [LinkDevice.revoked]，按指纹去重；[LinkDevice.cloudDeviceId] 命中尚未挂接的云条目时
     *    **挂接**到该条目（同一台机器一条），否则成为独立直连条目；
     * 3. 挂接条目：名称取云名称，在线 = 任一为 true → true、否则任一未知 → 未知、否则 false，
     *    默认目录 / 路径风格云优先、缺失用直连的；
     * 4. 顺序：云条目（含挂接）按云名册顺序，随后独立直连条目；名称为空用短码，多台同名（忽略大小写与首尾空白）
     *    时全部追加 ` · 短码`（在合并后的整表上做，短码取云 id / 指纹）。
     *
     * - [cloudPresenceKnown]：云端在线状态可信（[CloudPresence.isKnown]），否则云设备在线未知；
     * - [localReady]：本地服务已就绪，否则直连在线未知。
     */
    fun dispatchTargets(
        cloud: List<CloudDevice>,
        link: List<LinkDevice>,
        cloudPresenceKnown: Boolean,
        localReady: Boolean,
    ): List<DispatchTarget> {
        val seenCloud = HashSet<String>()
        val cloudTargets = ArrayList<DispatchTarget>(cloud.size)
        val cloudIndex = HashMap<String, Int>()
        for (d in cloud) {
            if (d.isCurrent || d.deviceId.isEmpty() || !seenCloud.add(d.deviceId)) continue
            cloudIndex[d.deviceId] = cloudTargets.size
            cloudTargets += DispatchTarget(
                kind = DispatchTarget.Kind.Cloud,
                deviceId = d.deviceId,
                name = d.name.trim(),
                online = if (cloudPresenceKnown && localReady) d.isOnline else null,
                defaultSaveDir = d.defaultSaveDir?.trim()?.takeIf { it.isNotEmpty() },
                pathStyle = PathStyle.effective(d.pathStyle, d.platform),
            )
        }
        val seenLink = HashSet<String>()
        val standalone = ArrayList<DispatchTarget>()
        for (d in link) {
            if (d.fingerprint.isEmpty() || d.revoked || !seenLink.add(d.fingerprint)) continue
            val linkOnline = if (localReady) d.online else null
            val saveDir = d.defaultSaveDir?.trim()?.takeIf { it.isNotEmpty() }
            val style = PathStyle.effective(d.pathStyle, d.platform)
            val slot = d.cloudDeviceId?.takeIf { it.isNotEmpty() }?.let { cloudIndex[it] }
            val host = slot?.let { cloudTargets[it] }
            if (slot != null && host != null && host.linkFingerprint == null) {
                cloudTargets[slot] = host.copy(
                    online = mergeOnline(host.online, linkOnline),
                    defaultSaveDir = host.defaultSaveDir ?: saveDir,
                    pathStyle = host.pathStyle ?: style,
                    linkFingerprint = d.fingerprint,
                    linkOnline = linkOnline,
                )
            } else {
                standalone += DispatchTarget(
                    kind = DispatchTarget.Kind.Link,
                    deviceId = d.fingerprint,
                    name = d.name.trim(),
                    online = linkOnline,
                    defaultSaveDir = saveDir,
                    pathStyle = style,
                )
            }
        }
        val targets = cloudTargets + standalone
        val counts = targets.groupingBy { it.name.lowercase() }.eachCount()
        return targets.map { t ->
            when {
                t.name.isEmpty() -> t.copy(name = shortCode(t.deviceId))
                (counts[t.name.lowercase()] ?: 0) > 1 -> t.copy(name = "${t.name} · ${shortCode(t.deviceId)}")
                else -> t
            }
        }
    }

    /** 挂接条目的在线：任一为 true → true；否则任一未知 → 未知；否则 false。 */
    private fun mergeOnline(cloud: Boolean?, link: Boolean?): Boolean? = when {
        cloud == true || link == true -> true
        cloud == null || link == null -> null
        else -> false
    }

    /**
     * 持久化 / 表单里记着的目标键 → 当前候选：直接命中，或 `link:<fp>` 的指纹已挂接到某云条目时解析为该云条目；
     * 目标已不在候选里（登出 / 解除配对 / 对方解除）为 null。
     */
    fun resolveTarget(id: String, targets: List<DispatchTarget>): DispatchTarget? {
        targets.firstOrNull { it.id == id }?.let { return it }
        if (!id.startsWith("link:")) return null
        val fingerprint = id.removePrefix("link:")
        return targets.firstOrNull { it.linkFingerprint == fingerprint }
    }

    /** 与同账号云设备合并为一条的直连记录指纹（账户页已配对列表据此显示「同一账号」标签；revoked 不挂接）。 */
    fun attachedLinkFingerprints(cloud: List<CloudDevice>, link: List<LinkDevice>): Set<String> =
        dispatchTargets(cloud, link, cloudPresenceKnown = false, localReady = false).mapNotNullTo(HashSet()) { it.linkFingerprint }

    /** 设备 id 的短码（末 4 位 ASCII 字母数字），用于同名设备消歧。 */
    fun shortCode(id: String): String = id.filter { it.isAsciiLetter() || it in '0'..'9' }.takeLast(4)
}

/** `agent.remote.dispatch` 参数：把下载经 FluxCloud 下发到本账号另一台受信任设备。 */
data class RemoteDispatchParams(
    /** 目标设备的 `deviceId`。 */
    val toDevice: String,
    val url: String,
    /** null = 由目标设备按 URL 推断。 */
    val fileName: String? = null,
    /** 目标设备上的保存目录；null = 目标设备默认下载目录。必须符合目标设备路径风格。 */
    val saveDir: String? = null,
) {
    fun toJson(): JsonValue =
        jsonObjectOmitNulls("toDevice" to toDevice, "url" to url, "fileName" to fileName, "saveDir" to saveDir)
}

/** `agent.link.dispatch` 参数：把下载直接下发到已配对的局域网设备。 */
data class LinkDispatchParams(
    val fingerprint: String,
    val url: String,
    val fileName: String? = null,
    /** 目标设备上的保存目录；null = 目标设备默认下载目录。 */
    val saveDir: String? = null,
) {
    fun toJson(): JsonValue =
        jsonObjectOmitNulls("fingerprint" to fingerprint, "url" to url, "fileName" to fileName, "saveDir" to saveDir)
}
