package mediaflow.desktop

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import dev.indexed.design.desktop.IndexedTheme
import kotlinx.coroutines.CompletableDeferred
import org.junit.Rule
import org.junit.Test
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.assertEquals

class DesktopPosterUiTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun restoresSelectedPosterOnlyWhenReturningFromDetail() {
        val returning = mutableStateOf(false)
        val item = desktopSeriesFixture().item
        compose.setContent {
            IndexedTheme {
                Box(Modifier.size(500.dp, 700.dp)) {
                    DesktopPosterWall(listOf(item), false, false, { null }, {}, {}, selectedId = item.id, restoreFocus = returning.value)
                }
            }
        }
        compose.runOnIdle { returning.value = true }
        compose.waitForIdle()
        compose.onNodeWithText("Series fixture").assertIsFocused()
    }

    @Test
    fun unavailablePosterCanRetryAndMediaRemainsSelectable() {
        val calls = AtomicInteger()
        var selected: String? = null
        val first = CompletableDeferred<Unit>()
        val item = desktopSeriesFixture().item
        compose.setContent {
            IndexedTheme {
                Box(Modifier.size(500.dp, 700.dp)) {
                    DesktopPosterWall(
                        listOf(item),
                        false,
                        false,
                        loadPoster = {
                            if (calls.incrementAndGet() == 1) first.await()
                            null
                        },
                        onSelect = { selected = it },
                        onMore = {},
                    )
                }
            }
        }
        compose.onNodeWithText("正在加载海报").assertIsDisplayed()
        first.complete(Unit)
        compose.waitForIdle()
        compose.onNodeWithText("海报暂不可用").assertIsDisplayed()
        compose.onNodeWithText("重试海报：Series fixture").performClick()
        compose.waitForIdle()
        assertEquals(2, calls.get())
        compose.onNodeWithText("Series fixture").performClick()
        assertEquals(item.id, selected)
    }
}
