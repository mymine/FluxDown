package com.fluxdown.fluxui.data

import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.PointMode
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import com.fluxdown.fluxui.theme.FluxTheme
import kotlin.math.min

/**
 * 波形数据：按时间升序（最旧在前）的速度序列，只有前 [count] 个有效（便于复用缓冲区，绘制零分配）。
 *
 * - [down] / [up]：原始 B/s（或任何同量纲值）；[up] 为空则不画上行线。
 * - [maxValue]：y 轴上限；NaN = 按规则 `max(1 MiB, 序列最大值) × 1.18` 自动计算。
 */
@Stable
class WaveSeries(
    val down: FloatArray,
    val up: FloatArray = EMPTY,
    val count: Int = down.size,
    val maxValue: Float = Float.NaN,
) {
    companion object {
        val EMPTY = FloatArray(0)

        /** 平线（idle）：无数据。 */
        val Idle = WaveSeries(EMPTY, EMPTY, 0)
    }
}

private const val MIB = 1024f * 1024f
private const val MINI_POINTS = 30

/** 非 mini 波形的上下内边距 / 右侧给头点留出的宽度；叠层（刻度、读数）与曲线共用同一几何。 */
internal val WAVE_PAD = 8.dp
internal val WAVE_HEAD_INSET = 4.dp

/**
 * 实时速度波形（品牌签名之二）：点阵网格 → 面积纯色（低 α）→ 上行虚线 → 主线 → 头部光点，
 * 左侧 0→28% 渐隐（内容遮罩）。曲线为 Catmull-Rom → 三次贝塞尔（张力 .18）。
 *
 * - [samples] 在**绘制阶段**读取：把它连到 Snapshot State 即可只触发重绘、不触发重组；
 *   绘制路径复用同一组 `Path`，逐帧零分配。
 * - [mini]：最近 30 点、无网格 / 上行线 / 头点，面积纯色 α .10（顶部读数条）。
 * - 高度由调用方给定（建议 92 / 56 / mini 22）；未给定时 92 / 22。
 * - 装饰：不进入无障碍树（由所属仪表汇总朗读）。
 */
@Composable
fun Waveform(
    samples: () -> WaveSeries,
    modifier: Modifier = Modifier,
    mini: Boolean = false,
    /** 左侧 0→28% 渐隐；整段历史曲线（起点有意义）传 false。 */
    fadeStart: Boolean = true,
) {
    val colors = FluxTheme.colors
    val latest = rememberUpdatedState(samples)
    Spacer(
        modifier
            .fillMaxWidth()
            .height(if (mini) 22.dp else 92.dp)
            .clearAndSetSemantics { }
            .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
            .drawWithCache {
                val w = size.width
                val h = size.height
                val pad = (if (mini) 3.dp else WAVE_PAD).toPx()
                val chartW = if (mini) w else (w - WAVE_HEAD_INSET.toPx()).coerceAtLeast(1f)
                val line = Path()
                val area = Path()
                val upPath = Path()
                val lineStroke = Stroke(1.6.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round)
                val upStroke = Stroke(
                    1.dp.toPx(),
                    cap = StrokeCap.Butt,
                    join = StrokeJoin.Round,
                    pathEffect = PathEffect.dashPathEffect(floatArrayOf(2.dp.toPx(), 3.dp.toPx())),
                )
                val hi = colors.accentHi
                val areaColor = hi.copy(alpha = if (mini) 0.10f else 0.12f)
                val upColor = colors.inkMuted.copy(alpha = 0.75f)
                val maskBrush = Brush.horizontalGradient(
                    0f to Color.Transparent, 0.28f to Color.Black, 1f to Color.Black,
                    startX = 0f, endX = w,
                )
                val headR = 2.8.dp.toPx()
                val headGlowR = 9.dp.toPx()
                val headGlow = Brush.radialGradient(
                    listOf(colors.accentGlow, Color.Transparent),
                    center = Offset.Zero,
                    radius = headGlowR,
                )

                // 点阵网格只在尺寸变化时录制一次
                val gridLayer = if (mini) null else obtainGraphicsLayer().also { layer ->
                    val step = 12.dp.toPx()
                    val off = 1.dp.toPx()
                    val cols = ((w - off) / step).toInt() + 1
                    val rows = ((h - off) / step).toInt() + 1
                    val pts = ArrayList<Offset>(cols * rows)
                    for (r in 0 until rows) for (c in 0 until cols) pts.add(Offset(off + c * step, off + r * step))
                    val dot = colors.hairlineStrong.copy(alpha = colors.hairlineStrong.alpha * 0.9f)
                    layer.record(this, layoutDirection, IntSize(w.toInt().coerceAtLeast(1), h.toInt().coerceAtLeast(1))) {
                        drawPoints(pts, PointMode.Points, dot, strokeWidth = 1.6.dp.toPx(), cap = StrokeCap.Round)
                    }
                }

                onDrawBehind {
                    val s = latest.value()
                    val total = min(s.count, s.down.size)
                    val n = if (mini) min(total, MINI_POINTS) else total
                    val off = total - n
                    val upTotal = min(s.count, s.up.size)
                    val upN = if (mini) min(upTotal, MINI_POINTS) else upTotal
                    val upOff = upTotal - upN
                    val max = if (s.maxValue.isNaN()) autoMax(s.down, off, n, s.up, upOff, upN) else s.maxValue

                    if (gridLayer != null) drawLayer(gridLayer)

                    buildSmooth(line, s.down, off, n, chartW, h, pad, max)
                    area.reset()
                    area.addPath(line)
                    area.lineTo(chartW, h)
                    area.lineTo(0f, h)
                    area.close()
                    drawPath(area, areaColor, style = Fill)

                    if (!mini) {
                        if (upN > 0) {
                            buildSmooth(upPath, s.up, upOff, upN, chartW, h, pad, max)
                            drawPath(upPath, upColor, style = upStroke)
                        }
                    }
                    drawPath(line, hi, style = lineStroke)

                    if (!mini) {
                        val hy = yOf(if (n > 0) s.down[off + n - 1] else 0f, h, pad, max)
                        translate(chartW, hy) {
                            drawCircle(headGlow, headGlowR, Offset.Zero)
                            drawCircle(hi, headR, Offset.Zero)
                        }
                    }
                    if (fadeStart) drawRect(maskBrush, size = Size(w, h), blendMode = BlendMode.DstIn)
                }
            },
    )
}

private fun autoMax(down: FloatArray, dOff: Int, dN: Int, up: FloatArray, uOff: Int, uN: Int): Float {
    var m = MIB
    for (i in 0 until dN) {
        val v = down[dOff + i]
        if (v > m) m = v
    }
    for (i in 0 until uN) {
        val v = up[uOff + i]
        if (v > m) m = v
    }
    return m * 1.18f
}

internal fun yOf(v: Float, h: Float, pad: Float, max: Float): Float {
    val r = if (max <= 0f) 0f else (v / max).coerceIn(0f, 1f)
    return h - pad - r * (h - pad * 2.2f)
}

/** 曲线第 [i] 个点（共 [n] 个）的 x；与 [buildSmooth] 等距布点一致，n<2 时为 0。 */
internal fun waveXAt(i: Int, chartW: Float, n: Int): Float = if (n < 2) 0f else i * chartW / (n - 1)

/** 把触点 x 吸附到最近的采样点下标（0 基）；n<=0 返回 -1。 */
internal fun waveIndexAt(x: Float, chartW: Float, n: Int): Int {
    if (n <= 0) return -1
    if (n == 1 || chartW <= 0f) return 0
    return Math.round(x / chartW * (n - 1)).coerceIn(0, n - 1)
}

/** Catmull-Rom → 三次贝塞尔（张力 .18），点 i 取 `a[off + i]`；n<2 时画一条水平线。 */
private fun buildSmooth(path: Path, a: FloatArray, off: Int, n: Int, w: Float, h: Float, pad: Float, max: Float) {
    path.reset()
    if (n < 2) {
        val y = yOf(if (n == 1) a[off] else 0f, h, pad, max)
        path.moveTo(0f, y)
        path.lineTo(w, y)
        return
    }
    val t = 0.18f
    val dx = w / (n - 1)
    path.moveTo(0f, yOf(a[off], h, pad, max))
    for (i in 0 until n - 1) {
        val i0 = if (i > 0) i - 1 else i
        val i3 = if (i + 2 < n) i + 2 else i + 1
        val x0 = i0 * dx
        val x1 = i * dx
        val x2 = (i + 1) * dx
        val x3 = i3 * dx
        val y0 = yOf(a[off + i0], h, pad, max)
        val y1 = yOf(a[off + i], h, pad, max)
        val y2 = yOf(a[off + i + 1], h, pad, max)
        val y3 = yOf(a[off + i3], h, pad, max)
        path.cubicTo(
            x1 + (x2 - x0) * t, y1 + (y2 - y0) * t,
            x2 - (x3 - x1) * t, y2 - (y3 - y1) * t,
            x2, y2,
        )
    }
}
