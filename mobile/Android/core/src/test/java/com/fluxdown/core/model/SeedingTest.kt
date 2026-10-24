package com.fluxdown.core.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SeedingTest {
    private fun task(
        status: TaskStatus = TaskStatus.Completed,
        seeding: SeedingStatus = SeedingStatus.None,
        missing: Boolean = false,
        id: String = "t",
    ) = Task(
        taskId = id, url = "magnet:?xt=1", originUrl = "", fileName = "a", saveDir = "/d",
        status = status, downloadedBytes = 1, totalBytes = 1, fileMissing = missing, seedingStatus = seeding,
    )

    @Test
    fun isSeedingCoversSeedingAndQueuedOnlyWhenCompleted() {
        assertTrue(task(seeding = SeedingStatus.Seeding).isSeeding)
        assertTrue(task(seeding = SeedingStatus.QueuedForSlot).isSeeding)
        assertFalse(task(seeding = SeedingStatus.UserStopped).isSeeding)
        assertFalse(task(seeding = SeedingStatus.None).isSeeding)
        assertFalse(task(status = TaskStatus.Downloading, seeding = SeedingStatus.Seeding).isSeeding)
        assertFalse(task(status = TaskStatus.Paused, seeding = SeedingStatus.QueuedForSlot).isSeeding)
    }

    @Test
    fun canResumeSeedingOnlyForStoppedStatesWithFile() {
        for (s in listOf(
            SeedingStatus.RatioReached, SeedingStatus.TimeReached, SeedingStatus.UserStopped,
            SeedingStatus.TaskDeleted, SeedingStatus.SessionReleased, SeedingStatus.InactiveStop,
        )) {
            assertTrue("$s", task(seeding = s).canResumeSeeding)
            assertTrue("$s", task(seeding = s).canResume)
            assertFalse("$s missing", task(seeding = s, missing = true).canResumeSeeding)
        }
        for (s in listOf(SeedingStatus.None, SeedingStatus.Seeding, SeedingStatus.QueuedForSlot, SeedingStatus.Unknown)) {
            assertFalse("$s", task(seeding = s).canResumeSeeding)
        }
        // #935：普通已完成任务保持不可恢复。
        assertFalse(task(seeding = SeedingStatus.None).canResume)
        assertFalse(task(status = TaskStatus.Paused, seeding = SeedingStatus.UserStopped).canResumeSeeding)
    }

    @Test
    fun pauseAndResumeEligibility() {
        assertTrue(task(seeding = SeedingStatus.Seeding).canPause)
        assertTrue(task(seeding = SeedingStatus.QueuedForSlot).canPause)
        assertFalse(task(seeding = SeedingStatus.None).canPause)
        assertFalse(task(seeding = SeedingStatus.UserStopped).canPause)
        assertTrue(task(status = TaskStatus.Downloading).canPause)
        assertTrue(task(status = TaskStatus.Pending).canPause)
        assertTrue(task(status = TaskStatus.Preparing).canPause)
        assertTrue(task(status = TaskStatus.Paused).canResume)
        assertTrue(task(status = TaskStatus.Failed).canResume)
        assertFalse(task(status = TaskStatus.Downloading).canResume)
    }

    @Test
    fun allPausedCountsSeedingAsPausable() {
        val paused = task(status = TaskStatus.Paused, id = "p")
        val seeding = task(seeding = SeedingStatus.Seeding, id = "s")
        val stopped = task(seeding = SeedingStatus.UserStopped, id = "u")
        val stoppedMissing = task(seeding = SeedingStatus.UserStopped, missing = true, id = "m")
        assertTrue(allTasksPaused(listOf(paused), 0, 0))
        assertTrue(allTasksPaused(listOf(stopped), 0, 0))
        assertFalse(allTasksPaused(listOf(stoppedMissing), 0, 0))
        assertFalse(allTasksPaused(listOf(seeding), 0, 0))
        assertFalse(allTasksPaused(listOf(paused, seeding), 0, 0))
        assertFalse(allTasksPaused(listOf(paused), 1, 0))
        assertFalse(allTasksPaused(listOf(paused), 0, 1))
        assertEquals(false, allTasksPaused(emptyList(), 0, 0))
    }
}
