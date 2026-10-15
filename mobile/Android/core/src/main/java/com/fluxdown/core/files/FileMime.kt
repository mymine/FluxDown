package com.fluxdown.core.files

/**
 * 打开已下载文件时的 MIME 推断。
 * 优先内置覆盖表（系统 MimeTypeMap 常缺 mkv/flv/m3u8 等，或把 ts 错判为文本），
 * 其次系统查询（由调用方注入，core 不依赖 android.webkit），最后 `*` / `*`。
 */
object FileMime {
    const val UNKNOWN = "*/*"

    fun resolve(fileName: String, system: (String) -> String?): String {
        val ext = extensionOf(fileName) ?: return UNKNOWN
        OVERRIDES[ext]?.let { return it }
        return system(ext)?.takeIf { it.isNotBlank() } ?: UNKNOWN
    }

    /** 最后一个 `.` 之后的小写扩展名；无扩展名（含 `.hidden`、结尾 `.`）→ null。 */
    internal fun extensionOf(fileName: String): String? {
        val name = fileName.substringAfterLast('/')
        val dot = name.lastIndexOf('.')
        if (dot <= 0 || dot == name.length - 1) return null
        return name.substring(dot + 1).lowercase()
    }

    private val OVERRIDES: Map<String, String> = HashMap<String, String>().also { map ->
        fun add(mime: String, vararg exts: String) {
            for (e in exts) map[e] = mime
        }
        // 视频
        add("video/mp4", "mp4", "m4v")
        add("video/x-matroska", "mkv")
        add("video/webm", "webm")
        add("video/x-flv", "flv")
        add("video/quicktime", "mov")
        add("video/x-msvideo", "avi")
        add("video/x-ms-wmv", "wmv")
        add("video/3gpp", "3gp")
        add("video/3gpp2", "3g2")
        add("video/mpeg", "mpg", "mpeg", "vob")
        add("video/mp2t", "ts", "m2ts", "mts")
        add("video/vnd.rn-realvideo", "rmvb", "rm")
        add("video/mp4", "m4s")
        add("application/vnd.apple.mpegurl", "m3u8")
        // 音频
        add("audio/mpeg", "mp3")
        add("audio/mp4", "m4a")
        add("audio/flac", "flac")
        add("audio/ogg", "ogg", "oga", "opus")
        add("audio/x-wav", "wav")
        add("audio/aac", "aac")
        add("audio/x-ms-wma", "wma")
        add("audio/x-ape", "ape")
        add("audio/amr", "amr")
        // 字幕
        add("application/x-subrip", "srt")
        add("text/x-ssa", "ass", "ssa")
        add("text/vtt", "vtt")
        // 图片
        add("image/jpeg", "jpg", "jpeg")
        add("image/png", "png")
        add("image/gif", "gif")
        add("image/webp", "webp")
        // 文档 / 包
        add("application/pdf", "pdf")
        add("application/vnd.android.package-archive", "apk")
        add("application/zip", "zip")
        add("application/x-rar-compressed", "rar")
        add("application/x-7z-compressed", "7z")
        add("application/gzip", "gz")
        add("application/x-tar", "tar")
        add("application/x-bittorrent", "torrent")
    }
}
