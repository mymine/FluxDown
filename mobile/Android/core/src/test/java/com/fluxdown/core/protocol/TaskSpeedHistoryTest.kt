package com.fluxdown.core.protocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TaskSpeedHistoryTest {
    private fun parse(json: String) = TaskSpeedHistory.fromJson(Json.parse(json))

    @Test fun decodesWireAndDerivesAverage() {
        val h = parse(
            """{"taskId":"t","bucketSecs":2,"samples":[0,100,300],"activeSecs":6,"transferredBytes":1200,"peakBps":400}""",
        )!!
        assertEquals(listOf(0L, 100L, 300L), h.samples)
        assertEquals(2, h.bucketSecs)
        assertEquals(200L, h.averageBps)
        assertTrue(h.hasChart)
    }

    @Test fun noActiveTimeMeansZeroAverageAndEmptyChart() {
        val h = parse("""{"taskId":"t","bucketSecs":1,"samples":[0,0],"activeSecs":0,"transferredBytes":5,"peakBps":0}""")!!
        assertEquals(0L, h.averageBps)
        assertFalse(h.hasChart)
        assertFalse(parse("""{"taskId":"t","samples":[5],"peakBps":5}""")!!.hasChart)
    }

    @Test fun nullResultIsNoHistory() {
        assertNull(parse("null"))
    }

    private fun history(bucket: Int, samples: List<Long>, active: Long) =
        TaskSpeedHistory("t", bucket, samples, active, 0, samples.maxOrNull() ?: 0)

    @Test fun elapsedFollowsBucketsAndClampsToActiveTime() {
        // 3 点 × 2 s，活跃 5 s：末点是部分桶 → 5 s
        assertEquals(listOf(2L, 4L, 5L), history(2, listOf(1, 2, 3), 5).elapsedSecs().toList())
    }

    @Test fun elapsedIsStrictlyIncreasingWhenActiveTimeIsShort() {
        // 活跃 1 s 但有 4 点 → 顺延为 1,2,3,4，不重复不倒退
        assertEquals(listOf(1L, 2L, 3L, 4L), history(1, listOf(1, 2, 3, 4), 1).elapsedSecs().toList())
        assertEquals(listOf(1L, 2L), history(5, listOf(1, 2), 0).elapsedSecs().toList())
    }

    @Test fun formatsElapsedAsMinutesOrHours() {
        assertEquals("0:00", TaskSpeedHistory.formatElapsed(0))
        assertEquals("0:59", TaskSpeedHistory.formatElapsed(59))
        assertEquals("1:05", TaskSpeedHistory.formatElapsed(65))
        assertEquals("59:59", TaskSpeedHistory.formatElapsed(3599))
        assertEquals("1:00:00", TaskSpeedHistory.formatElapsed(3600))
        assertEquals("2:03:04", TaskSpeedHistory.formatElapsed(7384))
        assertEquals("0:00", TaskSpeedHistory.formatElapsed(-3))
    }

    @Test fun axisHasThreeAscendingTicksEndingAtChartMax() {
        // 小样本：量程取 1 MiB 下限 × 1.18
        assertEquals(1_237_319L, history(1, listOf(10, 20), 2).chartMaxBps)
        val big = history(1, listOf(0, 10L shl 20), 2)
        val ticks = big.axisTicksBps
        assertEquals(3, ticks.size)
        assertEquals(big.chartMaxBps, ticks[2])
        assertTrue(ticks[0] < ticks[1] && ticks[1] < ticks[2])
        assertEquals(big.chartMaxBps / 3, ticks[0])
    }
}
