package com.fluxdown.core.model

/**
 * 引擎产生的稳定任务错误文案；`:app` 将 [i18nKey] 映射到 `R.string.<i18nKey>` 本地化展示。
 *
 * 字面量镜像 `native/engine/src/downloader.rs` 的 `POST_ONLY_ENDPOINT_MESSAGE` /
 * `POST_ONLY_RESUME_MESSAGE`（客户端不能依赖引擎）；改动引擎文案须同步此处。
 */
enum class TaskErrorMessage(val raw: String, val i18nKey: String) {
    PostOnlyEndpoint(
        "This link only accepts POST requests (HTTP 405); start the download from the browser",
        "taskErrorPostOnlyEndpoint",
    ),
    PostOnlyResume(
        "This task was started by a browser POST request and cannot be resumed; start it again from the browser",
        "taskErrorPostOnlyResume",
    ),
    ;

    companion object {
        /** 去首尾空白后整串全等匹配；不做前缀/子串匹配。 */
        fun match(raw: String): TaskErrorMessage? {
            val s = raw.trim()
            return entries.firstOrNull { it.raw == s }
        }
    }
}
