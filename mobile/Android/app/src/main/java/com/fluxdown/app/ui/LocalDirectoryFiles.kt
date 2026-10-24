package com.fluxdown.app.ui

import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.InvalidPathException

private val localDirectoryOrder = Comparator<File> { left, right ->
    val order = left.name.compareTo(right.name, ignoreCase = true)
    if (order != 0) order else left.name.compareTo(right.name)
}

private fun normalizedLocalDirectory(path: String): File {
    if (path.isBlank()) throw IOException("Directory path must not be blank")
    val directory = File(path)
    if (!directory.isAbsolute) throw IOException("Directory path must be absolute: $path")
    return try {
        directory.toPath().normalize().toFile()
    } catch (failure: InvalidPathException) {
        throw IOException("Invalid directory path: $path", failure)
    }
}

/**
 * Lists only immediate, non-hidden directories; an unreadable directory is an error, not an empty listing.
 * Hidden folders (`.trash-storage`, `.thumbnails`…) are never sensible download targets; a path can still be typed.
 */
internal fun listLocalDirectories(path: String): List<File> {
    val directory = normalizedLocalDirectory(path)
    val children = try {
        directory.listFiles { child -> !child.name.startsWith('.') && child.isDirectory }
    } catch (failure: SecurityException) {
        throw IOException("Cannot read directory: ${directory.path}", failure)
    } ?: throw IOException("Cannot read directory: ${directory.path}")
    return children.sortedWith(localDirectoryOrder)
}

/** Closest existing directory at or above [path] (start point when a saved folder was deleted); null if none. */
internal fun nearestExistingDirectory(path: String): File? {
    var current: File? = try {
        normalizedLocalDirectory(path)
    } catch (_: IOException) {
        return null
    }
    while (current != null) {
        if (current.isDirectory) return current
        current = current.parentFile
    }
    return null
}

/** Why a new folder name is rejected; null = acceptable. */
internal enum class FolderNameError { Blank, Invalid, TooLong }

internal fun validateFolderName(name: String): FolderNameError? {
    val trimmed = name.trim()
    return when {
        trimmed.isEmpty() -> FolderNameError.Blank
        trimmed == "." || trimmed == ".." || trimmed.any { it == '/' || it == '\u0000' } -> FolderNameError.Invalid
        trimmed.toByteArray(Charsets.UTF_8).size > 255 -> FolderNameError.TooLong
        else -> null
    }
}

/** Creates [name] under [parent]; an existing directory is reused, an existing file is an error. */
internal fun createChildDirectory(parent: String, name: String): String {
    val error = validateFolderName(name)
    if (error != null) throw IOException("Invalid folder name: $error")
    val base = normalizedLocalDirectory(parent)
    val child = File(base, name.trim())
    try {
        if (child.isDirectory) return child.path
        if (child.exists()) throw IOException("A file with this name exists: ${child.path}")
        if (!child.mkdir() && !child.isDirectory) throw IOException("Cannot create directory: ${child.path}")
    } catch (failure: SecurityException) {
        throw IOException("Cannot create directory: ${child.path}", failure)
    }
    return child.path
}

/** Creates a directory if needed and probes actual file creation, writing and removal. */
internal fun prepareLocalDirectory(path: String): String {
    try {
        val directory = normalizedLocalDirectory(path)
        if (!directory.isDirectory && !directory.mkdirs() && !directory.isDirectory) {
            throw IOException("Cannot create directory: ${directory.path}")
        }
        val probe = Files.createTempFile(directory.toPath(), ".fluxdown-write-", ".tmp")
        try {
            Files.newOutputStream(probe).use { it.write(0) }
        } catch (failure: Exception) {
            try {
                Files.delete(probe)
            } catch (cleanupFailure: Exception) {
                failure.addSuppressed(cleanupFailure)
            }
            throw failure
        }
        Files.delete(probe)
        return directory.path
    } catch (failure: SecurityException) {
        throw IOException("Cannot access directory: $path", failure)
    }
}

/**
 * Prepares every distinct non-blank directory in [paths] (see [prepareLocalDirectory]) so folders exist before
 * any download lands there; returns the paths that could not be created or written, in input order.
 */
internal fun prepareLocalDirectories(paths: List<String>): List<String> =
    paths.map { it.trim() }.filter { it.isNotEmpty() }.distinct().filter { path ->
        try {
            prepareLocalDirectory(path)
            false
        } catch (_: IOException) {
            true
        }
    }

/** Path components, rather than string prefixes, define shared-storage membership. */
internal fun isPathInStorageRoot(path: String, root: File): Boolean {
    val directory = File(path)
    if (!directory.isAbsolute || !root.isAbsolute) return false
    return try {
        directory.toPath().normalize().startsWith(root.toPath().normalize())
    } catch (_: InvalidPathException) {
        false
    }
}

/**
 * Android 11+ hides every app's `Android/data` and `Android/obb` even with all-files access; the only
 * reachable entries are the app's own roots. Browsing there would show an empty or failing listing.
 */
internal fun isProtectedAppDataPath(path: String, primaryRoot: File, volumeRoots: List<File>, appRoots: List<File>): Boolean {
    val directory = sharedStoragePath(path, primaryRoot) ?: return false
    if (appRoots.any { root -> sharedStoragePath(root.path, primaryRoot)?.let { isPathInStorageRoot(directory.path, it) } == true }) {
        return false
    }
    return (volumeRoots + primaryRoot).any { root ->
        isPathInStorageRoot(directory.path, File(root, "Android/data")) || isPathInStorageRoot(directory.path, File(root, "Android/obb"))
    }
}

/** One breadcrumb: [path] null = the storage-location list above every volume. */
internal class PathCrumb(val label: String, val path: String?)

/**
 * Breadcrumbs from the storage list to [path]. A path inside a known volume starts at that volume
 * (labelled by [volumeRoots]) so users never walk into unlistable `/storage/emulated`; others start at `/`.
 */
internal fun pathCrumbs(path: String, rootLabel: String, volumeRoots: List<Pair<String, File>>): List<PathCrumb> {
    val directory = File(path).toPath().normalize().toFile()
    val volume = volumeRoots
        .filter { (_, root) -> isPathInStorageRoot(directory.path, root) }
        .maxByOrNull { (_, root) -> root.path.length }
    val crumbs = ArrayList<PathCrumb>()
    crumbs += PathCrumb(rootLabel, null)
    var base: File
    if (volume != null) {
        base = volume.second
        crumbs += PathCrumb(volume.first, base.path)
    } else {
        base = File("/")
        crumbs += PathCrumb("/", "/")
    }
    val rest = base.toPath().relativize(directory.toPath())
    for (segment in rest) {
        if (segment.toString().isEmpty()) continue
        base = File(base, segment.toString())
        crumbs += PathCrumb(segment.toString(), base.path)
    }
    return crumbs
}

private fun sharedStoragePath(path: String, primaryRoot: File): File? {
    val candidate = File(path)
    if (!candidate.isAbsolute) return null
    return try {
        val candidatePath = candidate.toPath()
        val primary = primaryRoot.toPath()
        val sdcard = File("/sdcard").toPath()
        val selfPrimary = File("/storage/self/primary").toPath()
        when {
            candidatePath.startsWith(sdcard) -> primary.resolve(sdcard.relativize(candidatePath))
            candidatePath.startsWith(selfPrimary) -> primary.resolve(selfPrimary.relativize(candidatePath))
            else -> candidatePath
        }.normalize().toFile()
    } catch (_: InvalidPathException) {
        null
    }
}

/** Includes the volume browser root but excludes the app's permission-free private storage. */
internal fun isSharedStorageAccessPath(
    path: String,
    primaryRoot: File,
    volumeRoots: List<File>,
    appRoots: List<File>,
): Boolean {
    val directory = sharedStoragePath(path, primaryRoot) ?: return false
    if (appRoots.any { root ->
            sharedStoragePath(root.path, primaryRoot)?.let { isPathInStorageRoot(directory.path, it) } == true
        }
    ) return false
    return directory == File("/storage") ||
        isPathInStorageRoot(directory.path, primaryRoot) ||
        volumeRoots.any { isPathInStorageRoot(directory.path, it) }
}

/**
 * 展示用路径:`/storage/emulated/0/Download/视频` → `内部存储 › Download › 视频`。
 * [roots] 为 (标签, 根路径);取最长匹配根,`/sdcard` 视为主存储别名;无匹配时原样返回。
 * 仅用于显示,保存与校验仍用真实绝对路径。
 */
internal fun friendlyPath(path: String, roots: List<Pair<String, String>>): String {
    val trimmed = path.trim()
    if (trimmed.isEmpty()) return trimmed
    val normalized = (if (trimmed == "/sdcard" || trimmed.startsWith("/sdcard/")) "/storage/emulated/0" + trimmed.removePrefix("/sdcard") else trimmed)
        .trimEnd('/')
    val hit = roots
        .filter { (_, root) -> root.isNotEmpty() && (normalized == root.trimEnd('/') || normalized.startsWith(root.trimEnd('/') + "/")) }
        .maxByOrNull { (_, root) -> root.trimEnd('/').length }
        ?: return trimmed
    val rest = normalized.removePrefix(hit.second.trimEnd('/')).split('/').filter { it.isNotEmpty() }
    return (listOf(hit.first) + rest).joinToString(" › ")
}
