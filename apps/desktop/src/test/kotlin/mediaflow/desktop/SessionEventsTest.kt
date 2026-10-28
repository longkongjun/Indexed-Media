package mediaflow.desktop

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals

@OptIn(ExperimentalCoroutinesApi::class)
class SessionEventsTest {
    @Test fun twoScreensShareOneUpstreamAndPersistBeforeDispatch() =
        runTest {
            var starts = 0
            val persisted = mutableListOf<Int>()
            val source =
                flow {
                    starts++
                    delay(1)
                    emit(42)
                    awaitCancellation()
                }
            val stream = sharedSessionEvents(source, backgroundScope) { persisted += it }
            val first = mutableListOf<Int>()
            val second = mutableListOf<Int>()
            backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) {
                stream.take(1).collect {
                    assertEquals(listOf(42), persisted)
                    first +=
                        it
                }
            }
            backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { stream.take(1).collect { second += it } }
            runCurrent()
            testScheduler.advanceTimeBy(2)
            runCurrent()
            assertEquals(1, starts)
            assertEquals(listOf(42), first)
            assertEquals(first, second)
        }
}
