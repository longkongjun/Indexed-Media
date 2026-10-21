package mediaflow.androidapp

import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.paging.PageState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** 页面状态语义映射（ui-contract 对齐）：UI 层唯一展示语义来源。 */
class PageVisualTest {
    @Test
    fun fiveStatesMapToFiveModesWithStale() {
        assertEquals(PageVisual.Mode.Loading, PageVisual.from(PageState.Loading(stale = false)).mode)
        assertEquals(PageVisual.Mode.Loading, PageVisual.from(PageState.Loading(stale = true)).mode)
        assertTrue(PageVisual.from(PageState.Loading(stale = true)).stale)
        assertEquals(PageVisual.Mode.Content, PageVisual.from(PageState.Content).mode)
        assertEquals(PageVisual.Mode.Empty, PageVisual.from(PageState.Empty).mode)
        val offline = PageVisual.from(PageState.Offline(stale = true))
        assertEquals(PageVisual.Mode.Offline, offline.mode)
        assertTrue(offline.stale)
        val error = PageVisual.from(PageState.Error(MediaFlowError.Http(500, null, "", null), stale = false))
        assertEquals(PageVisual.Mode.Error, error.mode)
    }

    @Test
    fun httpSemanticsHaveReadableChineseMessages() {
        fun message(status: Int) = PageVisual.readableMessage(MediaFlowError.Http(status, null, "raw $status", null))
        assertEquals("登录已过期，请重新登录", message(401))
        assertEquals("内容已被他人更新，请刷新后重试", message(409))
        assertEquals("操作过于频繁，请稍后重试", message(429))
        assertEquals("内容不存在或已移除", message(404))
        // 无专属语义的状态码回退到信封 message。
        assertEquals("raw 503", message(503))
    }

    @Test
    fun contentAndEmptyCarryNoMessage() {
        assertNull(PageVisual.from(PageState.Content).message)
        assertEquals("暂无内容", PageVisual.from(PageState.Empty).message)
    }
}
