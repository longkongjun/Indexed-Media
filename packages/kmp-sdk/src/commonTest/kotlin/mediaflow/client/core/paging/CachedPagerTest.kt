package mediaflow.client.core.paging

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlinx.io.IOException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class CachedPagerTest {
    private class UnknownHostExceptionFake(
        message: String,
    ) : IOException(message)

    private class FakeCache<T>(
        initial: List<T>,
    ) {
        var snapshot: List<T> = initial
        val writes = mutableListOf<List<T>>()

        fun read(): List<T> = snapshot

        fun write(items: List<T>) {
            snapshot = items
            writes += items
        }
    }

    private fun <T> wall(
        cache: FakeCache<T>,
        fetch: suspend (cursor: String?, limit: Int) -> PageResult<T>,
    ): CachedPager<T> = CachedPager(CursorPager(fetch), cache::read, cache::write)

    @Test
    fun coldStartOfflineKeepsCachedItemsAndMarksOfflineStale() =
        runTest {
            val cache = FakeCache(listOf(1, 2))
            val wall = wall(cache) { _, _ -> throw UnknownHostExceptionFake("nas.local") }
            assertEquals(listOf(1, 2), wall.items.value)
            assertEquals(PageState.Loading(stale = true), wall.state.value)

            assertTrue(wall.reload())
            assertEquals(listOf(1, 2), wall.items.value, "cached snapshot must survive an offline first refresh")
            assertEquals(PageState.Offline(stale = true), wall.state.value)
            assertEquals(listOf(1, 2), cache.snapshot)
            assertTrue(cache.writes.isEmpty())
        }

    @Test
    fun coldStartOfflineWithoutCacheIsOfflineAndNotStale() =
        runTest {
            val cache = FakeCache<Int>(emptyList())
            val wall = wall(cache) { _, _ -> throw UnknownHostExceptionFake("nas.local") }
            assertEquals(PageState.Loading(stale = false), wall.state.value)

            assertTrue(wall.reload())
            assertEquals(emptyList(), wall.items.value)
            assertEquals(PageState.Offline(stale = false), wall.state.value)
            assertTrue(cache.writes.isEmpty())
        }

    @Test
    fun coldStartFailureKeepsCachedItemsAndMarksErrorStale() =
        runTest {
            val cache = FakeCache(listOf(1, 2))
            val wall = wall(cache) { _, _ -> throw IOException("reset") }

            assertTrue(wall.reload())
            assertEquals(listOf(1, 2), wall.items.value)
            val state = assertIs<PageState.Error>(wall.state.value)
            assertTrue(state.stale)
            assertEquals(listOf(1, 2), cache.snapshot)
            assertTrue(cache.writes.isEmpty())
        }

    @Test
    fun initialEmptyPagerFlowDoesNotOverwriteCachedItems() =
        runTest {
            val cache = FakeCache(listOf(1, 2))
            val release = CompletableDeferred<Unit>()
            val wall =
                wall(cache) { _, _ ->
                    release.await()
                    PageResult(listOf(3), null)
                }
            val refresh = async { wall.reload() }
            runCurrent()

            assertEquals(listOf(1, 2), wall.items.value, "pager's initial empty StateFlow must not replace the cache snapshot")
            assertEquals(PageState.Loading(stale = true), wall.state.value)
            assertTrue(cache.writes.isEmpty())

            release.complete(Unit)
            assertTrue(refresh.await())
            assertEquals(listOf(3), wall.items.value)
            assertEquals(PageState.Content, wall.state.value)
            assertEquals(listOf(listOf(3)), cache.writes)
        }

    @Test
    fun authoritativeEmptyFirstPageClearsStaleCacheAndShowsEmpty() =
        runTest {
            val cache = FakeCache(listOf(1, 2))
            val wall = wall(cache) { _, _ -> PageResult(emptyList(), null) }

            assertTrue(wall.reload())
            assertEquals(emptyList(), wall.items.value)
            assertEquals(PageState.Empty, wall.state.value)
            assertEquals(emptyList(), cache.snapshot, "server-authoritative empty page must clear the stale cache")
            assertEquals(listOf(emptyList()), cache.writes)
        }

    @Test
    fun authoritativeEmptyRefreshAfterContentClearsCache() =
        runTest {
            val cache = FakeCache<Int>(emptyList())
            var empty = false
            val wall =
                wall(cache) { _, _ ->
                    if (empty) PageResult(emptyList(), null) else PageResult(listOf(1), null)
                }
            assertTrue(wall.reload())
            assertEquals(listOf(1), cache.snapshot)

            empty = true
            assertTrue(wall.reload())
            assertEquals(emptyList(), wall.items.value)
            assertEquals(PageState.Empty, wall.state.value)
            assertEquals(emptyList(), cache.snapshot, "authoritative empty refresh must clear the previously cached wall")
        }

    @Test
    fun supersededLateSuccessCannotWriteItemsOrCache() =
        runTest {
            val cache = FakeCache<Int>(emptyList())
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            var calls = 0
            val wall =
                wall(cache) { _, _ ->
                    if (++calls == 1) {
                        started.complete(Unit)
                        release.await()
                        PageResult(listOf(1), null)
                    } else {
                        PageResult(listOf(2), null)
                    }
                }
            val old = async { wall.reload() }
            started.await()
            assertTrue(wall.reload())
            release.complete(Unit)

            assertFalse(old.await())
            assertEquals(listOf(2), wall.items.value)
            assertEquals(PageState.Content, wall.state.value)
            assertEquals(listOf(listOf(2)), cache.writes, "only the winning refresh may write the cache")
        }

    @Test
    fun supersededNextPageCannotAppendOrWriteCacheAfterReload() =
        runTest {
            val cache = FakeCache<Int>(emptyList())
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            var first = true
            val wall =
                wall(cache) { cursor, _ ->
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
                }
            assertTrue(wall.reload())
            val old = async { wall.loadNext() }
            started.await()
            assertEquals(PageState.Loading(stale = true), wall.state.value)
            assertTrue(wall.reload())
            release.complete(Unit)

            assertFalse(old.await())
            assertEquals(listOf(3), wall.items.value)
            assertEquals(PageState.Content, wall.state.value, "superseded loadNext must not leave the wall in Loading")
            assertEquals(listOf(listOf(1), listOf(3)), cache.writes)

            assertTrue(wall.loadNext())
            assertEquals(listOf(3, 4), wall.items.value)
            assertEquals(listOf(listOf(1), listOf(3), listOf(3, 4)), cache.writes)
            assertFalse(wall.hasMore())
        }

    @Test
    fun cancelledReloadKeepsCachedItemsAndWritesNothing() =
        runTest {
            val cache = FakeCache(listOf(1))
            val started = CompletableDeferred<Unit>()
            val wall =
                wall(cache) { _, _ ->
                    started.complete(Unit)
                    awaitCancellation()
                }
            val request = launch { wall.reload() }
            started.await()
            request.cancelAndJoin()

            assertEquals(listOf(1), wall.items.value)
            assertEquals(PageState.Loading(stale = true), wall.state.value)
            assertEquals(listOf(1), cache.snapshot)
            assertTrue(cache.writes.isEmpty())
        }

    @Test
    fun cancelledFetchReturningLateCannotWriteCache() =
        runTest {
            val cache = FakeCache(listOf(1))
            val started = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            val wall =
                wall(cache) { _, _ ->
                    // 不响应取消的适配器可能在调用方取消后仍返回结果。
                    withContext(NonCancellable) {
                        started.complete(Unit)
                        release.await()
                    }
                    PageResult(listOf(9), null)
                }
            val request = launch { wall.reload() }
            started.await()
            request.cancel()
            release.complete(Unit)
            request.join()

            assertEquals(listOf(1), wall.items.value)
            assertEquals(listOf(1), cache.snapshot)
            assertTrue(cache.writes.isEmpty())
        }

    @Test
    fun loadNextAppendsAndCachesFullSnapshotThenStopsAtEnd() =
        runTest {
            val cache = FakeCache<Int>(emptyList())
            val wall =
                wall(cache) { cursor, _ ->
                    if (cursor == null) PageResult(listOf(1), "c2") else PageResult(listOf(2), null)
                }
            assertTrue(wall.reload())
            assertTrue(wall.loadNext())
            assertEquals(listOf(1, 2), wall.items.value)
            assertEquals(PageState.Content, wall.state.value)
            assertEquals(listOf(listOf(1), listOf(1, 2)), cache.writes)
            assertFalse(wall.hasMore())

            assertFalse(wall.loadNext())
            assertEquals(PageState.Content, wall.state.value, "exhausted loadNext must not leave the wall in Loading")
            assertEquals(2, cache.writes.size)
        }

    @Test
    fun loadNextFailureKeepsItemsAndCacheAndMarksStaleError() =
        runTest {
            val cache = FakeCache<Int>(emptyList())
            var failNext = false
            val wall =
                wall(cache) { _, _ ->
                    if (failNext) throw IOException("reset") else PageResult(listOf(1), "c2")
                }
            assertTrue(wall.reload())
            failNext = true

            assertTrue(wall.loadNext())
            val state = assertIs<PageState.Error>(wall.state.value)
            assertTrue(state.stale)
            assertEquals(listOf(1), wall.items.value)
            assertEquals(listOf(listOf(1)), cache.writes)
        }

    @Test
    fun offlineRefreshAfterContentKeepsItemsAndCache() =
        runTest {
            val cache = FakeCache<Int>(emptyList())
            var fail = false
            val wall =
                wall(cache) { _, _ ->
                    if (fail) throw UnknownHostExceptionFake("nas.local") else PageResult(listOf(1), null)
                }
            assertTrue(wall.reload())
            fail = true

            assertTrue(wall.reload())
            assertEquals(listOf(1), wall.items.value)
            assertEquals(PageState.Offline(stale = true), wall.state.value)
            assertEquals(listOf(listOf(1)), cache.writes)
        }
}
