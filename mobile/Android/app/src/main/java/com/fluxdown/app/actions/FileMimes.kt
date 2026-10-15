package com.fluxdown.app.actions

import android.webkit.MimeTypeMap
import com.fluxdown.core.files.FileMime

/** 打开已下载文件的 MIME：内置覆盖表优先，其次系统 MimeTypeMap，最后 `*` / `*`。 */
internal fun mimeForFile(fileName: String): String =
    FileMime.resolve(fileName) { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it) }
