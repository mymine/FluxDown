package com.fluxdown.fluxui.chrome

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.fluxdown.fluxui.icons.FluxIcon
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.FluxBlur
import com.fluxdown.fluxui.material.FluxGlass
import com.fluxdown.fluxui.material.FluxGlassKind
import com.fluxdown.fluxui.material.fluxGlass
import com.fluxdown.fluxui.theme.FluxGlassMode
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import com.fluxdown.fluxui.theme.LocalFluxGlassMode
import kotlinx.coroutines.delay
import kotlinx.coroutines.withTimeoutOrNull

/** 新建球扇形的一个动作（按从下到上的顺序排列，第 0 项最靠近球）。 */
@Immutable
data class FluxOrbAction(val icon: ImageVector, val label: String)

/**
 * 新建球与扇形层共享的状态。用 [rememberFluxOrbState] 创建，同一个实例同时交给
 * [FluxOrb]（手势 / 球本体）与 [FluxOrbFanLayer]（遮罩与扇形项，应放在应用最上层）。
 */
@Stable
class FluxOrbState internal constructor() {
    /** 扇形是否展开（长按成立后为 true，松手 / 取消后为 false）。 */
    var fanOpen: Boolean by mutableStateOf(false)
        internal set

    /** 手指当前悬停的扇形项下标，-1 = 无。 */
    var hotIndex: Int by mutableIntStateOf(-1)
        internal set

    internal var orbRect: Rect by mutableStateOf(Rect.Zero)
    internal var actions: List<FluxOrbAction> by mutableStateOf(emptyList())

    /** 扇形层在根坐标中的原点；与 [itemRects]（根坐标）一起由扇形层布局时写入。 */
    internal var layerOrigin: Offset = Offset.Zero
    internal var itemRects: List<Rect> = emptyList()

    internal fun hitTest(root: Offset): Int {
        val rects = itemRects
        for (i in rects.indices) if (rects[i].contains(root)) return i
        return -1
    }
}

@Composable
fun rememberFluxOrbState(): FluxOrbState = remember { FluxOrbState() }

/**
 * 新建球（01 §12.3）：与导航坞等高的强调色实心按钮，与坞同一柔和投影托起。
 *
 * - 点按 → [onClick]；按住 [com.fluxdown.fluxui.theme.FluxMotion.orbHoldMs]（380ms）→ `longPress` 触感 + 扇形展开
 *   （由 [FluxOrbFanLayer] 绘制），手指滑过扇形项每进入新项 `tick`，在项上松手 → `confirm` + [onAction]，落空则收起。
 * - [selectionMode]：球就地变形为“退出选择”（与导航坞同一不透明面、ink 图标、`+` 转 45°），点按 = [onClick]，不展开扇形。
 * - a11y：`Role.Button`；`customActions` 等价于扇形各项（TalkBack 用户无需按住）。
 *
 * 球自身宽高均为主题 dockHeight，位置由应用摆放；其根坐标位置经 [state] 传给扇形层。
 */
@Composable
fun FluxOrb(
    state: FluxOrbState,
    onClick: () -> Unit,
    actions: List<FluxOrbAction>,
    onAction: (Int) -> Unit,
    modifier: Modifier = Modifier,
    selectionMode: Boolean = false,
    contentDescription: String = "新建下载",
    exitDescription: String = "退出选择",
) {
    val c = FluxTheme.colors
    val motion = FluxTheme.motion
    val haptics = FluxTheme.haptics
    val holdMs = motion.orbHoldMs
    val orbSize = FluxTheme.space.dockHeight
    SideEffect { state.actions = actions }

    val onClickState = rememberUpdatedState(onClick)
    val onActionState = rememberUpdatedState(onAction)
    var pressed by remember { mutableStateOf(false) }
    val fanOpen = state.fanOpen
    val coords = remember { arrayOfNulls<LayoutCoordinates>(1) }
    val fanEnabled = !selectionMode && actions.isNotEmpty()

    // 缩放只作用于视觉层，外层手势区域保持完整尺寸。
    val pressScale = remember { Animatable(1f) }
    val stateScale = remember { Animatable(1f) }
    val rotation = remember { Animatable(0f) }
    val sel = remember { Animatable(if (selectionMode) 1f else 0f) }
    var glassComposed by remember { mutableStateOf(selectionMode) }
    LaunchedEffect(pressed, motion) {
        pressScale.animateTo(if (pressed) 0.92f else 1f, motion.of(motion.press))
    }
    LaunchedEffect(fanOpen, motion) {
        stateScale.animateTo(if (fanOpen) 1.06f else 1f, motion.of(motion.fluid))
    }
    LaunchedEffect(fanOpen, selectionMode, motion) {
        val target = if (selectionMode) 45f else if (fanOpen) 135f else 0f
        rotation.animateTo(target, motion.of(motion.liquid))
    }
    LaunchedEffect(selectionMode, motion) {
        if (selectionMode) glassComposed = true
        sel.animateTo(if (selectionMode) 1f else 0f, motion.of(motion.fluid))
        if (!selectionMode) glassComposed = false
    }

    Box(
        modifier
            .size(orbSize)
            .onGloballyPositioned {
                coords[0] = it
                state.orbRect = it.boundsInRoot()
            }
            .semantics {
                role = Role.Button
                this.contentDescription = if (selectionMode) exitDescription else contentDescription
                onClick { onClickState.value(); true }
                if (fanEnabled) {
                    customActions = actions.mapIndexed { i, a ->
                        CustomAccessibilityAction(a.label) { onActionState.value(i); true }
                    }
                }
            }
            .pointerInput(fanEnabled, holdMs) {
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    pressed = true
                    try {
                        if (!fanEnabled) {
                            val up = waitForUpOrCancellation()
                            if (up != null) {
                                up.consume()
                                onClickState.value()
                            }
                            return@awaitEachGesture
                        }
                        var up: PointerInputChange? = null
                        var held = true
                        withTimeoutOrNull(holdMs) {
                            up = waitForUpOrCancellation()
                            held = false
                        }
                        if (!held) {
                            up?.let {
                                it.consume()
                                onClickState.value()
                            }
                            return@awaitEachGesture
                        }
                        haptics.longPress()
                        state.hotIndex = -1
                        state.fanOpen = true
                        while (true) {
                            val event = awaitPointerEvent()
                            val change = event.changes.firstOrNull { it.id == down.id } ?: break
                            val root = coords[0]?.takeIf { it.isAttached }?.localToRoot(change.position)
                            val hot = if (root == null) -1 else state.hitTest(root)
                            if (hot != state.hotIndex) {
                                state.hotIndex = hot
                                if (hot >= 0) haptics.tick()
                            }
                            change.consume()
                            if (!change.pressed) {
                                if (hot >= 0) {
                                    haptics.confirm()
                                    onActionState.value(hot)
                                }
                                break
                            }
                        }
                    } finally {
                        pressed = false
                        state.fanOpen = false
                        state.hotIndex = -1
                    }
                }
            },
    ) {
        Box(
            Modifier
                .fillMaxSize()
                .graphicsLayer {
                    val s = pressScale.value * stateScale.value
                    scaleX = s
                    scaleY = s
                },
        ) {
            // 强调色实心面 + 与坞同一柔和投影（选择模式淡出）。
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer {
                        alpha = 1f - sel.value.coerceIn(0f, 1f)
                        compositingStrategy = CompositingStrategy.ModulateAlpha
                    }
                    .floatingChromeShadow(CircleShape)
                    .background(c.accentFill, CircleShape)
                    .border(0.5.dp, c.onAccent.copy(alpha = 0.16f), CircleShape),
            )
            if (glassComposed) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .graphicsLayer {
                            alpha = sel.value.coerceIn(0f, 1f)
                            compositingStrategy = CompositingStrategy.ModulateAlpha
                        }
                        .floatingChromeSurface(CircleShape),
                )
            }
            Box(
                Modifier
                    .align(Alignment.Center)
                    .size(24.dp)
                    .graphicsLayer { rotationZ = rotation.value },
            ) {
                FluxIcon(
                    FluxIcons.Plus, null,
                    Modifier.graphicsLayer { alpha = 1f - sel.value.coerceIn(0f, 1f) },
                    size = 24.dp, tint = c.onAccent,
                )
                FluxIcon(
                    FluxIcons.Plus, null,
                    Modifier.graphicsLayer { alpha = sel.value.coerceIn(0f, 1f) },
                    size = 24.dp, tint = c.ink,
                )
            }
        }
    }
}

/**
 * 新建球的扇形层：`dim` 遮罩（Real 玻璃下附带模糊）+ 自球上方 14dp 起、间距 60dp、右对齐球右缘的玻璃胶囊。
 *
 * 必须放在应用的**最上层覆盖层**（位于 `fluxBackdropSource` 内容与所有页面之后的兄弟），
 * 与 [FluxOrb] 共用同一个 [state]。层自身不接收手势（长按的手势由球持有）；未展开时不占组合。
 */
@Composable
fun FluxOrbFanLayer(state: FluxOrbState, modifier: Modifier = Modifier) {
    val c = FluxTheme.colors
    val motion = FluxTheme.motion
    val open = state.fanOpen
    val scrim = remember { Animatable(0f) }
    var composed by remember { mutableStateOf(false) }
    LaunchedEffect(open, motion) {
        if (open) {
            composed = true
            scrim.animateTo(1f, motion.of(motion.fluid))
        } else {
            scrim.animateTo(0f, motion.of(motion.snap))
            composed = false
        }
    }
    if (!composed) return
    val blurScrim = LocalFluxGlassMode.current == FluxGlassMode.Blur
    val actions = state.actions
    val itemShape = remember { RoundedCornerShape(26.dp) }

    Box(
        modifier
            .fillMaxSize()
            .onGloballyPositioned { state.layerOrigin = it.positionInRoot() }
            .clearAndSetSemantics { },
    ) {
        Box(
            Modifier
                .fillMaxSize()
                .graphicsLayer { alpha = scrim.value.coerceIn(0f, 1f) }
                .then(
                    if (blurScrim) Modifier.fluxGlass(FluxGlass.G1, RectangleShape, FluxBlur.Thin, FluxGlassKind.Real)
                    else Modifier,
                )
                .background(c.dim)
                .pointerInput(Unit) {
                    awaitPointerEventScope {
                        while (true) awaitPointerEvent().changes.forEach { it.consume() }
                    }
                },
        )
        Layout(
            content = {
                actions.forEachIndexed { i, a ->
                    FanItem(state = state, index = i, action = a, open = open, shape = itemShape)
                }
            },
            modifier = Modifier.fillMaxSize(),
        ) { measurables, constraints ->
            val h = 52.dp.roundToPx()
            val spacing = 60.dp.roundToPx()
            val above = 14.dp.roundToPx()
            val orb = state.orbRect
            val origin = state.layerOrigin
            val placeables = measurables.map {
                it.measure(Constraints(minHeight = h, maxHeight = h, maxWidth = constraints.maxWidth))
            }
            layout(constraints.maxWidth, constraints.maxHeight) {
                val rects = ArrayList<Rect>(placeables.size)
                placeables.forEachIndexed { i, p ->
                    val x = (orb.right - origin.x - p.width).toInt()
                    val y = (orb.top - origin.y - above - i * spacing - h).toInt()
                    p.place(x, y)
                    rects.add(Rect(Offset(x + origin.x, y + origin.y), androidx.compose.ui.geometry.Size(p.width.toFloat(), h.toFloat())))
                }
                state.itemRects = rects
            }
        }
    }
}

@Composable
private fun FanItem(
    state: FluxOrbState,
    index: Int,
    action: FluxOrbAction,
    open: Boolean,
    shape: androidx.compose.ui.graphics.Shape,
) {
    val c = FluxTheme.colors
    val type = FluxTheme.type
    val motion = FluxTheme.motion
    val enter = remember { Animatable(0f) }
    val hotA = remember { Animatable(0f) }
    val hot = state.hotIndex == index
    LaunchedEffect(open, motion) {
        if (open) {
            if (!motion.reduce) delay(30L * index)
            enter.animateTo(1f, motion.of(motion.liquid))
        } else {
            enter.animateTo(0f, motion.of(motion.snap))
        }
    }
    LaunchedEffect(hot, motion) { hotA.animateTo(if (hot) 1f else 0f, motion.of(motion.snap)) }
    val labelStyle = remember(type) { type.weight(type.sm, 600) }

    Row(
        Modifier
            .height(52.dp)
            .graphicsLayer {
                val raw = enter.value
                val e = raw.coerceIn(0f, 1f)
                alpha = e
                translationY = (60 + 30 * index).dp.toPx() * (1f - raw)
                val s = 0.5f + 0.5f * raw
                scaleX = s
                scaleY = s
            }
            .fluxGlass(FluxGlass.Sheet, shape, FluxBlur.Thick, FluxGlassKind.Real)
            .drawWithCache {
                val outline = shape.createOutline(size, layoutDirection, this)
                val hw = 0.5.dp.toPx()
                onDrawBehind {
                    val h = hotA.value
                    if (h > 0.001f) {
                        drawOutline(outline, c.accent.copy(alpha = 0.22f * h))
                        drawOutline(outline, c.accent.copy(alpha = 0.70f * h), style = Stroke(hw))
                    }
                }
            }
            .padding(start = 8.dp, end = 18.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .size(36.dp)
                .drawWithCache {
                    onDrawBehind {
                        drawCircle(lerp(c.glass3, c.accent, hotA.value.coerceIn(0f, 1f)))
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            FluxIcon(
                action.icon, null,
                Modifier.graphicsLayer { alpha = 1f - hotA.value.coerceIn(0f, 1f) },
                size = 20.dp, tint = c.ink,
            )
            FluxIcon(
                action.icon, null,
                Modifier.graphicsLayer { alpha = hotA.value.coerceIn(0f, 1f) },
                size = 20.dp, tint = c.onAccent,
            )
        }
        FluxText(
            action.label,
            modifier = Modifier.padding(start = 10.dp),
            style = labelStyle,
            color = c.ink,
            maxLines = 1,
            softWrap = false,
        )
    }
}
