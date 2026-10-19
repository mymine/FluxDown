package com.fluxdown.fluxui.data

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlin.math.min
import kotlin.math.roundToInt

/** 数值轴的一个刻度：[value] 与 [WaveSeries.down] 同量纲，[label] 为已格式化的文本（含单位）。 */
@Immutable
class WaveTick(val value: Float, val label: String)

/**
 * 可读数的波形图：[Waveform] + 数值轴刻度（线 + 标签，画在绘图区内，不占宽度）+ 触摸读数。
 *
 * - 读数：轻点某处读取最近采样点（再点同一点收起），或长按后左右拖动连续读取；
 *   气泡文本由 [readout]（采样点下标 → 已格式化文本）给出，竖线 + 圆点标出该点。
 * - 刻度：要求 [samples] 的 [WaveSeries.maxValue] 已显式给定（与 [WaveTick.value] 共用同一 y 映射）；
 *   NaN 时不画刻度。
 * - 读数手势不与外层垂直滚动冲突：轻点由点击识别，拖动须先长按。
 */
@Composable
fun WaveChart(
    samples: () -> WaveSeries,
    ticks: List<WaveTick>,
    readout: (index: Int) -> String,
    modifier: Modifier = Modifier,
    height: Dp = 120.dp,
    fadeStart: Boolean = true,
) {
    val colors = FluxTheme.colors
    val type = FluxTheme.type
    val haptics = FluxTheme.haptics
    val density = LocalDensity.current
    val latest = rememberUpdatedState(samples)
    var size by remember { mutableStateOf(IntSize.Zero) }
    var selected by remember { mutableIntStateOf(-1) }

    fun pointCount(): Int = latest.value().let { min(it.count, it.down.size) }
    val chartWOf = { (size.width - with(density) { WAVE_HEAD_INSET.toPx() }).coerceAtLeast(1f) }
    fun pick(x: Float) {
        val i = waveIndexAt(x, chartWOf(), pointCount())
        if (i >= 0 && i != selected) {
            selected = i
            haptics.tick()
        }
    }

    Box(
        modifier
            .height(height)
            .onSizeChanged { size = it }
            .pointerInput(Unit) {
                detectTapGestures { p ->
                    val i = waveIndexAt(p.x, chartWOf(), pointCount())
                    selected = if (i < 0 || i == selected) -1 else i
                    if (selected >= 0) haptics.tick()
                }
            }
            .pointerInput(Unit) {
                detectDragGesturesAfterLongPress(
                    onDragStart = { p ->
                        haptics.longPress()
                        pick(p.x)
                    },
                    onDrag = { change, _ ->
                        change.consume()
                        pick(change.position.x)
                    },
                )
            },
    ) {
        Waveform(samples = samples, modifier = Modifier.fillMaxSize(), fadeStart = fadeStart)

        // 刻度线 + 选中点的竖线与圆点
        Canvas(Modifier.fillMaxSize()) {
            val s = latest.value()
            val n = min(s.count, s.down.size)
            val max = s.maxValue
            val pad = WAVE_PAD.toPx()
            if (!max.isNaN()) {
                for (t in ticks) {
                    val y = yOf(t.value, this.size.height, pad, max)
                    drawLine(colors.hairline, Offset(0f, y), Offset(this.size.width, y), strokeWidth = 1.dp.toPx())
                }
            }
            if (selected >= 0 && n > 0 && !max.isNaN()) {
                val i = min(selected, n - 1)
                val chartW = (this.size.width - WAVE_HEAD_INSET.toPx()).coerceAtLeast(1f)
                val x = waveXAt(i, chartW, n)
                val y = yOf(s.down[i], this.size.height, pad, max)
                drawLine(colors.accentHi.copy(alpha = 0.55f), Offset(x, 0f), Offset(x, this.size.height), strokeWidth = 1.dp.toPx())
                drawCircle(colors.canvas, 5.dp.toPx(), Offset(x, y))
                drawCircle(colors.accentHi, 5.dp.toPx(), Offset(x, y), style = Stroke(2.dp.toPx()))
            }
        }

        // 刻度标签：贴刻度线下沿、靠左，画在绘图区内
        for (t in ticks) {
            FluxText(
                t.label,
                style = type.monoS,
                color = colors.inkFaint,
                maxLines = 1,
                modifier = Modifier.offset {
                    val max = latest.value().maxValue
                    val y = if (max.isNaN()) 0f else yOf(t.value, size.height.toFloat(), WAVE_PAD.toPx(), max)
                    IntOffset(6.dp.roundToPx(), (y + 2.dp.toPx()).roundToInt())
                },
            )
        }

        // 读数气泡：水平跟随选中点并夹在图内，贴顶
        if (selected >= 0) {
            val text = readout(selected)
            Layout(
                content = {
                    FluxText(
                        text,
                        style = type.monoS,
                        color = colors.ink,
                        maxLines = 1,
                        modifier = Modifier
                            .clip(RoundedCornerShape(8.dp))
                            .background(colors.menuBg.compositeOver(colors.canvas))
                            .border(1.dp, colors.hairlineStrong, RoundedCornerShape(8.dp))
                            .padding(horizontal = 8.dp, vertical = 3.dp),
                    )
                },
                modifier = Modifier.fillMaxSize(),
            ) { measurables, c ->
                val p = measurables[0].measure(c.copy(minWidth = 0, minHeight = 0))
                layout(c.maxWidth, c.maxHeight) {
                    val n = pointCount()
                    val chartW = chartWOf()
                    val cx = if (n > 0) waveXAt(min(selected, n - 1), chartW, n) else 0f
                    val x = (cx - p.width / 2f).coerceIn(0f, (c.maxWidth - p.width).coerceAtLeast(0).toFloat())
                    p.place(x.roundToInt(), 2.dp.roundToPx())
                }
            }
        }
    }
}
