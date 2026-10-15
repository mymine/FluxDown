package com.fluxdown.core.host

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ImportTest {
    private fun summary(
        succeeded: Int = 0,
        failed: Int = 0,
        notSubmitted: Int = 0,
        cancelled: Boolean = false,
        error: HostException? = null,
    ) = ImportSummary(succeeded + failed + notSubmitted, succeeded, failed, notSubmitted, cancelled, error)

    private val rejected = HostException(HostErrorCode.InvalidArgument, reason = "invalidChecksum")

    @Test
    fun fractionIsDoneOverTotalAndEmptyCountsAsComplete() {
        assertEquals(0.25f, ImportProgress(total = 1_000, done = 250, failed = 0).fraction, 0f)
        assertEquals(1f, ImportProgress(total = 0, done = 0, failed = 0).fraction, 0f)
        assertEquals(1f, ImportProgress(total = 10, done = 99, failed = 0).fraction, 0f)
    }

    @Test
    fun allSucceededIsTheCreatedNotice() {
        val notice = summary(succeeded = 300).notice()
        assertSame(ImportNotice.Created, notice)
        assertFalse(notice.isError)
    }

    @Test
    fun cancelledNoticeCarriesAddedOfTotalIncludingNotSubmitted() {
        val notice = summary(succeeded = 256, notSubmitted = 44, cancelled = true).notice()
        assertEquals(ImportNotice.Cancelled(added = 256, total = 300, reason = null), notice)
        assertFalse(notice.isError)
    }

    @Test
    fun cancelAfterFailuresIsAnErrorNoticeWithTheReason() {
        val notice = summary(succeeded = 10, failed = 1, notSubmitted = 5, cancelled = true, error = rejected).notice()
        assertEquals(ImportNotice.Cancelled(added = 10, total = 16, reason = rejected), notice)
        assertTrue(notice.isError)
    }

    @Test
    fun nothingCreatedIsJustTheReason() {
        val notice = summary(failed = 256, notSubmitted = 44, error = rejected).notice()
        assertEquals(ImportNotice.Failed(rejected), notice)
        assertTrue(notice.isError)
    }

    @Test
    fun someFailuresIsPartialWithTotalsAndFirstReason() {
        val notice = summary(succeeded = 299, failed = 1, error = rejected).notice()
        assertEquals(ImportNotice.Partial(added = 299, total = 300, failed = 1, reason = rejected), notice)
        assertTrue(notice.isError)
    }

    @Test
    fun retryExhaustionLeavesRemainingChunksAsNotSubmittedInThePartialTotal() {
        // 第二块之后连接不可达：已添加 256，失败 256，剩余 88 未提交
        val unavailable = HostException(HostErrorCode.Unavailable, retryable = true)
        val notice = summary(succeeded = 256, failed = 256, notSubmitted = 88, error = unavailable).notice()
        assertEquals(ImportNotice.Partial(added = 256, total = 600, failed = 256, reason = unavailable), notice)
    }

    private fun request(url: String, fileName: String = "", checksum: String = "", cookies: String = "") =
        CreateTaskRequest(url = url, fileName = fileName, checksum = checksum, cookies = cookies, saveDir = "/d", startPaused = true)

    @Test
    fun sharedOptionsFormOneBatchKeepingOrderAndPerItemFields() {
        val batches = groupImportBatches(
            listOf(request("https://a/1", fileName = "one.bin"), request("https://a/2", checksum = "sha256=ab"), request("https://a/3")),
        )
        assertEquals(1, batches.size)
        val batch = batches.single()
        assertEquals(listOf("https://a/1", "https://a/2", "https://a/3"), batch.items.map { it.url })
        assertEquals("one.bin", batch.items[0].fileName)
        assertEquals("sha256=ab", batch.items[1].checksum)
        assertEquals("", batch.common.url)
        assertEquals("", batch.common.fileName)
        assertEquals("", batch.common.checksum)
        assertEquals("/d", batch.common.saveDir)
        assertTrue(batch.common.startPaused)
    }

    @Test
    fun differingRequestContextSplitsIntoBatchesInFirstSeenOrder() {
        val batches = groupImportBatches(
            listOf(
                request("https://a/1"),
                request("https://b/1", cookies = "k=v"),
                request("https://a/2"),
                request("https://b/2", cookies = "k=v"),
            ),
        )
        assertEquals(2, batches.size)
        assertEquals(listOf("https://a/1", "https://a/2"), batches[0].items.map { it.url })
        assertEquals("", batches[0].common.cookies)
        assertEquals(listOf("https://b/1", "https://b/2"), batches[1].items.map { it.url })
        assertEquals("k=v", batches[1].common.cookies)
    }

    @Test
    fun emptyRequestsGiveNoBatches() {
        assertTrue(groupImportBatches(emptyList()).isEmpty())
    }
}
