package com.fluxdown.app.update

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.util.Log
import androidx.core.content.FileProvider
import androidx.core.content.pm.PackageInfoCompat
import com.fluxdown.app.R
import com.fluxdown.app.i18n.str
import com.fluxdown.core.update.UpdateFailure
import com.fluxdown.core.update.UpdatePolicy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/** [ApkInstaller.prepare] 的结果。 */
internal sealed interface InstallOutcome {
    /** 预检通过：[intent] 由前台界面拉起；[target] 为解析到的处理者（诊断用）。 */
    data class Ready(val intent: Intent, val target: String) : InstallOutcome

    /** 未交接：包校验失败或无法授权读取，应回报给 Rust（[UpdateFailure.Verify] 会让 Rust 删除坏包）。 */
    data class Rejected(val failure: UpdateFailure, val detail: String) : InstallOutcome
}

/**
 * 把已校验的更新包交给安装程序：`ACTION_VIEW` + FileProvider 只读 `content://`，与文件管理器打开 APK 同一入口，
 * 因此走用户设定的默认安装程序；`chooser` = 忽略默认、弹出系统选择器。安装始终由安装程序向用户确认。
 *
 * 不用 `PackageInstaller` 会话：会话的确认界面固定交给系统安装器（无视用户设定的默认安装程序），部分国行 MIUI / HyperOS
 * 安装器对第三方会话直接回 `INSTALL_FAILED_ABORTED`；本应用也未持有 `UPDATE_PACKAGES_WITHOUT_USER_ACTION`，会话同样要确认。
 * 「安装未知应用」授权由安装程序自行引导（第三方安装程序不需要本应用的这项授权），这里不预检。
 */
internal class ApkInstaller(private val context: Context, private val env: InstallEnvironment) {
    /** 预检 → 生成交接 Intent。解析 APK 在 [Dispatchers.IO]。 */
    suspend fun prepare(packagePath: String, chooser: Boolean): InstallOutcome = withContext(Dispatchers.IO) {
        val file = File(packagePath)
        precheck(file)?.let { return@withContext it }
        val uri = try {
            FileProvider.getUriForFile(context, "${context.packageName}.files", file)
        } catch (e: IllegalArgumentException) {
            // 更新目录不在 FileProvider 根内（file_paths.xml 的 app_update 与 noBackupFilesDir 布局不符）
            Log.w(TAG, "update package outside FileProvider roots: ${e.message}")
            return@withContext InstallOutcome.Rejected(UpdateFailure.Install, e.message.orEmpty())
        }
        val view = viewIntent(uri)
        if (chooser) {
            InstallOutcome.Ready(Intent.createChooser(view, context.str(R.string.mobileUpdateChooserTitle)), CHOOSER)
        } else {
            InstallOutcome.Ready(view, describe(view.resolveActivity(context.packageManager)))
        }
    }

    /** 包名必须是自身、签名证书与已安装一致（允许轮换链）、版本不得低于已安装；否则按 Verify 拒绝。 */
    private fun precheck(file: File): InstallOutcome.Rejected? {
        val archive = env.archiveInfo(file.path) ?: return InstallOutcome.Rejected(UpdateFailure.Verify, "unreadable package")
        if (archive.packageName != context.packageName) {
            return InstallOutcome.Rejected(UpdateFailure.Verify, "package name mismatch: ${archive.packageName}")
        }
        val installed = try {
            env.installedInfo()
        } catch (e: PackageManager.NameNotFoundException) {
            Log.w(TAG, "installed package info unavailable, skip precheck: ${e.message}")
            return null
        }
        val installedSigners = InstallEnvironment.signerDigests(installed)
        val archiveSigners = InstallEnvironment.signerDigests(archive)
        val sameSigners = archiveSigners == installedSigners ||
            (installedSigners.isNotEmpty() && InstallEnvironment.signerHistory(archive).containsAll(installedSigners))
        if (!sameSigners) {
            return InstallOutcome.Rejected(UpdateFailure.Verify, "${UpdatePolicy.SIGNATURE_MISMATCH}: installed=$installedSigners package=$archiveSigners")
        }
        val archiveCode = PackageInfoCompat.getLongVersionCode(archive)
        val installedCode = PackageInfoCompat.getLongVersionCode(installed)
        if (archiveCode < installedCode) {
            return InstallOutcome.Rejected(UpdateFailure.Verify, "version downgrade: $archiveCode < $installedCode")
        }
        return null
    }

    companion object {
        private const val TAG = "FluxDown.update"
        private const val APK_MIME = "application/vnd.android.package-archive"
        private const val CHOOSER = "chooser"

        private fun viewIntent(uri: Uri): Intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, APK_MIME)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

        /**
         * 打开 APK 的默认处理者（诊断用，同交接时的解析）：`包名/组件`；装有多个安装程序且没设默认时为系统选择器
         * （`android/…ResolverActivity`）；无人处理为 `none`。
         */
        fun defaultHandler(context: Context): String =
            describe(viewIntent(Uri.parse("content://${context.packageName}.files/probe.apk")).resolveActivity(context.packageManager))

        private fun describe(component: ComponentName?): String = component?.flattenToShortString() ?: "none"
    }
}
