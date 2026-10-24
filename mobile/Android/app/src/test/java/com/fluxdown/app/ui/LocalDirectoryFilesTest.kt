package com.fluxdown.app.ui

import java.io.File
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class LocalDirectoryFilesTest {
    @get:Rule
    val temporary = TemporaryFolder()

    @Test
    fun publicDownloadRootIsAcceptedWithoutAnAppSubdirectory() {
        val download = temporary.newFolder("Download")

        assertEquals(download.absolutePath, prepareLocalDirectory(download.path))
        assertTrue(isSharedStorageAccessPath("/storage/emulated/0/Download", primary, volumes, appRoots))
        assertTrue(isSharedStorageAccessPath(primary.path, primary, volumes, appRoots))
    }

    @Test
    fun createsNestedDirectoriesWithSpacesAndChineseNames() {
        val directory = File(temporary.root, "我的 下载/视频 文件/已完成")

        assertEquals(directory.absolutePath, prepareLocalDirectory(directory.path))
        assertTrue(directory.isDirectory)
        assertEquals(emptyList<File>(), directory.listFiles()!!.toList())
    }

    @Test
    fun returnsNormalizedAbsolutePathAndRemovesProbeFiles() {
        val directory = File(temporary.root, "Download")
        val input = File(temporary.root, "unused/../Download/.")

        assertEquals(directory.absolutePath, prepareLocalDirectory(input.path))
        assertTrue(directory.isDirectory)
        assertFalse(File(temporary.root, "unused").exists())
        assertEquals(emptyList<File>(), directory.listFiles()!!.toList())
    }

    @Test
    fun rejectsFilesAndFileAncestorsAsDirectoriesWithoutChangingContents() {
        val file = temporary.newFile("not-a-directory")
        file.writeText("keep this file")

        assertIOException { prepareLocalDirectory(file.path) }
        assertIOException { prepareLocalDirectory(File(file, "child").path) }
        assertFalse(isLocalDirWritable(file.path))
        assertEquals("keep this file", file.readText())
    }

    @Test
    fun batchPreparationCreatesEveryCategoryFolderAndReportsOnlyFailures() {
        val base = temporary.newFolder("Download")
        val blocker = File(base, "Other")
        blocker.writeText("a file named like the folder")
        val video = File(base, "Video").path
        val archive = File(base, "Archive").path

        val failed = prepareLocalDirectories(listOf("", video, " $video ", archive, blocker.path))

        assertEquals(listOf(blocker.path), failed)
        assertTrue(File(video).isDirectory)
        assertTrue(File(archive).isDirectory)
        assertEquals("a file named like the folder", blocker.readText())
    }

    @Test
    fun rejectsBlankRelativeAndInvalidPaths() {
        for (path in listOf("", "  ", "Download", "../Download", "/invalid\u0000path")) {
            assertIOException { prepareLocalDirectory(path) }
            assertIOException { listLocalDirectories(path) }
        }
        assertFalse(isLocalDirWritable("Download"))
        assertFalse(isLocalDirWritable("/invalid\u0000path"))
    }

    @Test
    fun writablePathPreservesDefaultAndSurroundingWhitespaceSemantics() {
        assertTrue(isLocalDirWritable(""))
        assertTrue(isLocalDirWritable("  "))
        val directory = temporary.newFolder("Download")
        assertTrue(isLocalDirWritable("  ${directory.path}  "))
    }

    @Test
    fun enumeratesOnlyVisibleImmediateDirectoriesInStableCaseInsensitiveOrder() {
        val parent = temporary.newFolder("listing")
        for (name in listOf("zoo", "Beta", ".hidden", "alpha", "中文")) {
            assertTrue(File(parent, name).mkdir())
        }
        assertTrue(File(parent, "alpha/nested").mkdir())
        File(parent, "aardvark.txt").writeText("not a directory")

        val expected = listOf("alpha", "Beta", "zoo", "中文")
        assertEquals(expected, listLocalDirectories(parent.path).map { it.name })
    }

    @Test
    fun friendlyPathReplacesVolumeRootWithItsNameAndPrefersMostSpecificRoot() {
        val roots = listOf(
            "内部存储" to "/storage/emulated/0",
            "SD 卡" to "/storage/ABCD-1234",
            "应用目录" to "/storage/emulated/0/Android/data/com.fluxdown.app/files",
        )
        assertEquals("内部存储 › Download", friendlyPath("/storage/emulated/0/Download", roots))
        assertEquals("内部存储 › Download › 视频", friendlyPath("/sdcard/Download/视频/", roots))
        assertEquals("内部存储", friendlyPath("/storage/emulated/0", roots))
        assertEquals("SD 卡 › Movies", friendlyPath("/storage/ABCD-1234/Movies", roots))
        assertEquals("应用目录 › Download", friendlyPath("/storage/emulated/0/Android/data/com.fluxdown.app/files/Download", roots))
        assertEquals("/storage/emulated/01/x", friendlyPath("/storage/emulated/01/x", roots))
        assertEquals("", friendlyPath("  ", roots))
    }

    @Test
    fun nearestExistingDirectoryFallsBackToClosestParent() {
        val existing = temporary.newFolder("Download")

        assertEquals(existing, nearestExistingDirectory(File(existing, "gone/deeper").path))
        assertEquals(existing, nearestExistingDirectory(existing.path))
        assertEquals(null, nearestExistingDirectory("relative/path"))
        assertFalse(File(existing, "gone").exists())
    }

    @Test
    fun folderNamesRejectSeparatorsDotsAndOverlongInput() {
        assertEquals(null, validateFolderName(" 视频 2026 "))
        assertEquals(FolderNameError.Blank, validateFolderName("   "))
        for (name in listOf(".", "..", "a/b", "/", "x\u0000y")) {
            assertEquals(name, FolderNameError.Invalid, validateFolderName(name))
        }
        assertEquals(FolderNameError.TooLong, validateFolderName("中".repeat(86)))
    }

    @Test
    fun createChildDirectoryReusesFoldersAndRefusesFilesOrEscapes() {
        val parent = temporary.newFolder("parent")
        val created = createChildDirectory(parent.path, " 新建 ")
        assertEquals(File(parent, "新建").path, created)
        assertEquals(created, createChildDirectory(parent.path, "新建"))

        File(parent, "taken").writeText("file")
        assertIOException { createChildDirectory(parent.path, "taken") }
        assertIOException { createChildDirectory(parent.path, "..") }
        assertIOException { createChildDirectory(parent.path, "a/b") }
        assertEquals("file", File(parent, "taken").readText())
        assertFalse(File(temporary.root, "a").exists())
    }

    @Test
    fun otherAppsDataAndObbAreProtectedButOwnRootsAreNot() {
        for (path in listOf(
            "/storage/emulated/0/Android/data",
            "/storage/emulated/0/Android/data/com.other.app",
            "/sdcard/Android/obb/com.other.app",
            "/storage/ABCD-1234/Android/data",
        )) {
            assertTrue(path, isProtectedAppDataPath(path, primary, volumes, appRoots))
        }
        for (path in listOf(
            "/storage/emulated/0/Android",
            "/storage/emulated/0/Android/media",
            "/storage/emulated/0/Android/data/com.fluxdown.app/files/Download",
            "/storage/emulated/0/Download",
        )) {
            assertFalse(path, isProtectedAppDataPath(path, primary, volumes, appRoots))
        }
    }

    @Test
    fun breadcrumbsStartAtTheContainingVolumeNotItsUnlistableParents() {
        val labelled = listOf("内部存储" to primary, "SD" to File("/storage/ABCD-1234"))

        val download = pathCrumbs("/storage/emulated/0/Download/视频", "存储位置", labelled)
        assertEquals(listOf("存储位置", "内部存储", "Download", "视频"), download.map { it.label })
        assertEquals(listOf(null, "/storage/emulated/0", "/storage/emulated/0/Download", "/storage/emulated/0/Download/视频"), download.map { it.path })

        assertEquals(listOf("存储位置", "SD"), pathCrumbs("/storage/ABCD-1234/", "存储位置", labelled).map { it.label })
        assertEquals(listOf("存储位置", "/", "data", "local"), pathCrumbs("/data/local", "存储位置", labelled).map { it.label })
    }

    @Test
    fun listingErrorsAreNotReportedAsEmptyDirectories() {
        val missing = File(temporary.root, "missing")
        val file = temporary.newFile("file")

        assertIOException { listLocalDirectories(missing.path) }
        assertIOException { listLocalDirectories(file.path) }
        assertIOException { listLocalDirectories(File(file, "child").path) }
        assertFalse(missing.exists())
        assertEquals(emptyList<File>(), listLocalDirectories(temporary.newFolder("empty").path))
    }

    @Test
    fun sharedStorageSupportsRemovableVolumesAndTheVolumeBrowserRoot() {
        for (path in listOf("/storage", "/storage/ABCD-1234", "/storage/ABCD-1234/Download")) {
            assertTrue(path, isSharedStorageAccessPath(path, primary, volumes, appRoots))
        }
    }

    @Test
    fun similarPrefixesAndParentTraversalDoNotGrantSharedStorageMembership() {
        for (path in listOf(
            "/storage-extra",
            "/storage/emulated/01/Download",
            "/storage/ABCD-12345/Download",
            "/storage/ABCD-1234/../other/Download",
            "/storage/emulated/0/../../private",
            "/sdcard-backup/Download",
            "storage/emulated/0/Download",
            "/invalid\u0000path",
        )) {
            assertFalse(path, isSharedStorageAccessPath(path, primary, volumes, appRoots))
        }
    }

    @Test
    fun primaryAliasesAndNormalizedPathsUseTheSamePermissionRules() {
        for (path in listOf(
            "/sdcard",
            "/sdcard/Download",
            "/storage/self/primary/Download",
            "/sdcard/Download/../视频",
            "/storage/emulated/0/Download/../视频",
        )) {
            assertTrue(path, isSharedStorageAccessPath(path, primary, volumes, appRoots))
        }
        assertFalse(isSharedStorageAccessPath("/sdcard/../private", primary, volumes, appRoots))
    }

    @Test
    fun ownExternalFilesAndCacheNeverRequireSharedStoragePermission() {
        for (path in listOf(
            "/storage/emulated/0/Android/data/com.fluxdown.app/files/Download",
            "/storage/emulated/0/Android/data/com.fluxdown.app/cache",
            "/storage/ABCD-1234/Android/data/com.fluxdown.app/files/Download",
            "/sdcard/Android/data/com.fluxdown.app/files/Download",
            "/storage/self/primary/Android/data/com.fluxdown.app/files/Download",
            "/sdcard/Download/../Android/data/com.fluxdown.app/files",
        )) {
            assertFalse(path, isSharedStorageAccessPath(path, primary, volumes, appRoots))
        }
        assertTrue(isSharedStorageAccessPath(
            "/sdcard/Android/data/com.fluxdown.app-other/files",
            primary,
            volumes,
            appRoots,
        ))
    }

    private fun assertIOException(operation: () -> Unit) {
        try {
            operation()
            fail("Expected IOException")
        } catch (_: IOException) {
            // The operation must fail rather than treating an invalid path as writable or empty.
        }
    }

    private val primary = File("/storage/emulated/0")
    private val volumes = listOf(primary, File("/storage/ABCD-1234"))
    private val appRoots = listOf(
        File(primary, "Android/data/com.fluxdown.app"),
        File("/storage/ABCD-1234/Android/data/com.fluxdown.app"),
    )
}
