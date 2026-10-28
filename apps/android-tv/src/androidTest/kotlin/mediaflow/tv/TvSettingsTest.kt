package mediaflow.tv

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.pressKey
import dev.indexed.design.tv.IndexedTvTheme
import org.junit.Rule
import org.junit.Test
import kotlin.test.assertEquals

/** 真实TV待运行；取消/系统Back与主题重组不能触发远端退出。 */
class TvSettingsTest {
    @get:Rule val compose = createComposeRule()
    private val ui = TvUi(connected = true, identity = 7, server = "https://nas.local", deviceName = "Living room")

    @Test
    fun settingsShowsIdentityVersionAndExplicitThemeSelection() {
        val theme = mutableStateOf(TvTheme.System)
        compose.setContent { IndexedTvTheme { TvSettings(ui, theme.value, "0.1.0 (1)", { theme.value = it }, {}) } }
        compose.onNodeWithText("设备：Living room").assertExists()
        compose.onNodeWithText("应用版本：0.1.0 (1)").assertExists()
        compose.onNodeWithText("浅色").performScrollTo().performClick()
        compose.onNodeWithText("✓ 浅色").assertIsSelected()
        compose.runOnIdle { assertEquals(TvTheme.Light, theme.value) }
    }

    @Test
    fun cancelAndBackReturnToExitWithoutLoggingOut() {
        var exits = 0
        compose.setContent { IndexedTvTheme { TvSettings(ui, TvTheme.System, "test", {}, { exits++ }) } }
        compose.onNodeWithText("断开并退出设备").performScrollTo().performClick()
        compose.onNodeWithText("取消").assertIsFocused().performClick()
        compose.onNodeWithText("断开并退出设备").assertIsFocused().performClick()
        compose.onNodeWithText("取消").assertIsFocused().performKeyInput { pressKey(Key.Back) }
        compose.onNodeWithText("断开并退出设备").assertIsFocused()
        compose.runOnIdle { assertEquals(0, exits) }
    }

    @Test
    fun confirmationKeepsItsIdentityAndCancelFocusDuringRecomposition() {
        val theme = mutableStateOf(TvTheme.System)
        val targets = mutableListOf<Long>()
        compose.setContent { IndexedTvTheme { TvSettings(ui, theme.value, "test", {}, { targets.add(it) }) } }
        compose.onNodeWithText("断开并退出设备").performScrollTo().performClick()
        compose.runOnIdle { theme.value = TvTheme.Dark }
        compose.onNodeWithText("取消").assertIsFocused()
        compose.onNodeWithText("退出此设备").performClick()
        compose.runOnIdle { assertEquals(listOf(7L), targets) }
    }
}
