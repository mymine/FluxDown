package com.fluxdown.app.feature.devices

import android.content.Context
import android.net.wifi.WifiManager
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalContext
import com.fluxdown.app.AppContainer
import com.fluxdown.core.host.HostSession
import com.fluxdown.core.store.HostState
import com.fluxdown.core.store.HostStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.StateFlow

/**
 * 对进程内本机主机的独立会话 + 它自己的 [HostStore]：添加主机的配对流程无论当前选中哪台主机，都在本机 agent 上
 * 做发现 / 发起配对（`agent.link.*`）。发现列表（`agent.linkDiscovered`）与已配对名册（`linkDevices`）从 [state] 读。
 * 与主会话互不影响；[close] 只关自己的会话。
 */
internal class LocalLinkChannel private constructor(
    val session: HostSession,
    private val store: HostStore,
    private val scope: CoroutineScope,
) {
    val api = LinkApi(session)

    val state: StateFlow<HostState> get() = store.state

    fun close() {
        store.detach()
        scope.cancel()
        session.close()
    }

    companion object {
        suspend fun open(container: AppContainer): LocalLinkChannel {
            val session = container.openLocalSession()
            // HostStore 要求串行调度（工作副本单线程访问）。
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default.limitedParallelism(1))
            val store = HostStore(scope)
            store.attach(session)
            return LocalLinkChannel(session, store, scope)
        }
    }
}

private const val MULTICAST_LOCK_TAG = "fluxdown-link-discovery"

/**
 * [active] 期间持有 `WifiManager.MulticastLock`：Android 默认在 Wi-Fi 省电模式下丢弃组播包，mDNS 发现收不到应答。
 * [active] 变 false / 离开组合立即释放（需要 `CHANGE_WIFI_MULTICAST_STATE` 权限；拿不到锁只记日志，发现仍可手动输入地址）。
 */
@Composable
internal fun HoldMulticastLock(active: Boolean) {
    val context = LocalContext.current.applicationContext
    DisposableEffect(active) {
        val lock = if (active) acquireMulticastLock(context) else null
        onDispose { lock?.let(::releaseMulticastLock) }
    }
}

private fun acquireMulticastLock(context: Context): WifiManager.MulticastLock? {
    val wifi = context.getSystemService(WifiManager::class.java) ?: return null
    return try {
        wifi.createMulticastLock(MULTICAST_LOCK_TAG).apply {
            setReferenceCounted(false)
            acquire()
        }
    } catch (e: SecurityException) {
        Log.w("Pairing", "multicast lock unavailable: ${e.message}")
        null
    }
}

private fun releaseMulticastLock(lock: WifiManager.MulticastLock) {
    try {
        if (lock.isHeld) lock.release()
    } catch (e: RuntimeException) {
        Log.w("Pairing", "multicast lock release failed: ${e.message}")
    }
}
