package mediaflow.desktop

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import dev.indexed.design.desktop.IndexedTheme
import org.junit.Rule
import org.junit.Test
import kotlin.test.assertEquals

class DesktopBrandTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun aboutPresentsFamilyAndProductAndCloses() {
        var closed = 0
        compose.setContent { IndexedTheme { AboutMediaFlow { closed++ } } }
        compose.onNodeWithText("关于 MediaFlow").assertIsDisplayed()
        compose.onNodeWithText("Indexed", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithText("MediaFlow", useUnmergedTree = true).assertIsDisplayed()
        compose.onNodeWithText("关闭").performClick()
        assertEquals(1, closed)
    }
}
