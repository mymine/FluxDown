package com.fluxdown.fluxui.chrome

import kotlin.math.roundToInt
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DockGeometryTest {
    private fun assertCoverage(geometry: DockGeometry, width: Int) {
        for (rtl in listOf(false, true)) {
            var edge = 0
            val order = if (rtl) geometry.widths.indices.reversed() else geometry.widths.indices
            for (i in order) {
                assertEquals("tab $i, rtl=$rtl", edge, geometry.left(i, rtl))
                assertTrue(geometry.widths[i] >= 0)
                edge += geometry.widths[i]
            }
            assertEquals(width, edge)
            assertTrue(geometry.indicatorLeft(rtl) >= 0)
            assertTrue(geometry.indicatorWidth >= 0)
            assertTrue(geometry.indicatorLeft(rtl) + geometry.indicatorWidth <= width)
        }
    }

    @Test
    fun tabsShareWidthEquallyAndIndicatorSitsOnSelectedTab() {
        for (count in listOf(3, 4)) {
            for (width in listOf(0, 24, 131, 191, 192, 193, 239, 301, 900)) {
                for (selected in 0 until count) {
                    val geometry = DockGeometry(count)
                    geometry.update(width, FloatArray(count) { if (it == selected) 1f else 0f }, selected)
                    assertCoverage(geometry, width)
                    assertTrue(geometry.widths.max() - geometry.widths.min() <= 1)
                    for (rtl in listOf(false, true)) {
                        assertEquals(geometry.left(selected, rtl), geometry.indicatorLeft(rtl))
                    }
                    assertEquals(geometry.widths[selected], geometry.indicatorWidth)
                }
            }
        }
    }

    @Test
    fun indicatorInterpolatesTabEdgesDuringSelectionSpring() {
        val geometry = DockGeometry(4)
        geometry.update(300, floatArrayOf(0.5f, 0f, 0f, 0.5f), 3)
        assertArrayEquals(intArrayOf(75, 75, 75, 75), geometry.widths)
        assertArrayEquals(intArrayOf(0, 75, 150, 225), geometry.starts)
        assertEquals(113, geometry.indicatorStart)
        assertEquals(75, geometry.indicatorWidth)
        assertEquals(112, geometry.indicatorLeft(true))
        assertCoverage(geometry, 300)
    }

    @Test
    fun selectionAnimationCoversAllPixelsAtFractionalDensities() {
        for (density in listOf(0.75f, 1f, 1.1f, 1.25f, 1.5f, 2.625f, 3.5f)) {
            val pad = (4 * density).roundToInt()
            val height = (64 * density).roundToInt()
            for (count in listOf(3, 4)) {
                for (availableDp in listOf(180, 320, 393, 768)) {
                    val available = (availableDp * density).roundToInt()
                    val reserved = (76 * density).roundToInt()
                    val width = dockWidth(available, reserved, height) - pad * 2
                    for (from in 0 until count) for (selected in 0 until count) {
                        for (progress in listOf(0f, 0.13f, 0.5f, 0.87f, 1f)) {
                            val weights = FloatArray(count)
                            weights[from] += 1f - progress
                            weights[selected] += progress
                            val geometry = DockGeometry(count)
                            geometry.update(width, weights, selected)
                            assertCoverage(geometry, width)
                        }
                    }
                }
            }
        }
    }

    @Test
    fun largeFontsDropAllLabelsTogether() {
        assertTrue(dockLabelsFit(56, 24, 3, intArrayOf(17, 17, 29, 17)))
        assertFalse(dockLabelsFit(56, 24, 3, intArrayOf(17, 17, 30, 17)))
    }

    @Test
    fun interruptedSpringsAndNarrowContainersStayBounded() {
        assertEquals(317, dockWidth(393, 76, 64))
        assertEquals(64, dockWidth(100, 76, 64))
        assertEquals(40, dockWidth(40, 76, 64))
        val geometry = DockGeometry(4)
        for (weights in listOf(floatArrayOf(1.1f, -0.1f, 0f, 0.2f), FloatArray(4))) {
            geometry.update(301, weights, 3)
            assertCoverage(geometry, 301)
        }
        assertEquals(geometry.starts[3], geometry.indicatorStart)
        assertEquals(geometry.widths[3], geometry.indicatorWidth)
    }
}
