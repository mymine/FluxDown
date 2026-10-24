package com.fluxdown.app.feature.settings

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.withFrameNanos
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.i18n.str
import com.fluxdown.fluxui.chrome.FluxPageHead
import com.fluxdown.fluxui.controls.FluxPickList
import com.fluxdown.fluxui.controls.IconButtonSize
import com.fluxdown.fluxui.controls.FluxIconButton
import com.fluxdown.fluxui.controls.SelectOption
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.FlowInGate
import com.fluxdown.fluxui.material.fluxFlowIn
import com.fluxdown.fluxui.overlay.FluxPortal
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.FluxOverlayState
import com.fluxdown.fluxui.theme.FluxTheme

/** 设置页在 medium / expanded 档的内容最大宽度（§13）。 */
internal val PageMaxWidth = 640.dp

/** 推入页头高度：上 6 + 钮 44 + 下 10。 */
private val PageHeadHeight = 60.dp

/** 区块间距（GlassSection 之间）。 */
internal val SectionGap = 20.dp

/**
 * 推入式设置页骨架：内容 [LazyColumn] 从状态栏 + 页头之下起排并可滚到页头之下，
 * [FluxPageHead]（返回 → [onBack]）悬浮其上，滚动后背后渐隐。底部留白 `pageClearance`。
 * 设置搜索的定位请求（[SettingsFocus.pendingItemKey]）在这里消费：滚动到 key 相同的项（[flowItem] / `item(key)`）。
 */
@Composable
internal fun SettingsPageFrame(
    title: String,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    state: LazyListState = rememberLazyListState(),
    content: LazyListScope.() -> Unit,
) {
    val margin = FluxTheme.space.screenMargin
    val statusTop = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val navBottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    val scrolled by remember(state) { derivedStateOf { state.canScrollBackward } }
    val keys = remember { ArrayList<Any?>() }
    val focus = LocalSettingsFocus.current

    LaunchedEffect(focus.pendingItemKey) {
        val target = focus.pendingItemKey ?: return@LaunchedEffect
        // 内容可能随配置加载才出现：逐帧查找，最多约 0.5s。
        repeat(FOCUS_LOOKUP_FRAMES) {
            withFrameNanos { }
            val index = keys.indexOf(target)
            if (index >= 0) {
                state.animateScrollToItem(index)
                focus.pendingItemKey = null
                return@LaunchedEffect
            }
        }
        focus.pendingItemKey = null
    }

    Box(modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        Box(Modifier.widthIn(max = PageMaxWidth).fillMaxWidth().fillMaxHeight()) {
            LazyColumn(
                state = state,
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(
                    start = margin,
                    end = margin,
                    top = statusTop + PageHeadHeight + 4.dp,
                    bottom = navBottom + FluxTheme.space.pageClearance,
                ),
                verticalArrangement = Arrangement.spacedBy(SectionGap),
            ) {
                keys.clear()
                KeyRecordingScope(this, keys).content()
            }
            FluxPageHead(
                title = title,
                modifier = Modifier.padding(top = statusTop),
                onLead = onBack,
                scrolled = scrolled,
                backDescription = str(R.string.back),
            )
        }
    }
}

private const val FOCUS_LOOKUP_FRAMES = 30

/** 记录各项 key 的顺序（定位用）；`items` 无 key 时按 null 占位，保持索引对齐。 */
private class KeyRecordingScope(
    private val delegate: LazyListScope,
    private val keys: MutableList<Any?>,
) : LazyListScope by delegate {
    override fun item(key: Any?, contentType: Any?, content: @Composable LazyItemScope.() -> Unit) {
        keys += key
        delegate.item(key, contentType, content)
    }

    override fun items(
        count: Int,
        key: ((index: Int) -> Any)?,
        contentType: (index: Int) -> Any?,
        itemContent: @Composable LazyItemScope.(index: Int) -> Unit,
    ) {
        for (i in 0 until count) keys += key?.invoke(i)
        delegate.items(count, key, contentType, itemContent)
    }
}

/** 列表项 + 首次进入的“流入”（同 key 在 [gate] 生命周期内只播放一次）。 */
internal fun LazyListScope.flowItem(
    index: Int,
    gate: FlowInGate,
    key: String,
    content: @Composable () -> Unit,
) {
    item(key = key) {
        Box(Modifier.fluxFlowIn(index, gate, key)) { content() }
    }
}

/** 选择器描述：标题 + 单选项；点选即提交并关闭。 */
internal class PickerSpec(
    val title: String,
    val options: List<SelectOption<String>>,
    val selected: String?,
    val subtitle: String? = null,
    val onSelect: (String) -> Unit,
)

private class Last<T>(var value: T? = null)

/** 记住最近一个非空值：Sheet 退场动画期间内容保持不变。 */
@Composable
internal fun <T : Any> rememberLastNonNull(current: T?): T? {
    val holder = remember { Last<T>() }
    if (current != null) holder.value = current
    return current ?: holder.value
}

/** 浮动选择器 Sheet（标题 + 单选列表）；[picker] = null 时收起。经 FluxPortal 在舞台浮层层组合。 */
@Composable
internal fun PickerSheetHost(picker: PickerSpec?, onDismiss: () -> Unit) {
    val spec = rememberLastNonNull(picker)
    val closeDescription = str(R.string.close)
    FluxPortal {
        FluxSheet(
            visible = picker != null,
            onDismissRequest = onDismiss,
            title = spec?.title,
            header = {
                if (spec != null) {
                    FluxSheetHeader(
                        title = spec.title,
                        subtitle = spec.subtitle,
                        actions = {
                            FluxIconButton(FluxIcons.X, closeDescription, onDismiss, size = IconButtonSize.Sm)
                        },
                    )
                }
            },
        ) {
            if (spec != null) {
                FluxPickList(
                    options = spec.options,
                    selected = spec.selected,
                    onSelect = { value ->
                        spec.onSelect(value)
                        onDismiss()
                    },
                )
            }
        }
    }
}

/** 以 ACTION_VIEW 打开链接；没有可处理的应用时 toast 提示。 */
internal fun openLink(context: Context, overlays: FluxOverlayState, url: String, failText: String) {
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    try {
        context.startActivity(intent)
    } catch (e: ActivityNotFoundException) {
        overlays.toast(failText, FluxToastKind.Error)
    }
}

/** 启动系统设置类 Intent；设备不支持时 toast 提示。 */
internal fun startSettingsIntent(context: Context, overlays: FluxOverlayState, intent: Intent, failText: String) {
    try {
        context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (e: ActivityNotFoundException) {
        overlays.toast(failText, FluxToastKind.Error)
    }
}
