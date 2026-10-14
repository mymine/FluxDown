package com.fluxdown.app.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import com.fluxdown.app.R
import com.fluxdown.app.i18n.fill
import com.fluxdown.app.i18n.str
import com.fluxdown.fluxui.icons.FluxIcons
import com.fluxdown.fluxui.overlay.FluxDialogButton
import com.fluxdown.fluxui.overlay.FluxDialogButtonStyle
import com.fluxdown.fluxui.overlay.FluxDialogSpec
import com.fluxdown.fluxui.overlay.FluxToastKind
import com.fluxdown.fluxui.overlay.LocalFluxOverlays
import java.io.File

/**
 * 本机主机的默认保存目录：应用专属外部目录（无需存储权限，卸载时随应用清理）。
 * 与首次启动播种给引擎的 `default_save_dir` 同源，“恢复默认”也回到这里。
 */
fun defaultLocalSaveDir(context: Context): File =
    context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: File(context.filesDir, "downloads")

/**
 * 系统公共下载目录下的应用子目录（`/storage/emulated/0/Download/FluxDown`）。
 * Android 11+ 分区存储允许应用不申请权限直接在 Download/ 下创建任意类型文件（仅能管理自己创建的文件）；
 * Android 10 及以下需要存储权限（见 [canRequestStorageAccess]）。
 */
fun publicDownloadSaveDir(): File =
    File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "FluxDown")

/** 按路径写入共享存储的授权：Android 11+ 为「所有文件访问」，更低版本为存储运行时权限（Android 10 另靠 legacy 存储）。 */
private fun hasSharedStorageAccess(context: Context): Boolean =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        Environment.isExternalStorageManager()
    } else {
        ContextCompat.checkSelfPermission(context, Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
    }

/** 共享存储里的目录不可写且尚未授权：可引导授权后再试。 */
fun canRequestStorageAccess(context: Context, path: String): Boolean =
    path.startsWith(Environment.getExternalStorageDirectory().path) && !hasSharedStorageAccess(context)

/**
 * 共享存储授权流程（调用方已判定 [canRequestStorageAccess]）：返回的函数以目标目录弹出说明对话框，
 * 「去授权」后 Android 11+ 跳系统「所有文件访问」页（返回后由用户重新选择目录）；
 * 更低版本直接申请存储权限，授予后以同一目录回调 [onGranted]，拒绝则提示该目录不可写。
 */
@Composable
fun rememberStorageAccessPrompt(onGranted: (String) -> Unit): (String) -> Unit {
    val context = LocalContext.current
    val overlays = LocalFluxOverlays.current
    val legacy = Build.VERSION.SDK_INT < Build.VERSION_CODES.R
    val title = str(if (legacy) R.string.mobileStoragePermTitle else R.string.mobileAllFilesTitle)
    val desc = str(if (legacy) R.string.mobileStoragePermDesc else R.string.mobileAllFilesDescNative)
    val grant = str(R.string.mobileGoGrant)
    val cancel = str(R.string.cancel)
    val notWritable = str(R.string.mobileSaveDirNotWritable)
    val granted by rememberUpdatedState(onGranted)
    var pending by remember { mutableStateOf<String?>(null) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        val dir = pending ?: return@rememberLauncherForActivityResult
        pending = null
        if (ok) granted(dir) else overlays.toast(notWritable.fill("dir" to dir), FluxToastKind.Error)
    }
    return { dir ->
        overlays.showDialog(
            FluxDialogSpec(
                title = title,
                message = desc,
                icon = FluxIcons.FolderOpen,
                buttons = listOf(
                    FluxDialogButton(cancel),
                    FluxDialogButton(grant, FluxDialogButtonStyle.Primary) {
                        if (legacy) {
                            pending = dir
                            permission.launch(Manifest.permission.WRITE_EXTERNAL_STORAGE)
                        } else {
                            context.startActivity(
                                Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${context.packageName}")),
                            )
                        }
                    },
                ),
            ),
        )
    }
}

/**
 * 本机主机的保存目录能否写入（不存在则尝试创建）。
 *
 * Android 分区存储下，应用无权限的目录（如拼错的 `Android/dta/...`、其他应用的私有目录）
 * 创建 / 写入会得到 EPERM，引擎只能在下载开始后报 `Operation not permitted`；提交前在这里拦住。
 * 远端主机的路径在服务器上，不能在手机侧校验。
 */
fun isLocalDirWritable(path: String): Boolean {
    if (path.isBlank()) return true
    val dir = File(path.trim())
    if (!dir.isAbsolute) return false
    return (dir.isDirectory || dir.mkdirs()) && dir.canWrite()
}

/** SAF 目录树 URI → 绝对路径（`primary:Download/X` → `/storage/emulated/0/Download/X`）；无法映射为 null。 */
fun treeUriToPath(uri: Uri): String? {
    val docId = runCatching { DocumentsContract.getTreeDocumentId(uri) }.getOrNull() ?: return null
    val volume = docId.substringBefore(':')
    val rel = docId.substringAfter(':', "").trim('/')
    val base = when {
        volume == "primary" -> "/storage/emulated/0"
        volume.isNotEmpty() && docId.contains(':') && volume != "raw" && volume != "home" -> "/storage/$volume"
        volume == "raw" -> return rel.takeIf { docId.substringAfter(':').startsWith("/") }?.let { "/$it" }
        else -> return null
    }
    return if (rel.isEmpty()) base else "$base/$rel"
}
