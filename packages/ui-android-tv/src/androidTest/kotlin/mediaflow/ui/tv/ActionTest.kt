package mediaflow.ui.tv

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.MaterialTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class ActionTest {
    @get:Rule val compose = createComposeRule()

    @Test fun busyKeepsNameAndBlocksActivation() {
        var events = 0
        compose.setContent {
            MfTvTheme {
                MfTvButton("Continue", onClick = { events++ }, busy = true)
            }
        }
        compose.onNodeWithText("Continue").assertIsNotEnabled().performClick()
        compose.runOnIdle { assertEquals(0, events) }
    }

    @Test fun enabledActionEmitsOnce() {
        var events = 0
        compose.setContent {
            MfTvTheme { MfTvButton("Continue", onClick = { events++ }) }
        }
        compose.onNodeWithText("Continue").performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        compose.onNodeWithText("Continue").performKeyInput {
            keyDown(Key.DirectionCenter)
            keyUp(Key.DirectionCenter)
        }
        compose.runOnIdle { assertEquals(1, events) }
    }

    @Test
    fun directionRightMovesFocusWithoutActivating() {
        var events = 0
        compose.setContent {
            MfTvTheme {
                Row {
                    MfTvButton("First", onClick = { events++ })
                    MfTvButton("Second", onClick = { events++ })
                }
            }
        }
        compose.onNodeWithText("First").performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        compose.onNodeWithText("First").performKeyInput {
            keyDown(Key.DirectionRight)
            keyUp(Key.DirectionRight)
        }
        compose.onNodeWithText("Second").assertIsFocused()
        compose.runOnIdle { assertEquals(0, events) }
    }

    @Test
    fun buttonUsesTvTextAndRetainsFocusAcrossThemeChange() {
        val dark = mutableStateOf(true)
        compose.setContent {
            MfTvTheme(dark = dark.value) {
                Box(Modifier.background(MaterialTheme.colorScheme.background).padding(24.dp).testTag("canvas")) {
                    MfTvButton("Continue", onClick = {}, modifier = Modifier.testTag("action"))
                }
            }
        }
        compose.onNodeWithText("Continue").performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        val layouts = mutableListOf<TextLayoutResult>()
        compose
            .onNodeWithText("Continue", useUnmergedTree = true)
            .performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
        assertEquals(
            20.sp,
            layouts
                .single()
                .layoutInput.style.fontSize,
        )
        for (isDark in listOf(true, false)) {
            compose.runOnIdle { dark.value = isDark }
            compose.onNodeWithTag("action").assertIsFocused()
            val bounds = compose.onNodeWithTag("action").fetchSemanticsNode().boundsInRoot
            assertTrue("Focused action retains a nonempty layout", bounds.width > 0 && bounds.height > 0)
        }
    }

    @Test
    fun decorativeArtworkDoesNotRepeatAccessibleName() {
        compose.setContent {
            MfTvTheme {
                MfTvContentCard("Session", "Room A", onClick = {}, artwork = {
                    Box(Modifier.size(24.dp).semantics { contentDescription = "Poster" })
                })
            }
        }
        compose.onNodeWithContentDescription("Poster").assertDoesNotExist()
        compose.onNodeWithText("Session").assertExists()
    }
}
