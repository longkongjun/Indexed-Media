package mediaflow.client.core.paging

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlinx.io.IOException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertSame
import kotlin.test.assertTrue

class CursorPagerTest {
    @Test
    fun reloadLoadsFirstPageAndTracksCursor() =
        runTest {
            val pager =
                CursorPager<Int>(fetch = { cursor, _ ->
                    if (cursor == null) {
                        PageResult(listOf(1, 2), nextCursor = "c2")
                    } else {
                        PageResult(listOf(3), nextCursor = null)
                    }
                })
            assertTrue(pager.reload())
            assertEquals(listOf(1, 2), pager.items.value)
            assertEquals(PageState.Content, pager.state.value)
            assertTrue(pager.hasMore())
        }

    @Test
    fun loadNextAppendsAndStopsAtEnd() =
        runTest {
            var calls = 0
            val pager =
                CursorPager<Int>(fetch = { cursor, _ ->
                    calls += 1
                    if (cursor == null) {
                        PageResult(listOf(1), "c2")
                    } else {
                        PageResult(listOf(2, 3), null)
                    }
                })
            pager.reload()
            assertTrue(pager.loadNext())
            assertEquals(listOf(1, 2, 3), pager.items.value)
            assertFalse(pager.hasMore())
            assertFalse(pager.loadNext())
            assertEquals(2, calls)
        }

    @Test
    fun emptyFirstPageMapsToEmptyState() =
        runTest {
            val pager = CursorPager<Int>(fetch = { _, _ -> PageResult(emptyList(), null) })
            pager.reload()
            assertEquals(PageState.Empty, pager.state.value)
        }

    @Test
    fun offlineFailureKeepsContentAndMarksOfflineStale() =
        runTest {
            var fail = false
            val pager =
                CursorPager<Int>(fetch = { _, _ ->
                    if (!fail) {
                        PageResult(listOf(1), null)
                    } else {
                        throw UnknownHostExceptionFake("nas.local")
                    }
                })
            pager.reload()
            fail = true
            pager.reload()
            val state = pager.state.value
            assertIs<PageState.Offline>(state)
            assertTrue(state.stale)
            assertEquals(listOf(1), pager.items.value)
        }

    @Test
    fun concurrentReloadDropsStaleResponsesAndKeepsLatest() =
        runTest {
            coroutineScope {
                var slowFirst = true
                val pager =
                    CursorPager<Int>(fetch = { cursor, _ ->
                        if (cursor != null) return@CursorPager PageResult(emptyList(), null)
                        // 第一页慢响应：晚于第二代次完成，必须被丢弃。
                        if (slowFirst) kotlinx.coroutines.delay(50)
                        PageResult(List(5) { it }, null)
                    })
                val first = async { pager.reload() }
                kotlinx.coroutines.delay(10)
                slowFirst = false
                val second = async { pager.reload() }
                assertFalse(first.await(), "stale reload result must be dropped")
                assertTrue(second.await(), "latest reload result applies")
            }
        }

    @Test
    fun loadNextFailurePreservesItemsAndMarksStaleError() =
        runTest {
            var failNext = false
            val pager =
                CursorPager<Int>(fetch = { _, _ ->
                    if (failNext) throw IOException("reset") else PageResult(listOf(1), "c2")
                })
            pager.reload()
            failNext = true
            // 失败也由该代次写入状态（Error+stale），返回 true 表示状态归它所有。
            assertTrue(pager.loadNext())
            val state = assertIs<PageState.Error>(pager.state.value)
            assertTrue(state.stale)
            assertEquals(listOf(1), pager.items.value)
        }

    @Test
    fun reloadPropagatesCancellationWithoutMappingItToError() =
        runTest {
            val cancellation = CancellationException("cancel reload")
            val pager = CursorPager<Int>(fetch = { _, _ -> throw cancellation })

            assertSame(cancellation, assertFailsWith<CancellationException> { pager.reload() })
            assertEquals(PageState.Loading(stale = false), pager.state.value)
            assertEquals(emptyList(), pager.items.value)
            assertFalse(pager.hasMore())
        }

    @Test
    fun cancelledNextPagePreservesItemsAndCursorForRetry() =
        runTest {
            val cancellation = CancellationException("cancel next page")
            var cancelNext = true
            val cursors = mutableListOf<String?>()
            val pager =
                CursorPager<Int>(fetch = { cursor, _ ->
                    cursors += cursor
                    when {
                        cursor == null -> PageResult(listOf(1), "c2")
                        cancelNext -> throw cancellation
                        else -> PageResult(listOf(2), null)
                    }
                })
            pager.reload()

            assertSame(cancellation, assertFailsWith<CancellationException> { pager.loadNext() })
            assertEquals(listOf(1), pager.items.value)
            assertEquals(PageState.Loading(stale = true), pager.state.value)
            assertTrue(pager.hasMore())
            cancelNext = false
            assertTrue(pager.loadNext())
            assertEquals(listOf(null, "c2", "c2"), cursors)
            assertEquals(listOf(1, 2), pager.items.value)
            assertEquals(PageState.Content, pager.state.value)
            assertFalse(pager.hasMore())
        }

    @Test
    fun cancellingSuspendedReloadRunsCleanupAndAllowsReload() =
        runTest {
            val started = CompletableDeferred<Unit>()
            var first = true
            var cleanedUp = false
            val pager =
                CursorPager<Int>(fetch = { _, _ ->
                    if (first) {
                        first = false
                        started.complete(Unit)
                        try {
                            awaitCancellation()
                        } finally {
                            cleanedUp = true
                        }
                    }
                    PageResult(listOf(2), null)
                })
            val request = launch { pager.reload() }
            started.await()
            request.cancelAndJoin()

            assertTrue(cleanedUp)
            assertEquals(PageState.Loading(stale = false), pager.state.value)
            assertTrue(pager.reload())
            assertEquals(listOf(2), pager.items.value)
            assertEquals(PageState.Content, pager.state.value)
        }

    @Test
    fun cancelledFetchCannotPublishAResultReturnedDuringCleanup() =
        runTest {
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            val pager =
                CursorPager<Int>(fetch = { _, _ ->
                    // 不响应取消的适配器可能在调用方取消后仍返回结果。
                    withContext(NonCancellable) {
                        started.complete(Unit)
                        release.await()
                    }
                    PageResult(listOf(1), "c2")
                })
            val request = launch { pager.reload() }
            started.await()
            request.cancel()
            release.complete(Unit)
            request.join()

            assertEquals(emptyList(), pager.items.value)
            assertFalse(pager.hasMore())
            assertEquals(PageState.Loading(stale = false), pager.state.value)
        }

    @Test
    fun cancelledFetchCannotMapAFailureThrownDuringCleanup() =
        runTest {
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            val pager =
                CursorPager<Int>(fetch = { _, _ ->
                    withContext(NonCancellable) {
                        started.complete(Unit)
                        release.await()
                    }
                    throw IOException("adapter failed after cancellation")
                })
            val request = launch { pager.reload() }
            started.await()
            request.cancel()
            release.complete(Unit)
            request.join()

            assertEquals(emptyList(), pager.items.value)
            assertFalse(pager.hasMore())
            assertEquals(PageState.Loading(stale = false), pager.state.value)
        }

    @Test
    fun supersededCancellationStillPropagatesWithoutChangingLatestState() =
        runTest {
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            val cancellation = CancellationException("cancel superseded reload")
            var calls = 0
            val pager =
                CursorPager<Int>(fetch = { _, _ ->
                    if (++calls == 1) {
                        started.complete(Unit)
                        release.await()
                        throw cancellation
                    }
                    PageResult(listOf(2), "latest")
                })
            val old = async { pager.reload() }
            started.await()
            assertTrue(pager.reload())
            release.complete(Unit)

            assertEquals(cancellation.message, assertFailsWith<CancellationException> { old.await() }.message)
            assertEquals(listOf(2), pager.items.value)
            assertEquals(PageState.Content, pager.state.value)
            assertTrue(pager.hasMore())
        }

    @Test
    fun supersededFailureCannotReplaceLatestContent() =
        runTest {
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            var calls = 0
            val pager =
                CursorPager<Int>(fetch = { _, _ ->
                    if (++calls == 1) {
                        started.complete(Unit)
                        release.await()
                        throw IOException("late failure")
                    }
                    PageResult(listOf(2), null)
                })
            val old = async { pager.reload() }
            started.await()
            assertTrue(pager.reload())
            release.complete(Unit)

            assertFalse(old.await())
            assertEquals(listOf(2), pager.items.value)
            assertEquals(PageState.Content, pager.state.value)
            assertFalse(pager.hasMore())
        }

    @Test
    fun supersededNextPageCannotAppendToReloadedItemsOrReplaceCursor() =
        runTest {
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            var first = true
            val cursors = mutableListOf<String?>()
            val pager =
                CursorPager<Int>(fetch = { cursor, _ ->
                    cursors += cursor
                    when (cursor) {
                        "old-next" -> {
                            started.complete(Unit)
                            release.await()
                            PageResult(listOf(2), "obsolete")
                        }
                        "new-next" -> PageResult(listOf(4), null)
                        else -> {
                            if (first) {
                                first = false
                                PageResult(listOf(1), "old-next")
                            } else {
                                PageResult(listOf(3), "new-next")
                            }
                        }
                    }
                })
            pager.reload()
            val old = async { pager.loadNext() }
            started.await()
            assertTrue(pager.reload())
            release.complete(Unit)

            assertFalse(old.await())
            assertEquals(listOf(3), pager.items.value)
            assertEquals(PageState.Content, pager.state.value)
            assertTrue(pager.loadNext())
            assertEquals(listOf(3, 4), pager.items.value)
            assertEquals(listOf(null, "old-next", null, "new-next"), cursors)
            assertFalse(pager.hasMore())
        }

    private class UnknownHostExceptionFake(
        message: String,
    ) : IOException(message)
}
