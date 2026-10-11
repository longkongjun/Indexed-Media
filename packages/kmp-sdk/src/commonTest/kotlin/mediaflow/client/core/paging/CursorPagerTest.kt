package mediaflow.client.core.paging

import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.test.runTest
import kotlinx.io.IOException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
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

    private class UnknownHostExceptionFake(
        message: String,
    ) : IOException(message)
}
