package com.fluxdown.core.protocol

import com.fluxdown.core.model.CloudDevice
import com.fluxdown.core.model.LinkDevice
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** 与 iOS `RemoteTasksProtocolTests` 的同一组下发规则用例。 */
class DevicesTest {
    @Test
    fun pathStyleInferenceAndAbsolutePaths() {
        assertEquals(PathStyle.Windows, PathStyle.effective(null, "Windows"))
        assertEquals(PathStyle.Posix, PathStyle.effective("posix", "windows"))
        assertEquals(PathStyle.Posix, PathStyle.effective("weird", "android"))
        assertNull(PathStyle.effective(null, "web"))
        assertTrue(PathStyle.Windows.isAbsolute("C:\\dl"))
        assertTrue(PathStyle.Windows.isAbsolute("d:/dl"))
        assertTrue(PathStyle.Windows.isAbsolute("\\\\server\\share"))
        assertFalse(PathStyle.Windows.isAbsolute("/home"))
        assertFalse(PathStyle.Windows.isAbsolute("C:"))
        assertTrue(PathStyle.Posix.isAbsolute(" /mnt/dl "))
        assertFalse(PathStyle.Posix.isAbsolute("dl"))
        assertFalse(PathStyle.Unknown("q").isAbsolute("/x"))
    }

    @Test
    fun remoteSaveDirFollowsTargetPathStyle() {
        assertEquals(DeviceRules.SaveDirCheck.UseDefault, DeviceRules.checkSaveDir("  ", PathStyle.Posix))
        assertEquals(DeviceRules.SaveDirCheck.Explicit("/mnt/dl"), DeviceRules.checkSaveDir(" /mnt/dl ", PathStyle.Posix))
        assertEquals(DeviceRules.SaveDirCheck.Invalid, DeviceRules.checkSaveDir("C:\\dl", PathStyle.Posix))
        assertEquals(DeviceRules.SaveDirCheck.Invalid, DeviceRules.checkSaveDir("/mnt", PathStyle.Windows))
        assertEquals(DeviceRules.SaveDirCheck.Explicit("D:\\x"), DeviceRules.checkSaveDir("D:\\x", PathStyle.Windows))
        // 风格未知：无法判断，非空即放行。
        assertEquals(DeviceRules.SaveDirCheck.Explicit("whatever"), DeviceRules.checkSaveDir("whatever", null))
        assertEquals(DeviceRules.SaveDirCheck.Explicit("whatever"), DeviceRules.checkSaveDir("whatever", PathStyle.Unknown("?")))
        assertEquals("D:\\Downloads", PathStyle.example(PathStyle.Windows))
        assertEquals("/home/user/Downloads", PathStyle.example(null))
    }

    @Test
    fun dispatchTargetsSkipCurrentDedupeAndDisambiguate() {
        val cloud = listOf(
            CloudDevice("me-1", "Me", "ios", isOnline = true, isCurrent = true, appVersion = null),
            CloudDevice("pc-aa11", " Office ", "windows", isOnline = false, isCurrent = false, appVersion = null, defaultSaveDir = "  "),
            CloudDevice("pc-aa11", "dup", null, isOnline = true, isCurrent = false, appVersion = null),
            CloudDevice("", "no id", null, isOnline = true, isCurrent = false, appVersion = null),
            CloudDevice("nas-bb22", "office", "linux", isOnline = true, isCurrent = false, appVersion = null, defaultSaveDir = " /srv/dl ", pathStyle = "windows"),
        )
        val link = listOf(
            LinkDevice("fp:cc-33", "", null, online = true, defaultSaveDir = "/x", pathStyle = "posix"),
            LinkDevice("", "ghost", null, online = true),
        )
        val targets = DeviceRules.dispatchTargets(cloud, link, cloudPresenceKnown = true, localReady = true)
        assertEquals(listOf("cloud:pc-aa11", "cloud:nas-bb22", "link:fp:cc-33"), targets.map { it.id })
        // 同名（忽略大小写与首尾空白）追加短码；空名用短码。
        assertEquals(listOf("Office · aa11", "office · bb22", "cc33"), targets.map { it.name })
        assertEquals(listOf(false, true, true), targets.map { it.online })
        // 空白目录读作无；自报风格优先于平台推断。
        assertNull(targets[0].defaultSaveDir)
        assertEquals(PathStyle.Windows, targets[0].pathStyle)
        assertEquals("/srv/dl", targets[1].defaultSaveDir)
        assertEquals(PathStyle.Windows, targets[1].pathStyle)
        assertTrue(targets[0].isCloud && targets[0].isOffline)
        assertTrue(!targets[2].isCloud && !targets[2].isOffline && targets[2].isLinkOnly)
    }

    @Test
    fun dispatchTargetPresenceUnknownUntilTrusted() {
        val cloud = listOf(CloudDevice("pc", "PC", null, isOnline = false, isCurrent = false, appVersion = null))
        val link = listOf(LinkDevice("fp", "Mac", "macos", online = false))
        // 云端 presence 不可信：云设备在线未知（不当作离线），独立直连设备仍按本地探测。
        val cloudUnknown = DeviceRules.dispatchTargets(cloud, link, cloudPresenceKnown = false, localReady = true)
        assertEquals(listOf(null, false), cloudUnknown.map { it.online })
        assertFalse(cloudUnknown[0].isOffline)
        // 本地服务未就绪：两类都未知。
        val notReady = DeviceRules.dispatchTargets(cloud, link, cloudPresenceKnown = true, localReady = false)
        assertEquals(listOf(null, null), notReady.map { it.online })
    }

    @Test
    fun cloudDevicesSortByPresenceOnlyWhenKnown() {
        fun d(name: String, online: Boolean = false, current: Boolean = false) =
            CloudDevice(name, name, null, isOnline = online, isCurrent = current, appVersion = null)
        val devices = listOf(d("zeta", online = true), d("Alpha"), d("beta", online = true), d("me", current = true), d("Gamma"))
        assertEquals(listOf("me", "beta", "zeta", "Alpha", "Gamma"), DeviceRules.sorted(devices, presenceKnown = true).map { it.name })
        // presence 未知：不按在线分组，纯名称序（未知不当离线沉底）。
        assertEquals(listOf("me", "Alpha", "beta", "Gamma", "zeta"), DeviceRules.sorted(devices, presenceKnown = false).map { it.name })
    }

    @Test
    fun sameMachineMergesIntoOneCloudEntry() {
        val cloud = listOf(
            CloudDevice("pc", "Office", "windows", isOnline = false, isCurrent = false, appVersion = null),
            CloudDevice("nas", "NAS", "linux", isOnline = true, isCurrent = false, appVersion = null, defaultSaveDir = "/srv"),
        )
        val link = listOf(
            LinkDevice("fp-pc", "Office-LAN", "windows", online = true, defaultSaveDir = "D:\\dl", pathStyle = "windows", cloudDeviceId = "pc"),
            // 云名册里没有该 id：独立直连条目。
            LinkDevice("fp-x", "Phone", "android", online = true, cloudDeviceId = "gone"),
            // 同一云条目已被挂接：后到的记录独立成条。
            LinkDevice("fp-pc2", "Office 2", "windows", online = false, cloudDeviceId = "pc"),
            // 对方已解除：不出现在任何下发目标里，也不挂接。
            LinkDevice("fp-rev", "Revoked", "macos", online = true, cloudDeviceId = "nas", revoked = true),
        )
        val targets = DeviceRules.dispatchTargets(cloud, link, cloudPresenceKnown = true, localReady = true)
        assertEquals(listOf("cloud:pc", "cloud:nas", "link:fp-x", "link:fp-pc2"), targets.map { it.id })
        // 账户页「同一账号」标签只给真正挂接的记录（后到的同 id、cloudDeviceId 不在名册、revoked 都不算）。
        assertEquals(setOf("fp-pc"), DeviceRules.attachedLinkFingerprints(cloud, link))
        val pc = targets[0]
        // 显示名取云名；任一在线即在线；目录云缺失时取直连的；风格云推断优先。
        assertEquals("Office", pc.name)
        assertEquals(true, pc.online)
        assertEquals("D:\\dl", pc.defaultSaveDir)
        assertEquals(PathStyle.Windows, pc.pathStyle)
        assertEquals("fp-pc", pc.linkFingerprint)
        assertEquals("fp-pc", pc.linkRouteFingerprint)
        assertTrue(pc.routesViaLink && !pc.isLinkOnly)
        // 未挂接的云条目走云；独立直连条目走直连。
        assertNull(targets[1].linkFingerprint)
        assertFalse(targets[1].routesViaLink)
        assertTrue(targets[2].routesViaLink && targets[2].isLinkOnly)
        assertEquals("fp-x", targets[2].linkRouteFingerprint)
    }

    @Test
    fun mergedOnlineAndRouting() {
        val cloud = listOf(CloudDevice("pc", "PC", null, isOnline = true, isCurrent = false, appVersion = null))
        fun merged(linkOnline: Boolean, cloudKnown: Boolean, ready: Boolean = true) = DeviceRules.dispatchTargets(
            cloud,
            listOf(LinkDevice("fp", "PC", null, online = linkOnline, cloudDeviceId = "pc")),
            cloudPresenceKnown = cloudKnown,
            localReady = ready,
        ).single()
        // 直连离线、云在线：在线，但走云。
        merged(linkOnline = false, cloudKnown = true).let {
            assertEquals(true, it.online)
            assertNull(it.linkRouteFingerprint)
        }
        // 云 presence 未知 + 直连在线：在线，走直连。
        merged(linkOnline = true, cloudKnown = false).let {
            assertEquals(true, it.online)
            assertEquals("fp", it.linkRouteFingerprint)
        }
        // 云未知 + 直连离线：未知（不当作离线），走云。
        merged(linkOnline = false, cloudKnown = false).let {
            assertNull(it.online)
            assertFalse(it.isOffline)
            assertNull(it.linkRouteFingerprint)
        }
        // 本地服务未就绪：云与直连都未知，不走直连。
        merged(linkOnline = true, cloudKnown = true, ready = false).let {
            assertNull(it.online)
            assertNull(it.linkOnline)
            assertNull(it.linkRouteFingerprint)
        }
        // 双方都已知离线：离线。
        val offline = DeviceRules.dispatchTargets(
            listOf(CloudDevice("pc", "PC", null, isOnline = false, isCurrent = false, appVersion = null)),
            listOf(LinkDevice("fp", "PC", null, online = false, cloudDeviceId = "pc")),
            cloudPresenceKnown = true,
            localReady = true,
        ).single()
        assertEquals(false, offline.online)
        assertTrue(offline.isOffline)
    }

    @Test
    fun disambiguationRunsOverMergedList() {
        val cloud = listOf(CloudDevice("pc-aa11", "Mac", null, isOnline = true, isCurrent = false, appVersion = null))
        val link = listOf(
            // 已合并：不再与云条目同名消歧。
            LinkDevice("fp-merged-1111", "Mac", null, online = true, cloudDeviceId = "pc-aa11"),
            // 独立条目与云条目同名：整表消歧，短码取云 id / 指纹。
            LinkDevice("fp-other-bb22", " mac ", null, online = true),
        )
        val targets = DeviceRules.dispatchTargets(cloud, link, cloudPresenceKnown = true, localReady = true)
        assertEquals(listOf("Mac · aa11", "mac · bb22"), targets.map { it.name })
        val single = DeviceRules.dispatchTargets(cloud, link.take(1), cloudPresenceKnown = true, localReady = true)
        assertEquals(listOf("Mac"), single.map { it.name })
    }

    @Test
    fun resolveTargetFollowsAttachedLink() {
        val cloud = listOf(CloudDevice("pc", "PC", null, isOnline = true, isCurrent = false, appVersion = null))
        val link = listOf(
            LinkDevice("fp", "PC", null, online = true, cloudDeviceId = "pc"),
            LinkDevice("solo", "Solo", null, online = true),
        )
        val targets = DeviceRules.dispatchTargets(cloud, link, cloudPresenceKnown = true, localReady = true)
        assertEquals("cloud:pc", DeviceRules.resolveTarget("cloud:pc", targets)?.id)
        // 持久化的 link:<fp> 已挂接到云条目 → 解析为该云条目。
        assertEquals("cloud:pc", DeviceRules.resolveTarget("link:fp", targets)?.id)
        assertEquals("link:solo", DeviceRules.resolveTarget("link:solo", targets)?.id)
        assertNull(DeviceRules.resolveTarget("link:revoked", targets))
        assertNull(DeviceRules.resolveTarget("cloud:gone", targets))
    }

    @Test
    fun dispatchParamsOmitEmptyOptionals() {
        assertEquals("""{"toDevice":"d","url":"http://x/a"}""", RemoteDispatchParams("d", "http://x/a").toJson().toJson())
        assertEquals(
            """{"toDevice":"d","url":"u","fileName":"a.bin","saveDir":"/dl"}""",
            RemoteDispatchParams("d", "u", "a.bin", "/dl").toJson().toJson(),
        )
        assertEquals("""{"fingerprint":"fp","url":"u"}""", LinkDispatchParams("fp", "u").toJson().toJson())
    }
}
