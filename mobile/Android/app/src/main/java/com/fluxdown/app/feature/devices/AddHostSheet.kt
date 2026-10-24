package com.fluxdown.app.feature.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.fluxdown.app.R
import com.fluxdown.app.i18n.str
import com.fluxdown.app.nav.SheetRoute
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.HostRef
import com.fluxdown.fluxui.controls.ButtonVariant
import com.fluxdown.fluxui.controls.FluxButton
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.controls.FluxPasswordField
import com.fluxdown.fluxui.controls.FluxSegmented
import com.fluxdown.fluxui.controls.SegOption
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.overlay.FluxSheet
import com.fluxdown.fluxui.overlay.FluxSheetDetent
import com.fluxdown.fluxui.overlay.FluxSheetFooter
import com.fluxdown.fluxui.overlay.FluxSheetHeader
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** 退场动画结束后清除内存中的访问密钥。 */
private const val SECRET_CLEAR_DELAY_MS = 600L

/** 打开计数：每次从关闭到打开新建一份表单 / 状态机；退场动画期间沿用同一份（添加主机 / 添加设备共用）。 */
internal class OpenTracker {
    var wasOpen = false
    var generation = 0
}

/** 本次打开时的重新配对目标（退场动画期间路由已清空，沿用打开时的值）。 */
private class RepairHolder {
    var value: HostRef.Remote? = null
}

private enum class HostMode { Pair, Key }

/**
 * V2 添加远程主机（`fluxdown-agent --server` / 开启局域网访问的桌面主机）：
 * - 局域网配对（默认）：发现 / 手输地址 → 配对码 → SAS 核对 → 主机批准 → 设备专属控制凭据，免抄访问密钥（[HostPairingModel]）；
 * - 访问密钥：名称、地址、访问密钥 → 连接测试 → 保存并切换。
 *
 * [route] 的 `repair` 非空 = 重新配对一台已保存、被拒的主机（只有配对模式，地址预填，成功后更新同一主机的凭据）。
 * 表单每次打开重建；密钥不落盘到界面状态之外（持久化加密由 `AppContainer.addRemoteHost` 负责）。
 */
@Composable
fun AddHostSheet(route: SheetRoute.AddHost?, onDismiss: () -> Unit) {
    val visible = route != null
    val tracker = remember { OpenTracker() }
    val repair = remember { RepairHolder() }
    if (visible && !tracker.wasOpen) {
        tracker.generation++
        repair.value = route?.repair
    }
    tracker.wasOpen = visible
    if (tracker.generation == 0) return
    key(tracker.generation) { AddHostSheetImpl(visible, repair.value, onDismiss) }
}

@Stable
private class AddHostForm {
    var name by mutableStateOf("")
    var address by mutableStateOf("")
    var key by mutableStateOf("")

    /** 点过「连接」后才显示本地校验错误，避免输入中途报错。 */
    var attempted by mutableStateOf(false)
    var busy by mutableStateOf(false)

    /** 主机拒绝了这把密钥（Unauthorized）；改动密钥后清除。 */
    var keyRejected by mutableStateOf(false)

    /** 连接失败横幅文案（不可达 / 超时 / 版本不兼容 / 其它）。 */
    var failure by mutableStateOf<String?>(null)
}

@Composable
private fun AddHostSheetImpl(visible: Boolean, repairTarget: HostRef.Remote?, onDismiss: () -> Unit) {
    val container = LocalAppContainer.current
    val context = LocalContext.current
    val haptics = FluxTheme.haptics
    val scope = rememberCoroutineScope()
    val actions = rememberHostActions()
    val form = remember { AddHostForm() }
    val visibleNow by rememberUpdatedState(visible)
    val dismissNow by rememberUpdatedState(onDismiss)
    val actionsNow by rememberUpdatedState(actions)

    var mode by remember { mutableStateOf(HostMode.Pair) }
    val pairMode = repairTarget != null || mode == HostMode.Pair

    val pairModel = remember {
        HostPairingModel(
            container = container,
            scope = container.appScope,
            repair = repairTarget,
            // 主机已保存且用新凭据连通：切换过去并收起面板（切换结果由 toast 回执）
            onConnected = { ref -> actionsNow.switchTo(ref) { if (visibleNow) dismissNow() } },
            onFailure = { haptics.reject() },
        )
    }
    val pairing = pairModel.pairing
    val pairBusy = pairModel.connect is HostConnect.Connecting

    // 配对模式出现 = 开始发现；离开配对模式 / 面板关闭 = 停止发现、放弃未完成的配对（关闭另外释放本机会话）
    LaunchedEffect(visible, pairMode) { if (visible && pairMode) pairModel.activate() else pairModel.deactivate() }
    LaunchedEffect(visible) { if (!visible) pairModel.close() }
    DisposableEffect(pairModel) { onDispose { pairModel.close() } }
    HoldMulticastLock(active = visible && pairMode)

    LaunchedEffect(visible) {
        if (!visible) {
            delay(SECRET_CLEAR_DELAY_MS)
            form.key = ""
        }
    }

    val parsed by remember { derivedStateOf { parseHostEndpoint(form.address) } }
    val keyIssue by remember { derivedStateOf { validateAccessKey(form.key.trim()) } }

    fun submit() {
        if (form.busy) return
        form.attempted = true
        val target = parsed as? EndpointParse.Ok
        val accessKey = form.key.trim()
        if (target == null || keyIssue != null) {
            haptics.reject()
            return
        }
        form.busy = true
        form.failure = null
        form.keyRejected = false
        val name = form.name.trim().ifEmpty { target.host }
        scope.launch {
            container.addRemoteHost(name, target.endpoint, accessKey).fold(
                onSuccess = { ref ->
                    // 主机已保存；无论切换是否成功都收起表单（切换结果由 toast 回执）
                    actions.switchTo(ref) {
                        form.busy = false
                        if (visibleNow) onDismiss()
                    }
                },
                onFailure = { e ->
                    haptics.reject()
                    if ((e as? HostException)?.code == HostErrorCode.Unauthorized) {
                        form.keyRejected = true
                    } else {
                        form.failure = context.hostErrorText(e)
                    }
                    form.busy = false
                },
            )
        }
    }

    val busy = form.busy || pairBusy
    val title = str(if (repairTarget != null) R.string.mobileHostRepair else R.string.mobileHostAddTitle)
    val pairStep = pairing?.step
    val verifying = pairMode && pairing != null && (pairStep is PairingModel.Step.Check || pairStep is PairingModel.Step.Finishing)
    FluxSheet(
        visible = visible,
        onDismissRequest = { if (!busy) onDismiss() },
        detent = if (pairMode) FluxSheetDetent.Full else FluxSheetDetent.Wrap,
        dismissible = !busy,
        title = title,
        header = {
            FluxSheetHeader(
                title = title,
                subtitle = repairTarget?.displayName ?: str(R.string.mobileHostAddSub),
                onClose = if (busy) null else onDismiss,
            )
        },
        footer = when {
            verifying && pairing != null -> ({ VerifyFooter(pairing, pairStep!!) })
            pairMode -> null
            else -> ({
                FluxSheetFooter {
                    FluxButton(
                        str(R.string.cancel),
                        onClick = onDismiss,
                        variant = ButtonVariant.Ghost,
                        enabled = !form.busy,
                        modifier = Modifier.weight(1f),
                    )
                    FluxButton(
                        str(R.string.mobileHostConnect),
                        onClick = ::submit,
                        variant = ButtonVariant.Primary,
                        loading = form.busy,
                        modifier = Modifier.weight(1f),
                    )
                }
            })
        },
    ) {
        Column(Modifier.padding(top = 2.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            if (repairTarget == null && !busy && !verifying) {
                FluxSegmented(
                    options = listOf(
                        SegOption(HostMode.Pair, str(R.string.mobileAddHostModePair)),
                        SegOption(HostMode.Key, str(R.string.mobileAddHostModeKey)),
                    ),
                    selected = mode,
                    onSelect = { mode = it },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            if (pairMode) {
                HostPairContent(model = pairModel)
            } else {
                KeyForm(form = form, parsedAddress = parsed, keyIssue = keyIssue, onSubmit = ::submit)
            }
        }
    }
}

/** 访问密钥模式：名称、地址、访问密钥（原表单）。 */
@Composable
private fun KeyForm(form: AddHostForm, parsedAddress: EndpointParse, keyIssue: KeyIssue?, onSubmit: () -> Unit) {
    val addressError = if (!form.attempted) {
        null
    } else {
        when (parsedAddress) {
            EndpointParse.Empty -> str(R.string.localPairingHostRequired)
            EndpointParse.BadAddress -> str(R.string.localPairingAddressInvalid)
            EndpointParse.BadPort -> str(R.string.localPairingPortInvalid)
            is EndpointParse.Ok -> null
        }
    }
    val keyError = when {
        form.keyRejected -> str(R.string.webLoginInvalidKey)
        !form.attempted -> null
        else -> when (keyIssue) {
            KeyIssue.BadChars -> str(R.string.webKeyBadChars)
            KeyIssue.TooShort -> str(R.string.webKeyTooShort, "min" to ACCESS_KEY_MIN_LEN)
            KeyIssue.TooLong -> str(R.string.webKeyTooLong, "max" to ACCESS_KEY_MAX_LEN)
            KeyIssue.NeedsMix -> str(R.string.webKeyNeedsMix)
            null -> null
        }
    }
    val cleartext = (parsedAddress as? EndpointParse.Ok)?.cleartext == true

    FluxField(
        value = form.name,
        onValueChange = { form.name = it },
        label = str(R.string.mobileHostName),
        placeholder = str(R.string.mobileHostNamePlaceholder),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Next),
        enabled = !form.busy,
    )
    FluxField(
        value = form.address,
        onValueChange = {
            form.address = it
            form.failure = null
        },
        label = str(R.string.mobileHostAddress),
        placeholder = "192.168.1.20:17800",
        hint = str(R.string.mobileHostAddressHint),
        error = addressError,
        mono = true,
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            keyboardType = KeyboardType.Uri,
            imeAction = ImeAction.Next,
        ),
        enabled = !form.busy,
    )
    if (cleartext) {
        FluxBanner(text = str(R.string.mobileHostCleartext), kind = FluxBannerKind.Warn, slim = true)
    }
    FluxPasswordField(
        value = form.key,
        onValueChange = {
            form.key = it
            form.keyRejected = false
            form.failure = null
        },
        label = str(R.string.webAccessKey),
        placeholder = str(R.string.webAccessKeyPlaceholder),
        hint = str(R.string.mobileHostKeyHint, "min" to ACCESS_KEY_MIN_LEN, "max" to ACCESS_KEY_MAX_LEN),
        error = keyError,
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            keyboardType = KeyboardType.Password,
            imeAction = ImeAction.Done,
        ),
        keyboardActions = KeyboardActions(onDone = { onSubmit() }),
        enabled = !form.busy,
    )
    form.failure?.let { FluxBanner(text = it, kind = FluxBannerKind.Error, slim = true) }
}
