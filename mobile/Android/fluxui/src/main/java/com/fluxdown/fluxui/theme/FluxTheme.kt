package com.fluxdown.fluxui.theme

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.takeOrElse
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat

/** 强调色来源：只填 accent 一个槽位（§2.4）。优先级 Wallpaper > Custom > Preset。 */
@Immutable
sealed interface FluxAccent {
    @Immutable data class Preset(val id: String) : FluxAccent
    @Immutable data class Custom(val argb: Int) : FluxAccent
    @Immutable data object Wallpaper : FluxAccent

    companion object {
        val Presets: List<Pair<String, Color>> = listOf(
            "blue" to Color(0xFF3B82F6),
            "green" to Color(0xFF22C55E),
            "violet" to Color(0xFF8B5CF6),
            "rose" to Color(0xFFF43F5E),
        )
        const val DEFAULT_CUSTOM: Int = 0xFF6366F1.toInt()

        fun presetColor(id: String): Color = Presets.firstOrNull { it.first == id }?.second ?: Presets[0].second
    }
}

val LocalFluxColors = staticCompositionLocalOf<FluxColors> { error("FluxTheme missing") }
val LocalFluxType = staticCompositionLocalOf<FluxType> { error("FluxTheme missing") }
val LocalFluxShapes = staticCompositionLocalOf { FluxShapes() }
val LocalFluxSpace = staticCompositionLocalOf { FluxSpace() }
val LocalFluxMotion = staticCompositionLocalOf { FluxMotion(reduce = false) }
val LocalFluxHaptics = staticCompositionLocalOf<FluxHaptics> { error("FluxTheme missing") }
val LocalFluxPerf = staticCompositionLocalOf { FluxPerf(FluxGlassMode.Blur) }
val LocalFluxWindowClass = staticCompositionLocalOf { FluxWindowClass.Compact }
val LocalFluxGlassMode = compositionLocalOf { FluxGlassMode.Blur }
/** 自有内容色（不是 Material 的 LocalContentColor）。 */
val LocalFluxContentColor = compositionLocalOf { Color.Unspecified }
val LocalFluxTextStyle = compositionLocalOf { TextStyle.Default }

object FluxTheme {
    val colors: FluxColors @Composable @ReadOnlyComposable get() = LocalFluxColors.current
    val type: FluxType @Composable @ReadOnlyComposable get() = LocalFluxType.current
    val shapes: FluxShapes @Composable @ReadOnlyComposable get() = LocalFluxShapes.current
    val space: FluxSpace @Composable @ReadOnlyComposable get() = LocalFluxSpace.current
    val motion: FluxMotion @Composable @ReadOnlyComposable get() = LocalFluxMotion.current
    val haptics: FluxHaptics @Composable @ReadOnlyComposable get() = LocalFluxHaptics.current
    val perf: FluxPerf @Composable @ReadOnlyComposable get() = LocalFluxPerf.current
    val windowClass: FluxWindowClass @Composable @ReadOnlyComposable get() = LocalFluxWindowClass.current
    val contentColor: Color
        @Composable @ReadOnlyComposable get() = LocalFluxContentColor.current.takeOrElse { LocalFluxColors.current.ink }
}

/**
 * Flux Lumen 主题入口。
 * - 字号由 [FluxType] 按 §3.3 非线性表自行缩放，因此向下提供 `fontScale = 1` 的 Density。
 * - 无水波：[LocalIndication] = 按压缩放。
 * - 系统栏内容色随明暗切换（边到边由 Activity 的 enableEdgeToEdge 负责）。
 */
@Composable
fun FluxTheme(
    dark: Boolean = isSystemInDarkTheme(),
    accent: FluxAccent = FluxAccent.Preset("blue"),
    imported: ImportedPalette? = null,
    reduceMotion: Boolean = rememberSystemReduceMotion(),
    content: @Composable () -> Unit,
) {
    val ctx = LocalContext.current
    val view = LocalView.current
    val config = LocalConfiguration.current
    val fontScale = config.fontScale
    val seed = remember(accent, ctx, config.uiMode) {
        when (accent) {
            is FluxAccent.Preset -> FluxAccent.presetColor(accent.id)
            is FluxAccent.Custom -> Color(accent.argb)
            FluxAccent.Wallpaper -> wallpaperAccent(ctx) ?: FluxAccent.presetColor("blue")
        }
    }
    val colors = remember(dark, seed, imported) { FluxColors.of(dark, seed, imported) }
    val cjk = config.locales[0].language in CJK
    val type = remember(fontScale, cjk) { FluxType.of(ctx, fontScale, cjk) }
    val motion = remember(reduceMotion) { FluxMotion(reduceMotion) }
    val haptics = remember(view) { FluxHaptics(view) }
    val perf = rememberFluxPerf(ctx)
    val windowClass = when {
        config.screenWidthDp < 600 -> FluxWindowClass.Compact
        config.screenWidthDp < 840 -> FluxWindowClass.Medium
        else -> FluxWindowClass.Expanded
    }
    val space = remember(windowClass) {
        FluxSpace(screenMargin = if (windowClass == FluxWindowClass.Compact) 16.dp else 20.dp)
    }
    val density = LocalDensity.current

    if (!view.isInEditMode) {
        val activity = ctx.findActivity()
        SideEffect {
            if (activity != null) {
                WindowCompat.getInsetsController(activity.window, view).apply {
                    isAppearanceLightStatusBars = !dark
                    isAppearanceLightNavigationBars = !dark
                }
            }
        }
    }

    CompositionLocalProvider(
        LocalFluxColors provides colors,
        LocalFluxType provides type,
        LocalFluxSpace provides space,
        LocalFluxMotion provides motion,
        LocalFluxHaptics provides haptics,
        LocalFluxPerf provides perf,
        LocalFluxWindowClass provides windowClass,
        LocalFluxGlassMode provides perf.glassMode,
        LocalDensity provides Density(density.density, fontScale = 1f),
        LocalIndication provides FluxPressIndication,
        LocalFluxContentColor provides colors.ink,
        LocalFluxTextStyle provides type.body,
        content = content,
    )
}

private val CJK = setOf("zh", "ja", "ko")

/** 壁纸取色（Material You 系统调色板）：Android 12 以下没有该调色板，返回 null。 */
fun wallpaperAccent(ctx: Context): Color? {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return null
    return Color(ctx.getColor(android.R.color.system_accent1_500)).takeIf { it.oklabChroma() >= 0.04f }
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/** 系统“移除动画”（ANIMATOR_DURATION_SCALE == 0）即 Reduce motion；不设 App 内开关。 */
@Composable
fun rememberSystemReduceMotion(): Boolean {
    val ctx = LocalContext.current
    val config = LocalConfiguration.current
    return remember(ctx, config) {
        Settings.Global.getFloat(ctx.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
}

/** 省电 / 过热（≥ SEVERE）/ 低内存 → 玻璃降级实色（§5.6）；Android 12 以下没有 `RenderEffect`，恒为实色。 */
@Composable
fun rememberFluxPerf(ctx: Context): FluxPerf {
    val pm = remember(ctx) { ctx.getSystemService(PowerManager::class.java) }
    val am = remember(ctx) { ctx.getSystemService(android.app.ActivityManager::class.java) }
    var powerSave by remember { mutableStateOf(pm?.isPowerSaveMode == true) }
    var thermal by remember { mutableStateOf(pm?.let(::thermalStatus) ?: PowerManager.THERMAL_STATUS_NONE) }
    DisposableEffect(pm) {
        if (pm == null) return@DisposableEffect onDispose { }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                powerSave = pm.isPowerSaveMode
            }
        }
        ContextCompat.registerReceiver(
            ctx,
            receiver,
            IntentFilter(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        val stopThermal = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) watchThermal(pm) { thermal = it } else null
        onDispose {
            ctx.unregisterReceiver(receiver)
            stopThermal?.invoke()
        }
    }
    val degraded = Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
        powerSave || thermal >= PowerManager.THERMAL_STATUS_SEVERE || am?.isLowRamDevice == true
    return remember(degraded) {
        if (degraded) FluxPerf(FluxGlassMode.Solid) else FluxPerf(FluxGlassMode.Blur)
    }
}

/** 温控状态：Android 10 以下无该接口，视为正常。 */
private fun thermalStatus(pm: PowerManager): Int =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) pm.currentThermalStatus else PowerManager.THERMAL_STATUS_NONE

/** 温控状态监听（API 29+）；返回注销函数。 */
@RequiresApi(Build.VERSION_CODES.Q)
private fun watchThermal(pm: PowerManager, onChange: (Int) -> Unit): () -> Unit {
    val listener = PowerManager.OnThermalStatusChangedListener(onChange)
    pm.addThermalStatusListener(listener)
    return { pm.removeThermalStatusListener(listener) }
}
