package mediaflow.tv

import androidx.compose.foundation.layout.height
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.unit.dp
import dev.indexed.design.tv.IndexedTvTheme
import mediaflow.api.models.MediaChild
import mediaflow.api.models.MediaFileAsset
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.MediaVersion
import org.junit.Rule
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.time.Instant

/** 仪器仅编译；运行后才能判断实体TV焦点/滚动。本夹具验证真实UI，不代签登录和Activity重建。 */
class TvSeriesNavigationTest {
    @get:Rule val compose = createComposeRule()

    @Test
    fun seasonEpisodeVersionsAndBackRestoreTheOriginalRows() {
        val state = mutableStateOf(ui())
        compose.setContent { IndexedTvTheme { Screen(state.value, { state.value = state.value.copy(detailPath = it) }) } }
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("Season 20"))
        compose.onNodeWithText("Season 20").performClick()
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("Episode 2"))
        compose.onNodeWithText("Episode 2").performClick()
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("文件：Series/Season 20/Episode 2.mkv", substring = true))
        compose.onNodeWithText("文件：Series/Season 20/Episode 2.mkv", substring = true).assertExists()
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("返回季详情"))
        compose.onNodeWithText("返回季详情").performClick()
        compose.onNodeWithText("Episode 2").assertIsFocused()
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("返回剧集详情"))
        compose.onNodeWithText("返回剧集详情").performClick()
        compose.onNodeWithText("Season 20").assertIsFocused()
        compose.runOnIdle { assertEquals(TvDetailPath(), state.value.detailPath) }
    }

    @Test
    fun savedStateRestoresTheInactiveAncestorAnchor() {
        val state = mutableStateOf(ui())
        val restoration = StateRestorationTester(compose)
        restoration.setContent { IndexedTvTheme { Screen(state.value, { state.value = state.value.copy(detailPath = it) }) } }
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("Season 20"))
        compose.onNodeWithText("Season 20").performClick()
        restoration.emulateSavedInstanceStateRestore()
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("返回剧集详情"))
        compose.onNodeWithText("返回剧集详情").performClick()
        compose.onNodeWithText("Season 20").assertIsFocused()
    }

    @Test
    fun removingTheFocusedRowWithinTheSameParentRestoresTheNearestSurvivor() {
        val state = mutableStateOf(ui())
        compose.setContent { IndexedTvTheme { Screen(state.value, { state.value = state.value.copy(detailPath = it) }) } }
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("Season 20"))
        compose.onNodeWithText("Season 20").performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        compose.onNodeWithText("Season 20").assertIsFocused()
        compose.runOnIdle {
            val detail = state.value.detail!!
            val removed = detail.children.first { it.title == "Season 20" }.id
            state.value =
                state.value.copy(detail = detail.copy(children = detail.children.filterNot { it.id == removed || it.parentId == removed }))
        }
        compose.onNodeWithText("Season 21").assertIsFocused()
    }

    @Test
    fun savedStateKeepsTheSelectedLeafFileReaderInsteadOfReturningToBack() {
        val original = ui()
        val detail = original.detail!!
        val season = detail.children.first { it.title == "Season 20" }
        val episode = detail.children.first { it.parentId == season.id && it.title == "Episode 2" }
        val state = mutableStateOf(original.copy(detailPath = TvDetailPath(season.id, episode.id)))
        val restoration = StateRestorationTester(compose)
        restoration.setContent { IndexedTvTheme { Screen(state.value, { state.value = state.value.copy(detailPath = it) }) } }
        val fileText = "文件：Series/Season 20/Episode 2.mkv"
        compose.onNodeWithTag("series-list").performScrollToNode(hasText(fileText, substring = true))
        compose.onNodeWithText(fileText, substring = true).performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        compose.onNodeWithText(fileText, substring = true).assertIsFocused()
        restoration.emulateSavedInstanceStateRestore()
        compose.onNodeWithText(fileText, substring = true).assertIsFocused()
    }

    @Test
    fun a512CharacterSeasonTitleRemainsReadableWithoutExpandingTheBackButton() {
        val original = ui()
        val detail = original.detail!!
        val season = detail.children.first { it.title == "Season 20" }
        val episode = detail.children.first { it.parentId == season.id && it.title == "Episode 2" }
        val longTitle = "S".repeat(512)
        val state =
            mutableStateOf(
                original.copy(
                    detail = detail.copy(children = detail.children.map { if (it.id == season.id) it.copy(title = longTitle) else it }),
                    detailPath = TvDetailPath(season.id, episode.id),
                ),
            )
        compose.setContent { IndexedTvTheme { Screen(state.value, { state.value = state.value.copy(detailPath = it) }) } }
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("当前季：$longTitle", substring = true))
        compose.onNodeWithText("当前季：$longTitle", substring = true).assertExists()
        compose.onNodeWithTag("series-list").performScrollToNode(hasText("返回季详情"))
        compose.onNodeWithText("返回季详情").performClick()
        compose.runOnIdle { assertEquals(TvDetailPath(seasonId = season.id), state.value.detailPath) }
    }

    @androidx.compose.runtime.Composable
    private fun Screen(
        state: TvUi,
        navigate: (TvDetailPath) -> Unit,
    ) {
        TvDetailPanel(state, onBack = {
            navigate(if (state.detailPath.childId != null) state.detailPath.copy(childId = null) else TvDetailPath())
        }, onSelect = { id ->
            val node = state.detail!!.children.first { it.id == id }
            navigate(if (node.type == MediaChild.Type.season) TvDetailPath(seasonId = id) else state.detailPath.copy(childId = id))
        }, onRefresh = {}, open = {}, modifier = Modifier.height(220.dp).testTag("series-list"))
    }

    private fun uuid(number: Int) = "019f0000-0000-7000-8000-${number.toString().padStart(12, '0')}"

    private fun ui(): TvUi {
        val children =
            (1..25).flatMap { season ->
                val id = uuid(season + 10)
                listOf(MediaChild(id, null, MediaChild.Type.season, "Season $season", season.toLong(), emptyList())) +
                    (1..2).map { episode ->
                        MediaChild(
                            uuid(1000 + season * 10 + episode),
                            id,
                            MediaChild.Type.episode,
                            "Episode $episode",
                            episode.toLong(),
                            listOf(
                                MediaVersion(
                                    uuid(5000 + season * 10 + episode),
                                    "1080p",
                                    listOf(
                                        MediaFileAsset(
                                            uuid(6000 + season * 10 + episode),
                                            "incoming/$season-$episode.mkv",
                                            "Series/Season $season/Episode $episode.mkv",
                                            1_048_576,
                                        ),
                                    ),
                                ),
                            ),
                        )
                    }
            }
        val detail =
            MediaItemDetail(
                MediaItemSummary(
                    uuid(100),
                    MediaItemSummary.Type.series,
                    uuid(51),
                    "Series",
                    2026,
                    MediaItemSummary.LocalStatus.complete,
                    null,
                    Instant.parse("2026-10-28T00:00:00Z"),
                ),
                emptyList(),
                emptyList(),
                children,
                MediaItemDetail.NfoStatus.complete,
                emptySet(),
            )
        return TvUi(connected = true, identity = 1, detail = detail, detailMediaId = uuid(100), detailOpen = true)
    }
}
