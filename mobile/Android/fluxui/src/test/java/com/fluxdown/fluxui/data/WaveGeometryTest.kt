package com.fluxdown.fluxui.data

import org.junit.Assert.assertEquals
import org.junit.Test

class WaveGeometryTest {
    @Test fun pointsAreEvenlySpacedAcrossTheChart() {
        assertEquals(0f, waveXAt(0, 100f, 5), 0f)
        assertEquals(25f, waveXAt(1, 100f, 5), 0f)
        assertEquals(100f, waveXAt(4, 100f, 5), 0f)
        assertEquals(0f, waveXAt(0, 100f, 1), 0f)
    }

    @Test fun touchSnapsToTheNearestPoint() {
        assertEquals(0, waveIndexAt(0f, 100f, 5))
        assertEquals(1, waveIndexAt(30f, 100f, 5))
        assertEquals(2, waveIndexAt(40f, 100f, 5))
        assertEquals(4, waveIndexAt(100f, 100f, 5))
    }

    @Test fun touchOutsideTheChartIsClampedAndEmptyHasNoIndex() {
        assertEquals(0, waveIndexAt(-20f, 100f, 5))
        assertEquals(4, waveIndexAt(400f, 100f, 5))
        assertEquals(0, waveIndexAt(50f, 100f, 1))
        assertEquals(-1, waveIndexAt(50f, 100f, 0))
    }
}
