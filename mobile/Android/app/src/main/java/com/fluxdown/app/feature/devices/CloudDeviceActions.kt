package com.fluxdown.app.feature.devices

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.fluxdown.app.AppContainer
import com.fluxdown.app.R
import com.fluxdown.app.feature.settings.account.AccountApi
import com.fluxdown.app.feature.settings.account.AccountText
import com.fluxdown.app.i18n.str
import com.fluxdown.app.shell.LocalAppContainer
import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.CloudDevice
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.material.FluxGlass
import com.fluxdown.fluxui.material.FluxGlassKind
import com.fluxdown.fluxui.material.fluxGlass
import com.fluxdown.fluxui.overlay.FluxBanner
import com.fluxdown.fluxui.overlay.FluxBannerKind
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.FluxOverlayState
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import com.fluxdown.fluxui.theme.FluxHaptics
import com.fluxdown.fluxui.theme.FluxText
import com.fluxdown.fluxui.theme.FluxTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * 账号已信任设备（云端）的删除：确认框（目标设备卡片；删除本机时加「将立即退出登录」警示）→ `agent.device.delete`。
 *
 * 快照里的 [CloudDevice] 没有云端行 id，删除前经 `agent.device.list` 取完整记录按 deviceId 解析（同 iOS `DevicesModel`）；
 * 该调用同时刷新名册，因此记录已不存在（别处已删除）时行自然消失，按成功处理。离线设备同样可删：删除只经 FluxCloud，
 * 不需要连上目标设备；目标下次上线时会话已失效，需重新验证邮箱登录。
 * 删除本机时 agent 随即清除本机会话，账户相关界面自动回到未登录态。
 */
@Stable
internal class CloudDeviceActions(
    private val container: AppContainer,
    private val context: Context,
    private val scope: CoroutineScope,
    private val overlays: FluxOverlayState,
    private val haptics: FluxHaptics,
) {
    /** 删除进行中的 deviceId：行内显示加载圈并屏蔽重复触发。 */
    var deleting by mutableStateOf<Set<String>>(emptySet())
        private set

    /**
     * [online] = 行上展示的在线状态（null = 状态未知），确认卡片与行保持一致。
     * 调用封装绑定打开确认框时的主机会话：确认前切换了主机，删除也不会落到新主机的账号上。
     */
    fun confirmDelete(device: CloudDevice, online: Boolean?) {
        if (device.deviceId in deleting) return
        val api = AccountApi(container.session)
        haptics.longPress()
        overlays.showDialog(
            FluxDialogSpec(
                title = context.str(R.string.accountDeviceDeleteConfirmTitle),
                message = context.str(R.string.accountDeviceDeleteConfirmDesc),
                icon = FluxIcons.Trash2,
                body = {
                    DeleteTargetCard(device, online)
                    if (device.isCurrent) {
                        FluxBanner(
                            str(R.string.accountDeviceDeleteCurrentWarning),
                            Modifier.fillMaxWidth().padding(top = 10.dp),
                            kind = FluxBannerKind.Warn,
                        )
                    }
                },
                buttons = listOf(
                    FluxDialogButton(context.str(R.string.cancel)),
                    FluxDialogButton(context.str(R.string.accountDeviceDeleteAction), FluxDialogButtonStyle.Destructive) {
                        delete(api, device)
                    },
                ),
            ),
        )
    }

    private fun delete(api: AccountApi, device: CloudDevice) {
        val deviceId = device.deviceId
        if (deviceId in deleting) return
        deleting = deleting + deviceId
        scope.launch {
            try {
                val rowId = api.deviceList().firstOrNull { it.deviceId == deviceId }?.id
                val deleted = rowId != null && deleteRow(api, rowId)
                haptics.confirm()
                // 只有本次真的删掉了本机，agent 才会清会话；别处先删掉的情况不报「已退出登录」。
                if (deleted && device.isCurrent) {
                    overlays.toast(context.str(R.string.mobileDeviceDeletedSignedOut), FluxToastKind.Info, FluxIcons.LogOut)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: HostException) {
                haptics.reject()
                overlays.toast(AccountText.error(context, e), FluxToastKind.Error, FluxIcons.CircleAlert)
            } finally {
                deleting = deleting - deviceId
            }
        }
    }

    /**
     * 删除成功 = true。列表与删除之间被别处删掉（云端 404）：重新拉一次名册让快照去掉该行，返回 false
     * （结果对用户等同删除成功，但本机会话不由这次调用清除）。
     */
    private suspend fun deleteRow(api: AccountApi, rowId: String): Boolean {
        try {
            api.deviceDelete(rowId)
            return true
        } catch (e: HostException) {
            if (e.code != HostErrorCode.NotFound) throw e
            api.deviceList()
            return false
        }
    }
}

@Composable
internal fun rememberCloudDeviceActions(): CloudDeviceActions {
    val container = LocalAppContainer.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val overlays = LocalFluxOverlays.current
    val haptics = FluxTheme.haptics
    return remember(container, context, scope, overlays, haptics) {
        CloudDeviceActions(container, context, scope, overlays, haptics)
    }
}

/** 确认框里的目标设备卡片：平台图块 + 名称 + 在线状态 / 平台 / 版本，避免在同名或相似设备间误删。 */
@Composable
private fun DeleteTargetCard(device: CloudDevice, online: Boolean?) {
    val c = FluxTheme.colors
    val t = FluxTheme.type
    val meta = listOfNotNull(
        str(
            when (online) {
                true -> R.string.deviceOnline
                false -> R.string.deviceOffline
                null -> R.string.devicePresenceUnknown
            },
        ),
        platformLabel(device.platform),
        device.appVersion?.takeIf { it.isNotBlank() }?.let { "v$it" },
    ).joinToString(" · ")
    Row(
        Modifier
            .fillMaxWidth()
            .fluxGlass(FluxGlass.G2, FluxTheme.shapes.tile, kind = FluxGlassKind.Flat)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        GlyphTile(platformIcon(device.platform), tint = if (online == true) c.ink else c.inkMuted, size = 32.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            FluxText(device.name, style = t.weight(t.body, 600), color = c.ink, maxLines = 1)
            FluxText(meta, style = t.sm, color = c.inkMuted, maxLines = 1)
        }
    }
}
