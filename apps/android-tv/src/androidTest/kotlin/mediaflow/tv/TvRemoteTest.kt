package mediaflow.tv

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.pressKey
import dev.indexed.design.tv.IndexedTvButton
import dev.indexed.design.tv.IndexedTvTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import mediaflow.api.models.MediaItemSummary
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.paging.PageState
import org.junit.Rule
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.time.Instant

/** 编译门禁；需在明确选择的TV设备实际执行，不能由JVM通过代签。 */
class TvRemoteTest {
    @get:Rule val compose = createComposeRule()

    @Test
    fun dpadEdgesToolbarAndDetailReturnKeepStableMediaFocus() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
        val model = TvModel(scope, { error("This focus test must not create a network session") })
        val showingWall = mutableStateOf(true)
        val ui = TvUi(connected = true, page = PageState.Content, wall = (1..12).map(::movie))
        try {
            compose.setContent {
                IndexedTvTheme {
                    val saved = rememberSaveableStateHolder()
                    val images = remember { TvArtworkCache() }
                    if (showingWall.value) {
                        saved.SaveableStateProvider("wall") {
                            Box(Modifier.fillMaxSize()) { TvMediaWall(ui, model, images) }
                        }
                    }
                }
            }
            compose
                .onNodeWithText("Movie 1")
                .assertIsFocused()
                .performKeyInput { pressKey(Key.DirectionLeft) }
                .assertIsFocused()
            compose.onNodeWithText("Movie 1").performKeyInput { pressKey(Key.DirectionRight) }
            compose.onNodeWithText("Movie 2").assertIsFocused()
            compose.runOnIdle { showingWall.value = false }
            compose.runOnIdle { showingWall.value = true }
            compose.onNodeWithText("Movie 2").assertIsFocused().performKeyInput { pressKey(Key.DirectionUp) }
            compose.onNodeWithText("搜索：输入片名").assertIsFocused().performKeyInput { pressKey(Key.DirectionDown) }
            compose.onNodeWithText("Movie 2").assertIsFocused()
            compose.onNodeWithText("清除筛选").performClick()
            compose.onNodeWithText("Movie 2").assertIsFocused()
        } finally {
            model.close()
            scope.cancel()
        }
    }

    @Test
    fun filterCancelIsInitialFocusAndDoesNotApplyDraft() {
        var applied = 0
        var closed = 0
        compose.setContent { IndexedTvTheme { TvFilters(TvUi(filter = MediaFilter(type = "movie")), { applied++ }, { closed++ }) } }
        compose.onNodeWithText("取消").assertIsFocused()
        compose.onNodeWithText("剧集").performClick()
        compose.onNodeWithText("取消").performClick()
        compose.runOnIdle {
            assertEquals(0, applied)
            assertEquals(1, closed)
        }
    }

    @Test
    fun detailInitialFocusWaitsForItsLazyItemAndRefreshDoesNotStealIt() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
        val model = TvModel(scope, { error("No transport required") })
        val ui = mutableStateOf(TvUi(connected = true, detailOpen = true, detailLoading = true))
        try {
            compose.setContent { IndexedTvTheme { TvDetail(ui.value, model, TvArtworkCache()) {} } }
            compose.onNodeWithText("返回影视墙").assertIsFocused()
            compose.onNodeWithText("重新加载详情").performSemanticsAction(SemanticsActions.RequestFocus) { it() }
            compose.runOnIdle { ui.value = ui.value.copy(detailLoading = false) }
            compose.onNodeWithText("重新加载详情").assertIsFocused()
        } finally {
            model.close()
            scope.cancel()
        }
    }

    @Test
    fun longParagraphConsumesDirectionsUntilItsBoundaryThenLeavesFocus() {
        val body = (1..100).joinToString("\n") { "Overview line $it" }
        compose.setContent {
            IndexedTvTheme {
                Column {
                    TvReadableText(body)
                    IndexedTvButton("Next section", {})
                }
            }
        }
        val paragraph = compose.onNodeWithText(body)
        paragraph.performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        paragraph.performKeyInput { pressKey(Key.DirectionDown) }
        compose.waitForIdle()
        val range = paragraph.fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange]
        assertTrue(range.value() > 0f)
        repeat(100) {
            paragraph.performKeyInput { pressKey(Key.DirectionDown) }
            compose.waitForIdle()
        }
        compose.onNodeWithText("Next section").assertIsFocused()
    }

    private fun movie(index: Int) =
        MediaItemSummary(
            id = "movie-$index",
            type = MediaItemSummary.Type.movie,
            libraryId = "library",
            title = "Movie $index",
            year = 2024L,
            localStatus = MediaItemSummary.LocalStatus.complete,
            artworkRef = null,
            updatedAt = Instant.parse("2026-10-28T00:00:00Z"),
        )
}
