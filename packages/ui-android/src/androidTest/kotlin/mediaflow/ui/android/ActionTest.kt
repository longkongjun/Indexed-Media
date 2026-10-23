package mediaflow.ui.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class ActionTest {
    @get:Rule val compose = createComposeRule()

    @Test fun busyKeepsNameAndBlocksActivation() {
        var events = 0
        compose.setContent {
            MfTheme {
                MfButton("Continue", onClick = { events++ }, busy = true)
            }
        }
        compose.onNodeWithText("Continue").assertIsNotEnabled().performClick()
        compose.runOnIdle { assertEquals(0, events) }
    }

    @Test fun enabledActionEmitsOnce() {
        var events = 0
        compose.setContent {
            MfTheme { MfButton("Continue", onClick = { events++ }) }
        }
        compose.onNodeWithText("Continue").performClick()
        compose.runOnIdle { assertEquals(1, events) }
    }

    @Test
    fun decorativeArtworkDoesNotRepeatAccessibleName() {
        compose.setContent {
            MfTheme {
                MfContentCard("Session", "Room A", onClick = {}, artwork = {
                    Box(Modifier.size(24.dp).semantics { contentDescription = "Poster" })
                })
            }
        }
        compose.onNodeWithContentDescription("Poster").assertDoesNotExist()
        compose.onNodeWithText("Session").assertExists()
    }
}
