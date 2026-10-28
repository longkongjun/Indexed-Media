package mediaflow.desktop

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.pressKey
import androidx.compose.ui.unit.dp
import dev.indexed.design.desktop.IndexedTheme
import org.junit.Rule
import org.junit.Test
import kotlin.test.assertEquals

class DesktopMediaDetailTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun seriesNavigationRestoresTheParentButtonAndReadsVersionFiles() {
        var closed = 0
        compose.setContent {
            Box(
                Modifier.size(600.dp, 800.dp),
            ) { IndexedTheme { DesktopMediaDetail(desktopSeriesFixture()) { closed++ } } }
        }
        compose.onNodeWithText("Season 2").performClick()
        compose.onNodeWithText("Episode 2").performClick()
        compose.onNodeWithText("episode.mkv").assertIsDisplayed()
        compose.onNodeWithText("1.0 MiB").assertIsDisplayed()
        compose.onNodeWithText("返回季详情").performClick()
        compose.onNodeWithText("Episode 2").assertIsFocused()
        compose.onNodeWithText("返回媒体详情").performClick()
        compose.onNodeWithText("Season 2").assertIsFocused()
        compose.onNodeWithText("返回媒体列表").performClick()
        assertEquals(1, closed)
    }

    @Test
    fun escapeWalksTheHierarchyAndAnotherRootDoesNotInheritSelection() {
        val detail = mutableStateOf(desktopSeriesFixture())
        var closed = 0
        compose.setContent { Box(Modifier.size(600.dp, 800.dp)) { IndexedTheme { DesktopMediaDetail(detail.value) { closed++ } } } }
        compose.onNodeWithText("Season 2").performClick()
        compose.onNodeWithText("Episode 2").performClick()
        compose.onNodeWithText("返回季详情").performKeyInput { pressKey(Key.Escape) }
        compose.onNodeWithText("Episode 2").assertIsFocused()
        compose.runOnIdle { detail.value = detail.value.copy(item = detail.value.item.copy(id = "another-root")) }
        compose.onNodeWithText("Season 2").assertIsDisplayed()
        compose.onNodeWithText("返回媒体列表").performClick()
        assertEquals(1, closed)
    }

    @Test
    fun removingSelectedEpisodeDuringRefreshFallsBackToTheSeason() {
        val detail = mutableStateOf(desktopSeriesFixture())
        compose.setContent { Box(Modifier.size(600.dp, 800.dp)) { IndexedTheme { DesktopMediaDetail(detail.value) {} } } }
        compose.onNodeWithText("Season 2").performClick()
        compose.onNodeWithText("Episode 2").performClick()
        compose.runOnIdle { detail.value = detail.value.copy(children = detail.value.children.filterNot { it.id == "e2" }) }
        compose.onNodeWithText("Episode 10").assertIsDisplayed()
        compose.onNodeWithText("返回媒体详情").assertIsDisplayed()
        compose.runOnIdle { detail.value = detail.value.copy(children = detail.value.children.filterNot { it.id == "s2" }) }
        compose.onNodeWithText("Season 7").assertIsDisplayed()
        compose.onNodeWithText("返回媒体列表").assertIsDisplayed()
    }
}
