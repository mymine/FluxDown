package com.fluxdown.app.feature.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.i18n.str
import com.fluxdown.core.protocol.LinkRules
import com.fluxdown.fluxui.theme.FluxScaleGroup
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme

/**
 * 短认证串 / 配对码的大号分组数字（3+3，等宽，居中；字号随系统字体缩放）。
 * 无障碍：整体作为一个节点，逐位朗读（数字间空格分隔）。
 */
@Composable
internal fun GroupedDigits(digits: String, sizeSp: Float, modifier: Modifier = Modifier, label: String? = null) {
    val type = FluxTheme.type
    val style = remember(type, sizeSp) {
        type.sized(type.weight(type.mono, 600, mono = true), sizeSp, FluxScaleGroup.Kl).copy(textAlign = TextAlign.Center)
    }
    val groups = remember(digits) { LinkRules.groupSas(digits).split(' ') }
    val spoken = remember(digits) { digits.filter { !it.isWhitespace() }.toList().joinToString(" ") }
    Row(
        modifier
            .fillMaxWidth()
            .semantics(mergeDescendants = true) { contentDescription = if (label != null) "$label $spoken" else spoken },
        horizontalArrangement = Arrangement.spacedBy(20.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        groups.forEach { group ->
            FluxText(
                group,
                modifier = Modifier.clearAndSetSemantics { },
                style = style,
                color = FluxTheme.colors.ink,
                maxLines = 1,
                softWrap = false,
            )
        }
    }
}

/** SAS 核对数字（V4 核对页 / V5 入站确认共用）。 */
@Composable
internal fun SasDigits(sas: String, modifier: Modifier = Modifier) =
    GroupedDigits(sas, sizeSp = 44f, modifier = modifier, label = str(R.string.localPairingSasTitle))
