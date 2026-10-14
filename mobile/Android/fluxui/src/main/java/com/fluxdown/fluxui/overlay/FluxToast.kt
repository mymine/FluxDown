package com.fluxdown.fluxui.overlay

import android.content.Context
import android.os.Build
import android.view.accessibility.AccessibilityManager
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.fluxdown.fluxui.icons.FluxIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.FluxBlur
import com.fluxdown.fluxui.material.FluxGlass
import com.fluxdown.fluxui.material.fluxGlass
import com.fluxdown.fluxui.material.fluxGlow
import com.fluxdown.fluxui.material.softShadow
import com.fluxdown.fluxui.theme.FluxColors
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import com.fluxdown.fluxui.theme.fluxPressable
import kotlinx.coroutines.delay

/** Toast 语气：info（中性）· success（mint）· warn（amber）· error（coral）· accent（accentHi）。 */
enum class FluxToastKind { Info, Success, Warn, Error, Accent }

/** Toast 末尾动作（如“撤销”）。点击后执行并关闭 Toast。 */
@Immutable
class FluxToastAction(val label: String, val onClick: () -> Unit)

internal class ToastEntry(
    val id: Long,
    val text: String,
    val kind: FluxToastKind,
    val icon: ImageVector?,
    val action: FluxToastAction?,
    val durationMs: Long,
)

/** 无障碍推荐停留时长（Android 10+）；更低版本沿用默认时长。 */
private fun recommendedTimeout(ctx: Context, e: ToastEntry): Long {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return e.durationMs
    val am = ctx.getSystemService(AccessibilityManager::class.java) ?: return e.durationMs
    val flags = AccessibilityManager.FLAG_CONTENT_TEXT or
        (if (e.action != null) AccessibilityManager.FLAG_CONTENT_CONTROLS else 0)
    return am.getRecommendedTimeoutMillis(e.durationMs.toInt(), flags).toLong()
}

@Immutable
private class ToneStyle(val fg: Color, val bg: Color, val defaultIcon: ImageVector)

private fun toneOf(kind: FluxToastKind, c: FluxColors): ToneStyle = when (kind) {
    FluxToastKind.Info -> ToneStyle(c.inkMuted, c.glass3, FluxIcons.Info)
    FluxToastKind.Success -> ToneStyle(c.mintText, c.mint.copy(alpha = 0.16f), FluxIcons.Check)
    FluxToastKind.Warn -> ToneStyle(c.amberText, c.amber.copy(alpha = 0.16f), FluxIcons.TriangleAlert)
    FluxToastKind.Error -> ToneStyle(c.coralText, c.coral.copy(alpha = 0.16f), FluxIcons.CircleAlert)
    FluxToastKind.Accent -> ToneStyle(c.accentHi, c.accentLo, FluxIcons.Zap)
}

/** z88。顶部居中，top = 状态栏 + 6，入场 `liquid`（translateY −20 / scale .9 / 模糊 8 → 0）。 */
@Composable
internal fun ToastLayer(state: FluxOverlayState) {
    val entry = state.toastEntry
    val motion = FluxTheme.motion
    val haptics = FluxTheme.haptics
    val ctx = LocalContext.current
    val prog = rememberOverlayProgress()
    val last = remember { Last<ToastEntry>() }
    if (entry != null) last.value = entry

    LaunchedEffect(entry) {
        if (entry != null) {
            when (entry.kind) {
                FluxToastKind.Success -> haptics.confirm()
                FluxToastKind.Warn, FluxToastKind.Error -> haptics.reject()
                else -> Unit
            }
            prog.enter(motion.of(motion.liquid), restartAt = 0.55f)
        } else {
            prog.exit(motion.of(motion.snap))
        }
    }
    LaunchedEffect(entry) {
        if (entry != null) {
            delay(recommendedTimeout(ctx, entry))
            state.dismissToastIf(entry.id)
        }
    }

    val shown = last.value
    if (shown == null || !prog.present) return

    val colors = FluxTheme.colors
    val type = FluxTheme.type
    val tone = toneOf(shown.kind, colors)
    val shape = FluxTheme.shapes.full
    val hasAction = shown.action != null

    Box(
        Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.statusBars)
            .padding(start = 20.dp, end = 20.dp, top = 6.dp),
        contentAlignment = Alignment.TopCenter,
    ) {
        Row(
            Modifier
                .graphicsLayer {
                    val v = prog.value
                    alpha = v.coerceIn(0f, 1f)
                    val s = 0.9f + 0.1f * v
                    scaleX = s
                    scaleY = s
                    translationY = -20.dp.toPx() * (1f - v)
                    transformOrigin = TransformOrigin(0.5f, 0f)
                }
                .fluxPressable(onClick = { state.dismissToastIf(shown.id) }, scale = 0.98f, role = null)
                .fluxGlow(colors.softShadow(0.45f), 16.dp, shape, spread = (-8).dp, dy = 12.dp)
                .fluxGlass(FluxGlass.Menu, shape, FluxBlur.Thick)
                .semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }
                .widthIn(min = 120.dp)
                .heightIn(min = 48.dp)
                .padding(start = 12.dp, end = if (hasAction) 6.dp else 18.dp, top = 4.dp, bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Box(Modifier.size(24.dp).background(tone.bg, shape), contentAlignment = Alignment.Center) {
                FluxIcon(shown.icon ?: tone.defaultIcon, null, size = 14.dp, tint = tone.fg)
            }
            FluxText(
                shown.text,
                modifier = Modifier.weight(1f, fill = false),
                style = type.weight(type.sm, 500),
                color = colors.ink,
                maxLines = 2,
            )
            shown.action?.let { action ->
                Box(
                    Modifier
                        .heightIn(min = 40.dp)
                        .fluxPressable(onClick = {
                            state.dismissToastIf(shown.id)
                            action.onClick()
                        })
                        .background(colors.accentLo, FluxTheme.shapes.full)
                        .padding(horizontal = 12.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    FluxText(action.label, style = type.weight(type.sm, 600), color = colors.accentHi, maxLines = 1)
                }
            }
        }
    }
}
