package com.fluxdown.app.feature.devices

import android.content.Context
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import com.fluxdown.app.AppContainer
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.account.AccountErrorLabel
import com.fluxdown.app.feature.settings.account.AccountSheet
import com.fluxdown.app.feature.settings.account.AccountText
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.LinkDevice
import com.fluxdown.core.protocol.AccountErrorContext
import com.fluxdown.core.protocol.LinkRules
import com.fluxdown.fluxui.controls.FluxField
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.FluxOverlayState
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxHaptics
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * 已配对设备（直连）的行内动作：刷新在线状态、解除配对（确认框）。
 * 列表本身来自 `LinkedDevicesChanged` 推送，动作成功后无需手动回写；失败 toast 本地化原因。
 */
@Stable
internal class LinkedDeviceActions(
    private val container: AppContainer,
    private val context: Context,
    private val scope: CoroutineScope,
    private val overlays: FluxOverlayState,
    private val haptics: FluxHaptics,
) {
    var refreshing by mutableStateOf(false)
        private set

    /** 探测全部已配对设备在线状态（`agent.link.refresh`）。 */
    fun refresh() {
        if (refreshing) return
        refreshing = true
        val api = LinkApi(container.session)
        scope.launch {
            try {
                api.refresh()
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                fail(e)
            } finally {
                refreshing = false
            }
        }
    }

    /** 解除配对：确认框说明双方都会解除（对方在线时同步解除）。 */
    fun confirmRemove(device: LinkDevice) {
        haptics.longPress()
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.linkedDeviceRemoveTitle),
                message = context.str(R.string.linkedDeviceRemoveDesc, "name" to device.name),
                icon = FluxIcons.Trash2,
                buttons = listOf(
                    FluxDialogButton(context.str(R.string.cancel)),
                    FluxDialogButton(context.str(R.string.linkedDeviceRemove), FluxDialogButtonStyle.Destructive) { remove(device) },
                ),
            ),
        )
    }

    private fun remove(device: LinkDevice) {
        val api = LinkApi(container.session)
        scope.launch {
            try {
                api.remove(device.fingerprint)
                haptics.confirm()
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                fail(e)
            }
        }
    }

    private fun fail(e: HostException) {
        haptics.reject()
        overlays.toast(AccountText.error(context, e, AccountErrorContext.Pairing), FluxToastKind.Error, FluxIcons.CircleAlert)
    }
}

@Composable
internal fun rememberLinkedDeviceActions(): LinkedDeviceActions {
    val container = LocalAppContainer.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    return remember(container, context, scope, overlays, haptics) {
        LinkedDeviceActions(container, context, scope, overlays, haptics)
    }
}

/**
 * 重命名已配对设备（`agent.link.rename`，仅本机显示）：留空 = 恢复对方设备名，否则 1–64 字符且无控制字符。
 * [target] 为 null 时收起；输入预填当前备注名，占位显示当前生效名。
 */
@Composable
internal fun LinkRenameSheet(target: LinkDevice?, readOnly: Boolean, onDismiss: () -> Unit) {
    // 退场动画期间保留最后一个目标，避免内容瞬间消失
    var last by remember { mutableStateOf<LinkDevice?>(null) }
    if (target != null) last = target
    val shown = last ?: return
    val container = LocalAppContainer.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val haptics = FluxTheme.haptics

    var value by remember(shown.fingerprint) { mutableStateOf(shown.customName.orEmpty()) }
    var busy by remember { mutableStateOf(false) }
    var errorText by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(target?.fingerprint) {
        if (target != null) {
            value = target.customName.orEmpty()
            errorText = null
        }
    }

    val check = LinkRules.checkRename(value)
    val invalid = check == LinkRules.RenameCheck.Invalid
    val unchanged = (check as? LinkRules.RenameCheck.Name)?.name == shown.customName ||
        (check == LinkRules.RenameCheck.Clear && shown.customName == null)

    fun save() {
        if (busy || invalid || unchanged) return
        val name = (check as? LinkRules.RenameCheck.Name)?.name
        val api = LinkApi(container.session)
        busy = true
        errorText = null
        scope.launch {
            try {
                api.rename(shown.fingerprint, name)
                haptics.confirm()
                onDismiss()
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                errorText = AccountText.error(context, e, AccountErrorContext.Pairing)
                haptics.reject()
            } finally {
                busy = false
            }
        }
    }

    AccountSheet(
        visible = target != null,
        title = str(R.string.accountDeviceRenameTitle),
        onDismiss = onDismiss,
        confirmText = str(R.string.confirm),
        canConfirm = !invalid && !unchanged,
        onConfirm = ::save,
        busy = busy,
        readOnly = readOnly,
    ) {
        FluxField(
            value = value,
            onValueChange = {
                value = it
                errorText = null
            },
            placeholder = shown.name,
            hint = str(R.string.linkedDeviceRenameHint),
            error = if (invalid) str(R.string.accountDeviceRenameInvalid) else null,
            enabled = !busy,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { save() }),
        )
        errorText?.let { AccountErrorLabel(it) }
    }
}
