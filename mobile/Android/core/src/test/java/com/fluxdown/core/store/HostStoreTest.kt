package com.fluxdown.core.store

import com.fluxdown.core.host.HostEvent
import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import com.fluxdown.core.host.HostSession
import com.fluxdown.core.host.HostSignal
import com.fluxdown.core.host.HostSnapshot
import com.fluxdown.core.model.HostInfo
import com.fluxdown.core.model.RuntimeStats
import com.fluxdown.core.model.Segment
import com.fluxdown.core.model.QueueOrderEntry
import com.fluxdown.core.model.Task
import com.fluxdown.core.model.TaskRuntime
import com.fluxdown.core.model.TaskStatus
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.lang.reflect.Proxy

@OptIn(ExperimentalCoroutinesApi::class)
class HostStoreTest {
    private fun task(id: String, status: TaskStatus, done: Long = 50, total: Long = 100) =
        Task(taskId = id, url = "https://x/$id", originUrl = "", fileName = "$id.bin", saveDir = "/d", status = status, downloadedBytes = done, totalBytes = total)

    private fun seg(i: Int, active: Boolean?) = Segment(i, i * 10L, i * 10L + 9, 5, active)

    private fun runtime(id: String, seq: Long, segs: List<Segment>) = TaskRuntime(id, seq, 2, null, 100, segs)

    private fun snapshot(vararg tasks: Task, runtime: Map<String, TaskRuntime> = emptyMap()) = HostSignal.Snapshot(
        HostSnapshot(
            info = HostInfo("t", "1", 7, emptySet()), daemonConnected = true, tasks = tasks.toList(), runtime = runtime,
            queues = emptyList(), queueOrder = emptyList(), groups = emptyList(), stats = RuntimeStats(), priorityTaskId = null,
            pendingSelections = emptyList(), config = emptyMap(), configRevision = 1, rssSources = emptyList(),
            cloudDevices = emptyList(), linkDevices = emptyList(),
        ),
    )

    private fun progress(id: String, status: TaskStatus, speed: Long, error: String = "") = HostSignal.Event(
        HostEvent.TaskProgress(id, status.wire, 60, 100, speed, 0, "", error, 0, 0),
    )

    private fun TestScope.store(): HostStore = HostStore(this, clock = { 1_000_000 })

    @Test
    fun progressWithDeletedSentinelRemovesTaskAndRuntime() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading), runtime = mapOf("a" to runtime("a", 1, listOf(seg(0, true))))))
        s.apply(progress("a", TaskStatus.Failed, 0, error = "deleted"))
        assertNull(s.state.value.task("a"))
        assertFalse("a" in s.state.value.runtime)
    }

    @Test
    fun staleRuntimeSampleIsDroppedAndEmptySegmentsKeepPrevious() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading), runtime = mapOf("a" to runtime("a", 5, listOf(seg(0, true), seg(1, true))))))
        s.apply(HostSignal.Event(HostEvent.TaskRuntimeChanged(runtime("a", 4, listOf(seg(0, false))))))
        advanceUntilIdle()
        assertEquals(2, s.state.value.runtime.getValue("a").segments.size)

        s.apply(HostSignal.Event(HostEvent.TaskRuntimeChanged(runtime("a", 6, emptyList()).copy(activeTransfers = 1))))
        advanceUntilIdle()
        val rt = s.state.value.runtime.getValue("a")
        assertEquals(6, rt.sampleSequence)
        assertEquals(2, rt.segments.size)
        assertEquals(1, rt.activeTransfers)
    }

    @Test
    fun pausingClearsActiveSegmentsAndLiveSpeed() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading), runtime = mapOf("a" to runtime("a", 1, listOf(seg(0, true), seg(1, null))))))
        s.apply(progress("a", TaskStatus.Downloading, 4096))
        advanceUntilIdle()
        assertEquals(4096, s.state.value.speeds.getValue("a").down)

        s.apply(progress("a", TaskStatus.Paused, 4096))
        val st = s.state.value
        assertEquals(TaskStatus.Paused, st.task("a")?.status)
        assertEquals(0L, st.speeds.getValue("a").down)
        assertTrue(st.runtime.getValue("a").segments.all { it.active == false })
        assertEquals(0, st.runtime.getValue("a").activeTransfers)
    }

    @Test
    fun completedTasksLeaveNoRuntimeAndLateSamplesCannotReaddIt() = runTest {
        val s = store()
        s.apply(snapshot())
        for (index in 0 until 600) {
            val id = "t$index"
            s.apply(HostSignal.Event(HostEvent.TaskChanged(task(id, TaskStatus.Downloading))))
            s.apply(HostSignal.Event(HostEvent.TaskRuntimeChanged(runtime(id, index + 1L, listOf(seg(0, true))))))
            advanceUntilIdle()
            assertEquals(setOf(id), s.state.value.runtime.keys)
            // 两条完成路径：TaskChanged / TaskProgress。
            if (index % 2 == 0) {
                s.apply(HostSignal.Event(HostEvent.TaskChanged(task(id, TaskStatus.Completed))))
            } else {
                s.apply(progress(id, TaskStatus.Completed, 0))
            }
            advanceUntilIdle()
            assertTrue(id, s.state.value.runtime.isEmpty())
            // 迟到的采样（序号更高也一样）不得把条目加回来。
            s.apply(HostSignal.Event(HostEvent.TaskRuntimeChanged(runtime(id, index + 10_000L, listOf(seg(0, true))))))
            advanceUntilIdle()
            assertTrue(id, s.state.value.runtime.isEmpty())
        }
        assertEquals(600, s.state.value.tasks.size)
    }

    @Test
    fun snapshotRuntimeOfCompletedTasksIsDroppedButPausedAndFailedKeepSegments() = runTest {
        val s = store()
        s.apply(
            snapshot(
                task("run", TaskStatus.Downloading), task("paused", TaskStatus.Paused),
                task("failed", TaskStatus.Failed), task("done", TaskStatus.Completed),
                runtime = listOf("run", "paused", "failed", "done", "gone").associateWith { runtime(it, 1, listOf(seg(0, true))) },
            ),
        )
        assertEquals(setOf("run", "paused", "failed"), s.state.value.runtime.keys)
        s.apply(progress("paused", TaskStatus.Completed, 0))
        s.apply(progress("failed", TaskStatus.Paused, 0))
        advanceUntilIdle()
        assertEquals(setOf("run", "failed"), s.state.value.runtime.keys)
        assertTrue(s.state.value.runtime.getValue("failed").segments.all { it.active == false })
    }

    @Test
    fun staleSignalMakesStateReadOnlyAndDropsRuntimeAndSpeeds() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading), runtime = mapOf("a" to runtime("a", 1, listOf(seg(0, true))))))
        s.apply(progress("a", TaskStatus.Downloading, 100))
        s.apply(HostSignal.Stale)
        val st = s.state.value
        assertTrue(st.isReadOnly)
        assertTrue(st.runtime.isEmpty())
        assertTrue(st.speeds.isEmpty())
        assertEquals(1, st.tasks.size)
    }

    @Test
    fun highFrequencyProgressIsCoalescedIntoOnePublish() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading)))
        val before = s.state.value
        repeat(20) { s.apply(progress("a", TaskStatus.Downloading, it.toLong() + 1)) }
        assertTrue("non-structural progress must not publish synchronously", s.state.value === before)
        advanceUntilIdle()
        assertEquals(20L, s.state.value.speeds.getValue("a").down)
    }

    @Test
    fun sectionChangedReplacesTheSectionImmediatelyAndNoticesDoNotTouchState() = runTest {
        val s = store()
        s.apply(
            HostSignal.Snapshot(
                HostSnapshot(
                    info = HostInfo("t", "1", 7, emptySet()),
                    daemonConnected = true,
                    tasks = emptyList(),
                    runtime = emptyMap(),
                    queues = emptyList(),
                    queueOrder = emptyList(),
                    groups = emptyList(),
                    stats = RuntimeStats(),
                    priorityTaskId = null,
                    pendingSelections = emptyList(),
                    config = emptyMap(),
                    configRevision = 1,
                    rssSources = emptyList(),
                    cloudDevices = emptyList(),
                    linkDevices = emptyList(),
                    sections = mapOf("agent.gateway" to """{"takeoverEnabled":false}"""),
                ),
            ),
        )
        assertEquals("""{"takeoverEnabled":false}""", s.state.value.sections["agent.gateway"])
        s.apply(HostSignal.Event(HostEvent.SectionChanged("agent.gateway", """{"takeoverEnabled":true}""")))
        assertEquals("""{"takeoverEnabled":true}""", s.state.value.sections["agent.gateway"])
        val before = s.state.value
        s.apply(HostSignal.Event(HostEvent.Notice("captureTasksStarted", """["t1"]""")))
        advanceUntilIdle()
        assertEquals(before, s.state.value)
    }

    private fun keys(vararg pairs: Pair<String, Long>) = pairs.map { QueueOrderEntry(it.first, it.second) }

    private fun HostStore.rank(id: String) = state.value.queueOrder.rank(id)

    @Test
    fun queueOrderDeltaKeepsRanksByKeyAndDoesNotRekeyOthers() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Pending), task("b", TaskStatus.Pending), task("c", TaskStatus.Pending)))
        s.apply(HostSignal.Event(HostEvent.QueuePositionsChanged(keys("a" to 10, "b" to 20, "c" to 30))))
        advanceUntilIdle()
        assertEquals(listOf(1, 2, 3), listOf("a", "b", "c").map { s.rank(it) })

        // 队首出队：其余任务的键不变，名次整体前移。
        s.apply(HostSignal.Event(HostEvent.QueueOrderChanged(listOf("a"), emptyList())))
        advanceUntilIdle()
        assertEquals(listOf(0, 1, 2), listOf("a", "b", "c").map { s.rank(it) })
        assertEquals(20L, s.state.value.queueOrder.orderOf("b"))

        // 新增排在键的名次位置；同任务多次出现以最后一项为准；已有任务按新键移动。
        s.apply(
            HostSignal.Event(
                HostEvent.QueueOrderChanged(emptyList(), keys("d" to 5, "d" to 25, "b" to 40)),
            ),
        )
        advanceUntilIdle()
        assertEquals(listOf("d", "c", "b"), s.state.value.queueOrder.entries.map { it.taskId })
        assertEquals(listOf(1, 2, 3), listOf("d", "c", "b").map { s.rank(it) })

        // 全量消息整表替换。
        s.apply(HostSignal.Event(HostEvent.QueuePositionsChanged(keys("c" to 1, "b" to 2))))
        advanceUntilIdle()
        assertEquals(listOf(0, 2, 1), listOf("d", "b", "c").map { s.rank(it) })
    }

    @Test
    fun queueOrderInstanceIsReusedWhenOrderUnchanged() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Pending), task("b", TaskStatus.Downloading)))
        s.apply(HostSignal.Event(HostEvent.QueuePositionsChanged(keys("a" to 1))))
        advanceUntilIdle()
        val before = s.state.value.queueOrder
        s.apply(progress("b", TaskStatus.Downloading, 5))
        advanceUntilIdle()
        assertTrue(s.state.value.queueOrder === before)
    }

    private fun loadingSnapshot(total: Long) = HostSignal.Snapshot(
        HostSnapshot(
            info = HostInfo("t", "1", 17, emptySet()), daemonConnected = true, tasks = emptyList(), runtime = emptyMap(),
            queues = emptyList(), queueOrder = emptyList(), groups = emptyList(), stats = RuntimeStats(), priorityTaskId = null,
            pendingSelections = emptyList(), config = emptyMap(), configRevision = 1, rssSources = emptyList(),
            cloudDevices = emptyList(), linkDevices = emptyList(),
            tasksLoading = true, taskTotal = total,
        ),
    )

    @Test
    fun loadingSnapshotIsLoadingPhaseNotAnEmptyList() = runTest {
        val s = store()
        assertEquals(TaskTable.Ready, s.state.value.taskTable)

        s.apply(loadingSnapshot(230_000))
        val loading = s.state.value
        assertEquals(Connection.Live, loading.connection)
        assertTrue(loading.tasks.isEmpty())
        assertTrue(loading.tasksLoading)
        assertEquals(TaskTable.Loading(230_000), loading.taskTable)

        s.apply(snapshot(task("a", TaskStatus.Downloading), task("b", TaskStatus.Completed)))
        val ready = s.state.value
        assertFalse(ready.tasksLoading)
        assertEquals(TaskTable.Ready, ready.taskTable)
        assertEquals(listOf("a", "b"), ready.tasks.map { it.taskId })
    }

    @Test
    fun resyncReloadDropsTheStaleListInsteadOfPresentingItAsComplete() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading)))
        assertEquals(TaskTable.Ready, s.state.value.taskTable)

        s.apply(loadingSnapshot(1))
        assertEquals(TaskTable.Loading(1), s.state.value.taskTable)
        assertTrue("半份 / 旧列表不得在载入期间继续当作完整列表", s.state.value.tasks.isEmpty())
    }

    @Test
    fun taskSpeedHistorySurvivesALoadingSnapshot() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading)))
        s.apply(progress("a", TaskStatus.Downloading, 4096))
        advanceUntilIdle()
        assertTrue("a" in s.state.value.taskSpeedHistory)

        s.apply(loadingSnapshot(1))
        s.apply(snapshot(task("a", TaskStatus.Downloading)))
        assertTrue("a" in s.state.value.taskSpeedHistory)
    }

    @Test
    fun taskSpeedHistoryIsPrunedWhenTheCompleteSnapshotLacksTheTask() = runTest {
        val s = store()
        s.apply(snapshot(task("a", TaskStatus.Downloading)))
        s.apply(progress("a", TaskStatus.Downloading, 4096))
        advanceUntilIdle()

        s.apply(loadingSnapshot(1))
        s.apply(snapshot(task("b", TaskStatus.Downloading)))
        assertFalse("a" in s.state.value.taskSpeedHistory)
    }

    private fun session(signals: Flow<HostSignal>): HostSession {
        val unused = Proxy.newProxyInstance(HostSession::class.java.classLoader, arrayOf(HostSession::class.java)) { _, method, _ ->
            throw UnsupportedOperationException(method.name)
        } as HostSession
        return object : HostSession by unused {
            override val signals: Flow<HostSignal> = signals
        }
    }

    @Test
    fun signalStreamEndingWithoutFatalSurfacesAFailureInsteadOfFreezingOnLoading() = runTest {
        val s = store()
        s.attach(session(flowOf(loadingSnapshot(1))))
        advanceUntilIdle()
        val failed = s.state.value.connection
        assertTrue("会话意外结束必须进入失败态（断连横幅），不能停在加载态 Live", failed is Connection.Failed)
        assertEquals(HostErrorCode.Unavailable, (failed as Connection.Failed).error.code)
    }

    @Test
    fun fatalBeforeStreamEndKeepsTheRealReason() = runTest {
        val s = store()
        val fatal = HostException(HostErrorCode.Unauthorized, message = "revoked")
        s.attach(session(flowOf(snapshot(), HostSignal.Fatal(fatal))))
        advanceUntilIdle()
        assertEquals(Connection.Failed(fatal), s.state.value.connection)
    }

    @Test
    fun detachDoesNotReportTheCancelledStreamAsFailure() = runTest {
        val s = store()
        val signals = MutableSharedFlow<HostSignal>(replay = 1)
        signals.emit(snapshot())
        s.attach(session(signals))
        advanceUntilIdle()
        s.detach()
        advanceUntilIdle()
        assertEquals(Connection.Live, s.state.value.connection)
    }

    @Test
    fun checkedExceptionFromTheBindingBecomesFailureNotACrash() = runTest {
        val s = store()
        s.attach(session(flow { emit(snapshot()); throw Exception("uniffi internal") }))
        advanceUntilIdle()
        val failed = s.state.value.connection
        assertTrue(failed is Connection.Failed)
        assertEquals(HostErrorCode.Internal, (failed as Connection.Failed).error.code)
    }
}
