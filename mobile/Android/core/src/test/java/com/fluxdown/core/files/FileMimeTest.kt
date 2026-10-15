package com.fluxdown.core.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FileMimeTest {
    private val none: (String) -> String? = { null }

    @Test
    fun caseInsensitive() {
        assertEquals("video/x-matroska", FileMime.resolve("Movie.MKV", none))
    }

    @Test
    fun noExtensionIsUnknown() {
        assertEquals("*/*", FileMime.resolve("README", none))
        assertEquals("*/*", FileMime.resolve("trailing.", none))
        assertEquals("*/*", FileMime.resolve(".hidden", none))
    }

    @Test
    fun overrideBeatsSystem() {
        assertEquals("video/mp2t", FileMime.resolve("a.ts", { "text/x-typescript" }))
    }

    @Test
    fun systemFallback() {
        assertEquals("text/csv", FileMime.resolve("a.csv", { if (it == "csv") "text/csv" else null }))
    }

    @Test
    fun unknownIsWildcard() {
        assertEquals("*/*", FileMime.resolve("a.zzzz", none))
        assertEquals("*/*", FileMime.resolve("a.zzzz", { "" }))
    }

    @Test
    fun lastSegmentWins() {
        assertEquals("application/gzip", FileMime.resolve("a.tar.gz", none))
        assertNull(FileMime.extensionOf("dir.d/file"))
    }
}
