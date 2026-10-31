package com.fluxdown.app.feature.settings.network

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.PickerSpec
import com.fluxdown.app.feature.settings.SettingsCtx
import com.fluxdown.app.feature.settings.rememberLastNonNull
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostException
import com.fluxdown.core.protocol.ProxyRule
import com.fluxdown.core.protocol.ProxyRuleIssue
import com.fluxdown.core.protocol.upsertRule
import com.fluxdown.fluxui.controls.ButtonSize
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxSegmented
import com.fluxdown.fluxui.controls.FluxSelect
import com.fluxdown.fluxui.controls.SegOption
import com.fluxdown.fluxui.controls.SelectOption
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.FluxPortal
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetDetent
import com.fluxdown.fluxui.overlay.FluxSheetFooter
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.launch

/* 规则编辑器（同 WebhookPage 的编辑 Sheet）：检测语义见计划第 7 步「编辑器的检测语义」。 */

/** 编辑目标；[nonce] 让相同规则的重复打开重建表单。 */
internal class ProxyRuleTarget(val seed: ProxyRule, val isNew: Boolean, val nonce: Long)

@Stable
private class EditorForm(seed: ProxyRule, val isNew: Boolean) {
    val ruleId = seed.id
    val enabled = seed.enabled
    var name by mutableStateOf(seed.name)
    var match by mutableStateOf(seed.match)
    var pattern by mutableStateOf(seed.pattern)
    var action by mutableStateOf(seed.action)
    var proxy by mutableStateOf(seed.proxy)
    var sample by mutableStateOf("")
    var issues by mutableStateOf<List<ProxyRuleIssue>>(emptyList())

    /** 试一试：null = 未检测 / 有错误。 */
    var tryResult by mutableStateOf<Boolean?>(null)
    var formError by mutableStateOf<String?>(null)
    var checking by mutableStateOf(false)

    val baseline: ProxyRule = buildDraft()

    fun buildDraft() = ProxyRule(
        id = ruleId,
        enabled = enabled,
        name = name.trim(),
        match = match,
        pattern = pattern.trim(),
        action = action,
        proxy = if (action == ProxyRule.ACTION_CUSTOM) proxy.trim() else "",
    )

    val isDirty get() = buildDraft() != baseline

    fun clearIssues(field: String) {
        issues = issues.filter { it.field != field }
        tryResult = null
        formError = null
    }

    /** 不属于任何输入框的 issue（显示在底部）。 */
    val bottomIssues get() = issues.filter { it.field !in INPUT_FIELDS }

    companion object {
        val INPUT_FIELDS = setOf("name", "pattern", "proxy")
    }
}

@Composable
internal fun ProxyRuleEditorSheet(
    target: ProxyRuleTarget?,
    ctx: SettingsCtx,
    model: ProxyRulesModel,
    rules: List<ProxyRule>,
    errorText: (HostException) -> String,
    onDismiss: () -> Unit,
    onSaved: () -> Unit,
) {
    val spec = rememberLastNonNull(target)
    FluxPortal {
        if (spec != null) {
            androidx.compose.runtime.key(spec.nonce) {
                EditorSheetImpl(
                    visible = target != null,
                    target = spec,
                    ctx = ctx,
                    model = model,
                    rules = rules,
                    errorText = errorText,
                    onDismiss = onDismiss,
                    onSaved = onSaved,
                )
            }
        }
    }
}

@Composable
private fun EditorSheetImpl(
    visible: Boolean,
    target: ProxyRuleTarget,
    ctx: SettingsCtx,
    model: ProxyRulesModel,
    rules: List<ProxyRule>,
    errorText: (HostException) -> String,
    onDismiss: () -> Unit,
    onSaved: () -> Unit,
) {
    val overlays = LocalFluxOverlays.current
    val context = LocalContext.current
    val container = LocalAppContainer.current
    val scope = rememberCoroutineScope()
    val form = remember { EditorForm(target.seed, target.isNew) }
    var saving by remember { mutableStateOf(false) }
    val busy = form.checking || saving
    val c = FluxTheme.colors

    val manualReady = ProxyRule.manualConfigured(ctx.form.string("proxy_host"), ctx.form.string("proxy_port"))
    val manualEndpoint = "${ctx.form.string("proxy_type", "http")}://${ctx.form.string("proxy_host")}:${ctx.form.string("proxy_port")}"

    fun requestClose() {
        if (saving) return
        if (!form.isDirty) {
            onDismiss()
            return
        }
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.mobileWebhookDiscardTitle),
                icon = FluxIcons.TriangleAlert,
                buttons = listOf(
                    FluxDialogButton(context.str(R.string.mobileWebhookKeepEditing)),
                    FluxDialogButton(context.str(R.string.mobileWebhookDiscard), FluxDialogButtonStyle.Destructive) { onDismiss() },
                ),
            ),
        )
    }

    /**
     * 检测。失焦 / 试一试：`{url: sample, rules: [draft]}`，只看下标 0；
     * 保存：`{url: "", rules: upsert(当前列表, draft)}`，看 draft 在 next 中的下标与 field 为空的 issue，无 issue 才写入。
     */
    fun runCheck(thenSave: Boolean) {
        if (busy) return
        val draft = form.buildDraft()
        val list = if (thenSave) upsertRule(rules, draft) else listOf(draft.copy(enabled = true))
        val index = list.indexOfFirst { it.id == draft.id }.coerceAtLeast(0)
        val sample = if (thenSave) "" else form.sample.trim()
        val session = container.session
        form.checking = true
        form.formError = null
        scope.launch {
            try {
                val r = model.check(session, list, sample)
                val mine = r.issues.filter { it.index == index || it.field.isEmpty() }
                form.issues = mine
                form.tryResult = if (!thenSave && sample.isNotEmpty() && mine.isEmpty()) r.matched?.index == 0 else null
                if (thenSave && mine.isEmpty()) {
                    saving = true
                    try {
                        model.writer.upsert(draft)
                        onSaved()
                        onDismiss()
                    } catch (e: HostException) {
                        form.formError = errorText(e)
                    } finally {
                        saving = false
                    }
                }
            } catch (e: HostException) {
                form.formError = errorText(e)
            } finally {
                form.checking = false
            }
        }
    }

    fun confirmDelete() {
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.proxyRuleDeleteConfirm, "name" to form.buildDraft().displayTitle()),
                message = context.str(R.string.proxyRuleDeleteDesc),
                icon = FluxIcons.Trash2,
                buttons = listOf(
                    FluxDialogButton(context.str(R.string.cancel), FluxDialogButtonStyle.Secondary),
                    FluxDialogButton(context.str(R.string.delete), FluxDialogButtonStyle.Destructive) {
                        scope.launch {
                            try {
                                model.writer.remove(form.ruleId)
                                onDismiss()
                            } catch (e: HostException) {
                                form.formError = errorText(e)
                            }
                        }
                    },
                ),
            ),
        )
    }

    val title = str(if (form.isNew) R.string.proxyRuleNew else R.string.proxyRuleEdit)
    FluxSheet(
        visible = visible,
        onDismissRequest = ::requestClose,
        detent = FluxSheetDetent.Full,
        dismissible = !saving,
        title = title,
        header = { FluxSheetHeader(title = title, onClose = if (saving) null else ::requestClose) },
        footer = {
            FluxSheetFooter {
                if (!form.isNew) {
                    FluxButton(
                        str(R.string.delete),
                        onClick = ::confirmDelete,
                        variant = ButtonVariant.Danger,
                        icon = FluxIcons.Trash2,
                        enabled = !busy,
                        modifier = Modifier.weight(1f),
                    )
                }
                FluxButton(
                    str(R.string.proxyRuleSave),
                    onClick = { runCheck(true) },
                    variant = ButtonVariant.Primary,
                    enabled = !busy,
                    loading = busy,
                    modifier = Modifier.weight(1f),
                )
            }
        },
    ) {
        val isRegex = form.match == ProxyRule.MATCH_REGEX
        val nameError = form.issues.firstOrNull { it.field == "name" }?.let { proxyIssueText(it) }
        val patternError = form.issues.firstOrNull { it.field == "pattern" }?.let { proxyIssueText(it) }
        val proxyError = form.issues.firstOrNull { it.field == "proxy" }?.let { proxyIssueText(it) }
        val actionLabel = str(R.string.proxyRuleAction)
        val actionOptions = ProxyRule.actions.map { wire ->
            when (wire) {
                ProxyRule.ACTION_SYSTEM -> SelectOption(wire, str(R.string.proxyRuleActionSystem), str(R.string.proxyRuleActionSystemHint))
                ProxyRule.ACTION_MANUAL -> SelectOption(
                    wire,
                    str(R.string.proxyRuleActionManual),
                    if (manualReady) str(R.string.proxyRuleActionManualHint, "endpoint" to manualEndpoint) else str(R.string.proxyRuleActionManualMissing),
                )
                ProxyRule.ACTION_CUSTOM -> SelectOption(wire, str(R.string.proxyRuleActionCustom), str(R.string.proxyRuleProxyUrlHint))
                else -> SelectOption(wire, str(R.string.proxyRuleActionDirect), str(R.string.proxyRuleActionDirectHint))
            }
        }
        Column(Modifier.padding(top = 2.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            FluxField(
                value = form.name,
                onValueChange = {
                    form.name = it
                    form.clearIssues("name")
                },
                label = str(R.string.proxyRuleName),
                placeholder = str(R.string.proxyRuleNamePlaceholder),
                error = nameError,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Words),
                enabled = !busy,
            )
            FluxSegmented(
                options = listOf(
                    SegOption(ProxyRule.MATCH_DOMAIN, str(R.string.proxyRuleMatchDomain)),
                    SegOption(ProxyRule.MATCH_REGEX, str(R.string.proxyRuleMatchRegex)),
                ),
                selected = form.match,
                onSelect = {
                    form.match = it
                    form.clearIssues("pattern")
                },
            )
            FluxField(
                value = form.pattern,
                onValueChange = {
                    form.pattern = it
                    form.clearIssues("pattern")
                },
                label = str(if (isRegex) R.string.proxyRuleMatchRegex else R.string.proxyRuleMatchDomain),
                placeholder = str(if (isRegex) R.string.proxyRulePatternRegexPlaceholder else R.string.proxyRulePatternDomainPlaceholder),
                hint = str(if (isRegex) R.string.proxyRulePatternRegexHint else R.string.proxyRulePatternDomainHint),
                error = patternError,
                mono = true,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = KeyboardType.Uri),
                onFocusChange = { focused -> if (!focused && form.pattern.isNotBlank()) runCheck(false) },
                enabled = !busy,
            )
            FluxSelect(
                value = actionOptions.first { it.value == form.action }.label,
                onClick = {
                    ctx.openPicker(
                        PickerSpec(actionLabel, actionOptions, form.action) {
                            form.action = it
                            form.clearIssues("proxy")
                        },
                    )
                },
                label = actionLabel,
                hint = actionOptions.first { it.value == form.action }.hint?.takeIf { form.action != ProxyRule.ACTION_MANUAL || manualReady },
                enabled = !busy,
            )
            if (form.action == ProxyRule.ACTION_MANUAL && !manualReady) {
                FluxBanner(str(R.string.proxyRuleActionManualMissing), kind = FluxBannerKind.Warn, slim = true)
            }
            if (form.action == ProxyRule.ACTION_CUSTOM) {
                FluxField(
                    value = form.proxy,
                    onValueChange = {
                        form.proxy = it
                        form.clearIssues("proxy")
                    },
                    label = str(R.string.proxyRuleProxyUrl),
                    placeholder = str(R.string.proxyRuleProxyUrlPlaceholder),
                    hint = str(R.string.proxyRuleProxyUrlHint),
                    error = proxyError,
                    mono = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = KeyboardType.Uri),
                    onFocusChange = { focused -> if (!focused && form.proxy.isNotBlank()) runCheck(false) },
                    enabled = !busy,
                )
            }
            FluxField(
                value = form.sample,
                onValueChange = {
                    form.sample = it
                    form.tryResult = null
                },
                label = str(R.string.proxyRuleTry),
                placeholder = str(R.string.proxyRouteUrlPlaceholder),
                mono = true,
                keyboardOptions = KeyboardOptions(
                    capitalization = KeyboardCapitalization.None,
                    autoCorrectEnabled = false,
                    keyboardType = KeyboardType.Uri,
                    imeAction = ImeAction.Search,
                ),
                keyboardActions = KeyboardActions(onSearch = { if (form.sample.isNotBlank()) runCheck(false) }),
                trailing = {
                    FluxButton(
                        text = str(R.string.proxyRouteCheck),
                        onClick = { runCheck(false) },
                        variant = ButtonVariant.Ghost,
                        size = ButtonSize.Xs,
                        enabled = form.sample.isNotBlank() && !busy,
                        loading = form.checking,
                    )
                },
                enabled = !busy,
            )
            form.tryResult?.let { hit ->
                FluxText(
                    str(if (hit) R.string.proxyRuleTryHit else R.string.proxyRuleTryMiss),
                    style = FluxTheme.type.sm,
                    color = if (hit) c.mintText else c.inkMuted,
                )
            }
            form.bottomIssues.firstOrNull()?.let {
                FluxBanner(proxyIssueText(it), kind = FluxBannerKind.Error, slim = true)
            }
            form.formError?.let { FluxBanner(it, kind = FluxBannerKind.Error, slim = true) }
        }
    }
}
