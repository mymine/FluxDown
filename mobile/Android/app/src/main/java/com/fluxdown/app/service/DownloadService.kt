package com.fluxdown.app.service

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.fluxdown.app.AppContainer
import com.fluxdown.app.FluxApplication
import com.fluxdown.app.R
import com.fluxdown.app.i18n.str
import com.fluxdown.core.format.Format
import com.fluxdown.core.model.HostRef
import com.fluxdown.core.store.TransferSummary
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

private const val TAG = "FluxDownService"
private const val CHANNEL_ID = "fluxdown_downloads"
private const val NOTIFICATION_ID = 1001

/** 通知刷新最小间隔（≤ 1 Hz）。 */
private const val THROTTLE_MS = 1_000L

/** 空闲后保留前台的宽限（任务切换 / 队列接力时 active 会瞬间归零），随后自行停止。 */
private const val IDLE_GRACE_MS = 3_000L

/**
 * 本机下载前台服务（`dataSync`）：本机引擎有活跃 / 排队 / 待重试任务时存在，让进程在后台继续下载——
 * 与当前选中的主机无关（远端主机在前台时，进程内的本机下载照样在跑）。
 * 通知常驻、静默，展示本机下载进度（单任务进度条 / 多任务聚合进度 + 展开明细，见 [buildNotification]），
 * 点按回到应用；本机空闲后自行停止。
 *
 * 事件驱动：只收集 [AppContainer.localActivity]（本机主机状态投影），无周期轮询；通知刷新按 [THROTTLE_MS] 节流，
 * 空闲宽限由新的活动量到来而取消。启动入口见 [DownloadServiceController.start]；
 * 停止由服务自己决定，因此 Activity 被销毁后也不会残留前台通知。
 */
class DownloadService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var watcher: Job? = null
    private var lastShownAtMs = 0L

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        ensureChannel(this)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // startForegroundService 之后必须在数秒内 startForeground：先用当前量发出，再进入观察
        val container = (application as FluxApplication).container
        show(container.localActivity.value)
        // 完成 / 失败通知的观察在应用作用域内：下载期间即使 Activity 已被划掉也不中断
        DownloadNotifier.attach(container, applicationContext)
        if (watcher?.isActive != true) {
            watcher = scope.launch { watch(container) }
        }
        return START_NOT_STICKY
    }

    private suspend fun watch(container: AppContainer) {
        container.localActivity.collectLatest { activity ->
            if (activity.busy) {
                val wait = THROTTLE_MS - (SystemClock.elapsedRealtime() - lastShownAtMs)
                if (wait > 0) delay(wait)
                show(activity)
            } else {
                delay(IDLE_GRACE_MS)
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                watcher?.cancel()
            }
        }
    }

    /** Android 15+：dataSync 前台服务 6 小时上限；系统回调后必须立即停止，回到前台时由 [DownloadServiceEffect] 重新拉起。 */
    override fun onTimeout(startId: Int, fgsType: Int) {
        Log.w(TAG, "foreground service timed out (type=$fgsType); stopping until the app returns to foreground")
        watcher?.cancel()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    /** 重复调用 `startForeground` 即更新通知，且不受通知权限影响；服务类型参数 Android 10 以下由 ServiceCompat 忽略。 */
    private fun show(activity: LocalActivity) {
        lastShownAtMs = SystemClock.elapsedRealtime()
        ServiceCompat.startForeground(this, NOTIFICATION_ID, buildNotification(this, activity), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    }
}

/** 渠道只创建一次（已存在则跳过）；LOW = 静默、不弹出。 */
private fun ensureChannel(context: Context) {
    val nm = context.getSystemService(NotificationManager::class.java)
    if (nm.getNotificationChannel(CHANNEL_ID) != null) return
    val channel = NotificationChannel(CHANNEL_ID, context.str(R.string.mobileDlNotifChannel), NotificationManager.IMPORTANCE_LOW).apply {
        description = context.str(R.string.mobileDlNotifChannelDesc)
        setShowBadge(false)
    }
    nm.createNotificationChannel(channel)
}

/**
 * 进度通知：
 * - 1 个活跃任务：标题 = 文件名，进度条 = 该任务进度（大小未知为不确定态），正文 = 速度 · 已下 / 总量 · 剩余时间；
 * - 多个活跃任务：标题 = 「N 个任务下载中」，进度条 = 大小已知任务的聚合进度（[TransferSummary]），
 *   展开后逐行列出前 [DETAIL_LINES] 个任务的进度与速度，其余折叠为「等 N 个文件」；
 * - 只有排队 / 待重试：标题说明等待，无进度条。
 * 页眉副文本放百分比，折叠态也能一眼看到进度；点按单任务通知直达该任务详情。
 */
private fun buildNotification(context: Context, a: LocalActivity): android.app.Notification {
    val transfers = a.transfers
    val single = transfers.singleOrNull()
    val builder = NotificationCompat.Builder(context, CHANNEL_ID)
        .setSmallIcon(R.drawable.ic_stat_download)
        .setColor(NotificationTint)
        .setContentIntent(NotificationIntents.pending(context, HostRef.Local.ID, single?.taskId.orEmpty(), code = NOTIFICATION_ID))
        .setOngoing(true)
        .setOnlyAlertOnce(true)
        .setSilent(true)
        .setShowWhen(false)
        .setCategory(NotificationCompat.CATEGORY_PROGRESS)
        .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
    val waiting = if (a.waiting > 0) context.str(R.string.mobileDlNotifWaiting, "n" to a.waiting) else null
    when {
        single != null -> {
            val fraction = single.fraction
            builder.setContentTitle(single.fileName.ifEmpty { context.str(R.string.mobileDlNotifActive, "n" to 1) })
                .setContentText(
                    listOfNotNull(
                        speedLabel(context, single.speedBps),
                        sizeLabel(single.downloadedBytes, single.totalBytes),
                        single.etaSeconds?.let { remainingLabel(context, it) },
                        waiting,
                    ).joinToString(SEP),
                )
            applyProgress(builder, fraction)
        }
        transfers.isNotEmpty() -> {
            val summary = TransferSummary.of(transfers)
            val headline = listOfNotNull(
                speedLabel(context, a.downBps),
                summary.etaSeconds?.let { remainingLabel(context, it) },
                waiting,
            ).joinToString(SEP)
            val lines = transfers.take(DETAIL_LINES).map { t ->
                val progress = t.fraction?.let(::percentLabel) ?: Format.bytes(t.downloadedBytes).toString()
                listOf(t.fileName, progress, Format.speedOrZero(t.speedBps).toString()).joinToString(SEP)
            }
            val more = transfers.size - DETAIL_LINES
            val body = buildList {
                add(headline)
                addAll(lines)
                if (more > 0) add(context.str(R.string.andMoreFiles, "count" to more))
            }.joinToString("\n")
            builder.setContentTitle(context.str(R.string.mobileDlNotifActive, "n" to transfers.size))
                .setContentText(headline)
                .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            applyProgress(builder, summary.fraction)
        }
        else -> {
            val title = if (a.active > 0) {
                context.str(R.string.mobileDlNotifActive, "n" to a.active)
            } else {
                context.str(R.string.mobileDlNotifWaiting, "n" to a.waiting)
            }
            builder.setContentTitle(title)
            if (a.active > 0) builder.setContentText(listOfNotNull(speedLabel(context, a.downBps), waiting).joinToString(SEP))
        }
    }
    return builder.build()
}

private const val SEP = " · "

/** 多任务通知展开后最多列出的任务行数。 */
private const val DETAIL_LINES = 5

private const val PROGRESS_MAX = 1000

/** 进度条 + 页眉百分比；大小未知为不确定态（不显示百分比）。 */
private fun applyProgress(builder: NotificationCompat.Builder, fraction: Float?) {
    if (fraction == null) {
        builder.setProgress(0, 0, true)
    } else {
        builder.setProgress(PROGRESS_MAX, (fraction * PROGRESS_MAX).toInt(), false)
            .setSubText(percentLabel(fraction))
    }
}

private fun percentLabel(fraction: Float): String = "${(fraction * 100).toInt()}%"

private fun speedLabel(context: Context, bps: Long): String =
    context.str(R.string.mobileDlNotifSpeed, "speed" to Format.speedOrZero(bps).toString())

/** `已下 / 总量`；大小未知只给已下载量。 */
private fun sizeLabel(downloaded: Long, total: Long): String =
    if (total > 0) "${Format.bytes(downloaded)} / ${Format.bytes(total)}" else Format.bytes(downloaded).toString()

/** 剩余时间：`<60s` 秒、`<1h` 分钟（向上取整）、其余 `h m`（与任务详情同一组文案）。 */
private fun remainingLabel(context: Context, seconds: Long): String {
    val time = when {
        seconds < 60 -> context.str(R.string.etaSeconds, "n" to seconds)
        seconds < 3600 -> context.str(R.string.etaMinutes, "n" to (seconds + 59) / 60)
        else -> context.str(R.string.etaHours, "n" to seconds / 3600) + " " + context.str(R.string.etaMinutes, "n" to seconds % 3600 / 60)
    }
    return context.str(R.string.mobileDlNotifRemaining, "time" to time)
}

/**
 * 前台服务启动入口与通知权限（Android 13+）。
 * 只在应用处于前台时调用（Android 12+ 禁止后台启动前台服务）。
 */
object DownloadServiceController {
    private const val PREFS = "fluxdown_service"
    private const val KEY_NOTIF_ASKED = "notif_permission_asked"

    /** 拉起（或刷新）前台服务。后台启动被系统拒绝时仅记录日志，服务若已在运行则不受影响。 */
    fun start(context: Context) {
        try {
            ContextCompat.startForegroundService(context, Intent(context, DownloadService::class.java))
        } catch (e: IllegalStateException) {
            Log.w(TAG, "foreground service start refused: ${e.message}")
        }
    }

    /** 需要请求通知权限：Android 13+、尚未授予、且从未请求过（避免每次冷启动打扰；拒绝后服务照常运行）。 */
    fun shouldRequestNotificationPermission(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return false
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) return false
        return !context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_NOTIF_ASKED, false)
    }

    /** 记录已请求过（无论用户同意与否）。 */
    fun markNotificationPermissionAsked(context: Context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_NOTIF_ASKED, true).apply()
    }
}
