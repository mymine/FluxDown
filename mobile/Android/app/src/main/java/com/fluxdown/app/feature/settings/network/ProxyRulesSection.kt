package com.fluxdown.app.feature.settings.network

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.SettingRowBox
import com.fluxdown.app.feature.settings.SettingsCtx
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostException
import com.fluxdown.core.protocol.NetworkProxyMode
import com.fluxdown.core.protocol.NetworkRow
import com.fluxdown.core.protocol.ProxyRule
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxActionRow
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxIconButton
import com.fluxdown.fluxui.controls.FluxListRow
import com.fluxdown.fluxui.controls.FluxSwitch
import com.fluxdown.fluxui.controls.GlassSection
import com.fluxdown.fluxui.controls.IconButtonSize
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.launch

/* 站点代理规则：列表（启用开关 / 排序 / 编辑 / 长按删除）、添加、测试网址。匹配在引擎里，这里只展示 RPC 结果。 */

/** 规则的显示标题：名称，没有名称时用 pattern。 */
internal fun ProxyRule.displayTitle(): String = name.ifEmpty { pattern }

@Composable
internal fun ProxyRulesSection(
    ctx: SettingsCtx,
    model: ProxyRulesModel,
    rules: List<ProxyRule>,
    mode: NetworkProxyMode,
    rulesRaw: String,
    onEdit: (ProxyRule?) -> Unit,
    onFailure: (HostException) -> Unit,
    errorText: (HostException) -> String,
) {
    val context = LocalContext.current
    val container = LocalAppContainer.current
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    val scope = rememberCoroutineScope()
    var reordering by rememberSaveable { mutableStateOf(false) }
    var url by rememberSaveable { mutableStateOf("") }
    val manualReady = ProxyRule.manualConfigured(ctx.form.string("proxy_host"), ctx.form.string("proxy_port"))
    val stamp = "${mode.wire}|$rulesRaw"

    fun write(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: HostException) {
                onFailure(e)
            }
        }
    }

    val cancelText = str(R.string.cancel)
    val deleteText = str(R.string.delete)
    val deleteDesc = str(R.string.proxyRuleDeleteDesc)
    fun confirmDelete(rule: ProxyRule) {
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.proxyRuleDeleteConfirm, "name" to rule.displayTitle()),
                message = deleteDesc,
                icon = FluxIcons.Trash2,
                buttons = listOf(
                    FluxDialogButton(cancelText, FluxDialogButtonStyle.Secondary),
                    FluxDialogButton(deleteText, FluxDialogButtonStyle.Destructive) {
                        write { model.writer.remove(rule.id) }
                    },
                ),
            ),
        )
    }

    GlassSection(title = str(R.string.proxyRulesTitle), footer = str(R.string.proxyRulesScope)) {
        row {
            SettingRowBox(ctx, NetworkRow.Rules.rowId, null) {
                FluxListRow(
                    title = str(R.string.proxyRulesTitle),
                    subtitle = str(R.string.proxyRulesDesc),
                    trailing = if (rules.isEmpty()) {
                        null
                    } else {
                        {
                            FluxButton(
                                text = str(if (reordering) R.string.mobileViewDone else R.string.mobileGeneralReorder),
                                onClick = { reordering = !reordering },
                                variant = ButtonVariant.Ghost,
                                size = ButtonSize.Xs,
                                enabled = !ctx.readOnly,
                            )
                        }
                    },
                )
            }
        }
        if (rules.isEmpty()) {
            custom(padded = true) { EmptyNote(str(R.string.proxyRulesEmptyDesc)) }
        }
        rules.forEachIndexed { index, rule ->
            row {
                val matchLabel = str(if (rule.match == ProxyRule.MATCH_REGEX) R.string.proxyRuleMatchRegexShort else R.string.proxyRuleMatchDomain)
                val actionLabel = when (rule.action) {
                    ProxyRule.ACTION_SYSTEM -> str(R.string.proxyRuleActionSystem)
                    ProxyRule.ACTION_MANUAL -> str(R.string.proxyRuleActionManual)
                    ProxyRule.ACTION_CUSTOM ->
                        "${str(R.string.proxyRuleActionCustom)} · ${ProxyRule.displayEndpoint(rule.proxy)}"
                    else -> str(R.string.proxyRuleActionDirect)
                }
                val parts = mutableListOf(matchLabel, actionLabel)
                if (rule.action == ProxyRule.ACTION_MANUAL && !manualReady) parts += str(R.string.proxyNotConfigured)
                val moveUp = str(R.string.mobileGeneralMoveUp)
                val moveDown = str(R.string.mobileGeneralMoveDown)
                FluxListRow(
                    title = rule.displayTitle(),
                    subtitle = parts.joinToString(" · "),
                    enabled = !ctx.readOnly,
                    onClick = { onEdit(rule) },
                    onLongClick = { confirmDelete(rule) },
                    trailing = {
                        if (reordering) {
                            Row {
                                FluxIconButton(
                                    FluxIcons.ArrowUp, moveUp, { write { model.writer.move(rule.id, -1) } },
                                    size = IconButtonSize.Sm, enabled = index > 0 && !ctx.readOnly,
                                )
                                FluxIconButton(
                                    FluxIcons.ArrowDown, moveDown, { write { model.writer.move(rule.id, 1) } },
                                    size = IconButtonSize.Sm, enabled = index < rules.lastIndex && !ctx.readOnly,
                                )
                            }
                        } else {
                            FluxSwitch(
                                checked = rule.enabled,
                                onCheckedChange = { on -> write { model.writer.setEnabled(rule.id, on) } },
                                enabled = !ctx.readOnly,
                            )
                        }
                    },
                )
            }
        }
        row(hasIcon = true) {
            SettingRowBox(ctx, NetworkRow.RulesAdd.rowId, null) {
                FluxActionRow(
                    str(R.string.proxyRulesAdd),
                    onClick = { onEdit(null) },
                    icon = FluxIcons.Plus,
                    enabled = !ctx.readOnly,
                )
            }
        }
        custom(padded = true) {
            SettingRowBox(ctx, NetworkRow.RulesTest.rowId, null) {
                val state = model.explain
                val running = state == ProxyRulesModel.Explain.Running
                val canCheck = url.isNotBlank() && !running && !ctx.readOnly
                fun run() {
                    if (!canCheck) return
                    val session = container.session
                    val sent = url.trim()
                    val sentRules = rulesRaw
                    val sentStamp = stamp
                    scope.launch {
                        model.explain(session, sent, sentRules, sentStamp, errorText)
                    }
                }
                val done = (state as? ProxyRulesModel.Explain.Done)?.takeIf { it.stamp == stamp }
                LaunchedEffect(done) { if (done != null) haptics.tick() }
                Column {
                    FluxText(str(R.string.proxyRouteTest), style = FluxTheme.type.body)
                    FluxText(str(R.string.proxyRouteTestDesc), style = FluxTheme.type.sm, color = FluxTheme.colors.inkMuted)
                    FluxField(
                        value = url,
                        onValueChange = { url = it },
                        placeholder = str(R.string.proxyRouteUrlPlaceholder),
                        mono = true,
                        enabled = !ctx.readOnly,
                        keyboardOptions = KeyboardOptions(
                            capitalization = KeyboardCapitalization.None,
                            autoCorrectEnabled = false,
                            keyboardType = KeyboardType.Uri,
                            imeAction = ImeAction.Search,
                        ),
                        keyboardActions = KeyboardActions(onSearch = { run() }),
                        trailing = {
                            FluxButton(
                                text = str(if (running) R.string.proxyRouteChecking else R.string.proxyRouteCheck),
                                onClick = { run() },
                                variant = ButtonVariant.Ghost,
                                size = ButtonSize.Xs,
                                enabled = canCheck,
                                loading = running,
                            )
                        },
                    )
                    when (state) {
                        is ProxyRulesModel.Explain.Done ->
                            if (state.stamp == stamp) {
                                proxyRouteLines(state.response).forEach { ResultLine(it.text, it.color) }
                            }
                        is ProxyRulesModel.Explain.Failed ->
                            if (state.stamp == stamp) ResultLine(state.text, FluxTheme.colors.coralText)
                        else -> Unit
                    }
                }
            }
        }
    }
}
