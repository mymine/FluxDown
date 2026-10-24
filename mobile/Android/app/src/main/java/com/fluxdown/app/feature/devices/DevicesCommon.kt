package com.fluxdown.app.feature.devices

import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.takeOrElse
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.i18n.str
import com.fluxdown.core.model.HostRef
import com.fluxdown.fluxui.icons.FluxIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.FluxGlass
import com.fluxdown.fluxui.material.FluxGlassKind
import com.fluxdown.fluxui.material.fluxGlass
import com.fluxdown.fluxui.theme.FluxTheme

/** 主机显示名：本机走 i18n（存储里的 displayName 不随语言变化），远端沿用用户起的名称。 */
@Composable
fun HostRef.localizedName(): String = when (this) {
    is HostRef.Local -> str(R.string.mobileHostLocalName)
    is HostRef.Remote -> displayName
}

/** 主机副标题：本机 = 进程内引擎；远端 = 地址。 */
@Composable
fun HostRef.localizedSubtitle(): String = when (this) {
    is HostRef.Local -> str(R.string.mobileHostLocalEngine)
    is HostRef.Remote -> endpoint
}

/**
 * 玻璃图块（Flat G2 + 发丝线）：40dp r12 / 32dp r10。[ring] 非空时加描边（失败态）；
 * [overlay] 非空时替换图标（如抓取中的点阵轨道）。装饰节点，不进入无障碍树。
 */
@Composable
fun GlyphTile(
    icon: ImageVector,
    modifier: Modifier = Modifier,
    tint: Color = Color.Unspecified,
    ring: Color? = null,
    size: Dp = 40.dp,
    iconSize: Dp = if (size >= 40.dp) 20.dp else 17.dp,
    overlay: (@Composable BoxScope.() -> Unit)? = null,
) {
    val shape = if (size >= 40.dp) FluxTheme.shapes.tile else FluxTheme.shapes.tileSm
    Box(
        modifier
            .size(size)
            .fluxGlass(FluxGlass.G2, shape, kind = FluxGlassKind.Flat)
            .then(if (ring != null) Modifier.border(1.dp, ring, shape) else Modifier)
            .clearAndSetSemantics { },
        contentAlignment = Alignment.Center,
    ) {
        if (overlay != null) overlay() else FluxIcon(icon, null, size = iconSize, tint = tint.takeOrElse { FluxTheme.colors.ink })
    }
}

/** 平台 → 图标。 */
fun platformIcon(platform: String?): ImageVector = when (platform?.lowercase()) {
    "android", "ios" -> FluxIcons.Smartphone
    "macos" -> FluxIcons.Laptop
    "windows" -> FluxIcons.Monitor
    "linux" -> FluxIcons.Terminal
    "web" -> FluxIcons.Globe
    else -> FluxIcons.Network
}

/** 平台显示名（`accountDevicePlatform*`）；未知平台原样显示，空 = null。 */
@Composable
fun platformLabel(platform: String?): String? = when (platform?.lowercase()) {
    null, "" -> null
    "windows" -> str(R.string.accountDevicePlatformWindows)
    "macos" -> str(R.string.accountDevicePlatformMacos)
    "linux" -> str(R.string.accountDevicePlatformLinux)
    "android" -> str(R.string.accountDevicePlatformAndroid)
    "ios" -> str(R.string.accountDevicePlatformIos)
    "web" -> str(R.string.accountDevicePlatformWeb)
    else -> platform
}
