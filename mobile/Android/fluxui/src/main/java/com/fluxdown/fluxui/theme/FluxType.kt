package com.fluxdown.fluxui.theme

import android.content.Context
import android.graphics.Typeface
import android.graphics.fonts.Font as PlatformFont
import android.graphics.fonts.FontFamily as PlatformFamily
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.compose.runtime.Immutable
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.Hyphens
import androidx.compose.ui.text.style.LineBreak
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import java.io.IOException

/**
 * 字体：拉丁 Geist / Geist Mono（可变 wght），中文 MiSans（三档静态字重），其余交给系统。
 * Compose 的 FontFamily 不是逐字形回退链，必须用平台 [Typeface.CustomFallbackBuilder] 组装（§3.1）。
 * 每个字重一个已加载族；`TextStyle.fontWeight` 对已加载族无效，故字重由族本身决定。
 * Android 10 以下没有自定义回退链：只加载拉丁可变字体，中文等交给系统默认回退。
 */
object FluxFonts {
    private const val TAG = "FluxFonts"
    private val cache = HashMap<Int, FontFamily>()

    /** 冷启动时可在后台线程预热，避免首帧在主线程解析字体文件。 */
    fun preload(context: Context) {
        for (w in WEIGHTS) {
            family(context, mono = false, weight = w)
            family(context, mono = true, weight = w)
        }
    }

    fun family(context: Context, mono: Boolean, weight: Int): FontFamily = synchronized(cache) {
        cache.getOrPut(if (mono) -weight else weight) { build(context.applicationContext, mono, weight) }
    }

    private fun build(ctx: Context, mono: Boolean, weight: Int): FontFamily = try {
        val latinFile = if (mono) "fonts/GeistMono.ttf" else "fonts/Geist.ttf"
        val system = if (mono) "monospace" else "sans-serif"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            FontFamily(withCjkFallback(ctx, latinFile, system, weight))
        } else {
            FontFamily(latinOnly(ctx, latinFile, system, weight))
        }
    } catch (e: IOException) {
        // 资源缺失只降级为系统字体，不崩溃（§3.1）
        Log.e(TAG, "font load failed (mono=$mono, w=$weight): $e")
        if (mono) FontFamily.Monospace else FontFamily.SansSerif
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun withCjkFallback(ctx: Context, latinFile: String, system: String, weight: Int): Typeface {
        val latinFont = PlatformFont.Builder(ctx.assets, latinFile)
            .setWeight(weight)
            .setFontVariationSettings("'wght' $weight")
            .build()
        val cjkFile = when {
            weight >= 600 -> "fonts/MiSans-Semibold.ttf"
            weight >= 500 -> "fonts/MiSans-Medium.ttf"
            else -> "fonts/MiSans-Regular.ttf"
        }
        val cjkFont = PlatformFont.Builder(ctx.assets, cjkFile).setWeight(weight).build()
        return Typeface.CustomFallbackBuilder(PlatformFamily.Builder(latinFont).build())
            .addCustomFallback(PlatformFamily.Builder(cjkFont).build())
            .setSystemFallback(system)
            .build()
    }

    /**
     * API 26–28：`Typeface.Builder` 同样支持可变字重，但没有自定义回退链；
     * 字体文件读取失败时按 [system] 回退系统字体（`setFallback` 保证 `build()` 不返回 null）。
     */
    private fun latinOnly(ctx: Context, latinFile: String, system: String, weight: Int): Typeface =
        Typeface.Builder(ctx.assets, latinFile)
            .setWeight(weight)
            .setFontVariationSettings("'wght' $weight")
            .setFallback(system)
            .build()

    private val WEIGHTS = intArrayOf(400, 450, 500, 600, 700)
}

/** 非线性字体缩放组（§3.3）：小字放大多、大字放大少。 */
enum class FluxScaleGroup(val k200: Float) { Ks(1.85f), Km(1.48f), Kl(1.20f) }

fun scaleOf(group: FluxScaleGroup, fontScale: Float): Float =
    if (fontScale <= 1f) fontScale.coerceAtLeast(0.85f)
    else 1f + (group.k200 - 1f) * (fontScale - 1f).coerceAtMost(1f)

/** 字阶（§3.2）。字号已按系统字体缩放折算，组件内部禁止再写 `.sp` 常数。 */
@Immutable
class FluxType(
    val display: TextStyle,
    val title: TextStyle,
    val h1: TextStyle,
    val h2: TextStyle,
    val body: TextStyle,
    val bodyM: TextStyle,
    val sm: TextStyle,
    val micro: TextStyle,
    val mono: TextStyle,
    val monoS: TextStyle,
    /** 原始系统字体缩放，供版面容错规则（如 ≥1.5 时元信息改 2 行）判断。 */
    val fontScale: Float,
    private val fonts: (mono: Boolean, weight: Int) -> FontFamily,
    private val scale: (FluxScaleGroup) -> Float,
) {
    /** 同字阶换字重（如列表行标题 450、按钮 600）：`type.weight(type.body, 450)`。 */
    fun weight(base: TextStyle, w: Int, mono: Boolean = false): TextStyle = base.copy(fontFamily = fonts(mono, w))

    /** 组件级变体字号：基准 sp × 缩放组系数（如顶部读数条数字 15 × km）。 */
    fun sized(base: TextStyle, sizeSp: Float, group: FluxScaleGroup): TextStyle =
        base.copy(fontSize = (sizeSp * scale(group)).sp)

    companion object {
        fun of(context: Context, fontScale: Float, cjk: Boolean): FluxType {
            val fam = { mono: Boolean, w: Int -> FluxFonts.family(context, mono, w) }
            val sc = { g: FluxScaleGroup -> scaleOf(g, fontScale) }
            fun st(size: Float, g: FluxScaleGroup, lh: Float, weight: Int, ls: Float, mono: Boolean = false, tnum: Boolean = false) =
                TextStyle(
                    fontFamily = fam(mono, weight),
                    fontSize = (size * sc(g)).sp,
                    lineHeight = lh.em,
                    letterSpacing = ls.em,
                    fontFeatureSettings = if (tnum || mono) "tnum" else null,
                    lineBreak = if (size >= 17f) LineBreak.Heading else LineBreak.Paragraph,
                    hyphens = Hyphens.None,
                    platformStyle = PlatformTextStyle(includeFontPadding = false),
                )
            return FluxType(
                display = st(56f, FluxScaleGroup.Kl, 1.00f, 600, -0.02f, tnum = true),
                title = st(34f, FluxScaleGroup.Kl, 1.10f, 600, if (cjk) -0.005f else -0.015f),
                h1 = st(22f, FluxScaleGroup.Kl, 1.25f, 600, -0.01f),
                h2 = st(17f, FluxScaleGroup.Km, 1.30f, 600, -0.005f),
                body = st(15f, FluxScaleGroup.Km, 1.45f, 400, 0f),
                bodyM = st(15f, FluxScaleGroup.Km, 1.40f, 500, 0f),
                sm = st(13f, FluxScaleGroup.Ks, 1.40f, 400, 0f),
                micro = st(11f, FluxScaleGroup.Ks, 1.30f, 500, if (cjk) 0.02f else 0.08f),
                mono = st(12f, FluxScaleGroup.Ks, 1.35f, 400, -0.01f, mono = true),
                monoS = st(11f, FluxScaleGroup.Ks, 1.30f, 400, 0f, mono = true),
                fontScale = fontScale,
                fonts = fam,
                scale = sc,
            )
        }
    }
}
