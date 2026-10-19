package com.fluxdown.app.service

import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.model.RuntimeStats
import com.fluxdown.core.store.Connection
import com.fluxdown.core.store.HostState
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalActivityTest {
    private val working = RuntimeStats(activeTasks = 1)

    @Test
    fun staleLocalSessionKeepsServiceAliveWithLastStats() {
        val activity = LocalActivity.from(HostState(connection = Connection.Stale, stats = working))
        assertTrue(activity.busy)
        assertTrue(activity.stale)
    }

    @Test
    fun liveIsNotStale() {
        val activity = LocalActivity.from(HostState(connection = Connection.Live, stats = working))
        assertTrue(activity.busy)
        assertFalse(activity.stale)
    }

    @Test
    fun connectingOrFailedIsIdle() {
        assertFalse(LocalActivity.from(HostState(connection = Connection.Connecting, stats = working)).busy)
        val failed = Connection.Failed(HostException(HostErrorCode.Internal, message = "x"))
        assertFalse(LocalActivity.from(HostState(connection = failed, stats = working)).busy)
    }
}
