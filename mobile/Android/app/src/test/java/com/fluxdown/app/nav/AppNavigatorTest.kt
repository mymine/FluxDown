package com.fluxdown.app.nav

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AppNavigatorTest {
    @Test
    fun settledSwipeSelectsRoot() {
        val nav = AppNavigator()
        assertTrue(nav.selectTabFromSwipe(AppTab.Devices, nav.revision))
        assertEquals(AppTab.Devices, nav.tab)
        assertTrue(nav.stack.isEmpty())
    }

    @Test
    fun returningToSamePageDoesNotInvalidateNextGesture() {
        val nav = AppNavigator()
        val revision = nav.revision
        assertTrue(nav.selectTabFromSwipe(AppTab.Downloads, revision))
        assertEquals(revision, nav.revision)
        assertTrue(nav.selectTabFromSwipe(AppTab.Rss, revision))
    }

    @Test
    fun dockTapWinsOverOlderSwipe() {
        val nav = AppNavigator()
        val revision = nav.revision
        nav.selectTab(AppTab.Settings)
        assertFalse(nav.selectTabFromSwipe(AppTab.Rss, revision))
        assertEquals(AppTab.Settings, nav.tab)
    }

    @Test
    fun tappingSelectedTabStillCancelsOlderSwipeWithoutPoppingRoute() {
        val nav = AppNavigator()
        val revision = nav.revision
        nav.selectTab(AppTab.Downloads)
        assertFalse(nav.selectTabFromSwipe(AppTab.Rss, revision))
        nav.push(Route.TaskDetail("task"))
        nav.selectTab(AppTab.Downloads)
        assertEquals(Route.TaskDetail("task"), nav.top)
    }

    @Test
    fun fastNavigationAwayAndBackCannotBeUndoneByOldSwipe() {
        val nav = AppNavigator()
        val revision = nav.revision
        nav.selectTab(AppTab.Settings)
        nav.selectTab(AppTab.Downloads)
        assertFalse(nav.selectTabFromSwipe(AppTab.Rss, revision))
        assertEquals(AppTab.Downloads, nav.tab)
    }

    @Test
    fun pushedRouteAndEvenCompletedPushPopInvalidateSwipe() {
        val nav = AppNavigator()
        val revision = nav.revision
        nav.push(Route.TaskDetail("task"))
        assertFalse(nav.selectTabFromSwipe(AppTab.Rss, nav.revision))
        assertTrue(nav.pop())
        assertFalse(nav.selectTabFromSwipe(AppTab.Rss, revision))
        assertTrue(nav.selectTabFromSwipe(AppTab.Rss, nav.revision))
    }

    @Test
    fun sheetSearchAndSelectionBlockPagingAndInvalidateOldCompletions() {
        val blockers: List<Pair<(AppNavigator) -> Unit, (AppNavigator) -> Unit>> = listOf(
            ({ nav: AppNavigator -> nav.openSheet(SheetRoute.ViewOptions) }) to AppNavigator::closeSheet,
            AppNavigator::openSearch to AppNavigator::closeSearch,
            ({ nav: AppNavigator -> nav.enterSelection("task") }) to AppNavigator::exitSelection,
        )
        blockers.forEach { (open, close) ->
            val nav = AppNavigator()
            val revision = nav.revision
            open(nav)
            assertFalse(nav.selectTabFromSwipe(AppTab.Settings, nav.revision))
            close(nav)
            assertFalse(nav.selectTabFromSwipe(AppTab.Settings, revision))
            assertEquals(AppTab.Downloads, nav.tab)
        }
    }

    @Test
    fun detailsStillReplaceAndBackKeepsExistingPriority() {
        val nav = AppNavigator()
        nav.selectTab(AppTab.Settings)
        nav.push(Route.TaskDetail("first"))
        nav.push(Route.TaskDetail("second"))
        assertEquals(listOf(Route.TaskDetail("second")), nav.stack.toList())
        nav.enterSelection("second")
        nav.openSheet(SheetRoute.Activity)
        nav.openSearch()
        assertTrue(nav.back())
        assertFalse(nav.searchOpen)
        assertEquals(SheetRoute.Activity, nav.sheet)
        assertTrue(nav.back())
        assertTrue(nav.selecting)
        assertTrue(nav.back())
        assertEquals(Route.TaskDetail("second"), nav.top)
        assertTrue(nav.back())
        assertEquals(AppTab.Settings, nav.tab)
        assertTrue(nav.back())
        assertEquals(AppTab.Downloads, nav.tab)
        assertFalse(nav.back())
    }
}
