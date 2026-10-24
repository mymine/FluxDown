package com.fluxdown.core.store

import com.fluxdown.core.model.HostRef
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteHostCodecTest {
    private val fp = "a".repeat(64)

    @Test
    fun roundTripKeepsLinkFingerprint() {
        val records = listOf(
            RemoteHostRecord(HostRef.Remote("id-1", "NAS", "http://10.0.0.2:17800"), "sealed-1"),
            RemoteHostRecord(HostRef.Remote("id-2", "PC", "http://10.0.0.3:17800", fp), "sealed-2"),
        )
        val encoded = RemoteHostCodec.encode(records)
        assertEquals(records, RemoteHostCodec.decode(encoded))
        // 访问密钥主机不写出该字段（旧版本读到的数据形状不变）。
        assertFalse(encoded.substringBefore("id-2").contains("linkFingerprint"))
        assertTrue(encoded.contains("\"linkFingerprint\":\"$fp\""))
    }

    @Test
    fun legacyEntriesWithoutFingerprintDecodeAsNull() {
        val legacy = """[{"id":"i","name":"Old","endpoint":"http://h:17800","key":"k"}]"""
        val decoded = RemoteHostCodec.decode(legacy)
        assertEquals(listOf(RemoteHostRecord(HostRef.Remote("i", "Old", "http://h:17800", null), "k")), decoded)
        assertNull(decoded.single().ref.linkFingerprint)
    }

    @Test
    fun emptyNameFallsBackToEndpointAndBlankFingerprintIsNull() {
        val raw = """[{"id":"i","endpoint":"http://h:17800","key":"k","linkFingerprint":""}]"""
        val ref = RemoteHostCodec.decode(raw).single().ref
        assertEquals("http://h:17800", ref.displayName)
        assertNull(ref.linkFingerprint)
    }

    @Test
    fun corruptInputIsDroppedNotFatal() {
        assertTrue(RemoteHostCodec.decode(null).isEmpty())
        assertTrue(RemoteHostCodec.decode("").isEmpty())
        assertTrue(RemoteHostCodec.decode("not json").isEmpty())
        assertTrue(RemoteHostCodec.decode("""{"id":"x"}""").isEmpty())
        val mixed = """[{"id":"","endpoint":"e","key":"k"},{"id":"ok","endpoint":"e","key":"k"},{"id":"nokey","endpoint":"e"},5]"""
        assertEquals(listOf("ok"), RemoteHostCodec.decode(mixed).map { it.ref.id })
    }

    @Test
    fun sameFingerprintResolvesToTheExistingHost() {
        val records = listOf(
            RemoteHostRecord(HostRef.Remote("a", "A", "http://a:1"), "k"),
            RemoteHostRecord(HostRef.Remote("b", "B", "http://b:1", fp), "k"),
        )
        assertEquals("b", RemoteHostCodec.idForFingerprint(records, fp))
        assertNull(RemoteHostCodec.idForFingerprint(records, "b".repeat(64)))
        assertNull(RemoteHostCodec.idForFingerprint(emptyList(), fp))
    }
}
