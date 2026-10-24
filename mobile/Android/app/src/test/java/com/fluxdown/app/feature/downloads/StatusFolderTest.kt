package com.fluxdown.app.feature.downloads

import com.fluxdown.app.ui.TaskVisualState
import com.fluxdown.app.ui.visualState
import com.fluxdown.core.model.SeedingStatus
import com.fluxdown.core.model.Task
import com.fluxdown.core.model.TaskStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StatusFolderTest {
    private fun task(status: TaskStatus, seeding: SeedingStatus = SeedingStatus.None, missing: Boolean = false) = Task(
        taskId = "t", url = "magnet:?xt=1", originUrl = "", fileName = "a", saveDir = "/d",
        status = status, downloadedBytes = 1, totalBytes = 1, fileMissing = missing, seedingStatus = seeding,
    )

    @Test
    fun seedingFolderFollowsCompletedInOrder() {
        val order = StatusFolder.entries
        assertEquals(order.indexOf(StatusFolder.Completed) + 1, order.indexOf(StatusFolder.Seeding))
    }

    @Test
    fun seedingFolderMembership() {
        assertTrue(StatusFolder.Seeding.accepts(task(TaskStatus.Completed, SeedingStatus.Seeding)))
        assertTrue(StatusFolder.Seeding.accepts(task(TaskStatus.Completed, SeedingStatus.QueuedForSlot)))
        assertFalse(StatusFolder.Seeding.accepts(task(TaskStatus.Completed, SeedingStatus.UserStopped)))
        assertFalse(StatusFolder.Seeding.accepts(task(TaskStatus.Completed, SeedingStatus.None)))
        assertFalse(StatusFolder.Seeding.accepts(task(TaskStatus.Downloading, SeedingStatus.Seeding)))
        // 做种任务同时仍属于「已完成」。
        assertTrue(StatusFolder.Completed.accepts(task(TaskStatus.Completed, SeedingStatus.Seeding)))
        assertTrue(StatusFolder.All.accepts(task(TaskStatus.Completed, SeedingStatus.Seeding)))
    }

    @Test
    fun queuedSeedingKeepsSeedingVisual() {
        assertEquals(TaskVisualState.Seeding, task(TaskStatus.Completed, SeedingStatus.Seeding).visualState(false))
        assertEquals(TaskVisualState.Seeding, task(TaskStatus.Completed, SeedingStatus.QueuedForSlot).visualState(false))
        assertEquals(TaskVisualState.Completed, task(TaskStatus.Completed, SeedingStatus.UserStopped).visualState(false))
    }
}
