package mediaflow.desktop

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.unit.dp
import dev.indexed.design.desktop.IndexedTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import org.junit.Rule
import org.junit.Test
import java.nio.file.Files
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class DesktopDiscoveryUiTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun mediaSearchAppliesDraftFiltersAndCanClearThem() =
        withCompanion { model, server ->
            compose.onNodeWithText("搜索媒体").performTextInput("new")
            assertTrue(synchronized(server.mediaQueries) { server.mediaQueries.none { it["q"] == "new" } })
            compose.onNodeWithText("媒体类型：全部类型").performClick()
            compose.onNodeWithText("电影").performClick()
            compose.onNodeWithText("媒体库：全部媒体库").performClick()
            compose.onNodeWithText("电影库").performClick()
            compose.onNodeWithText("入库状态：全部状态").performClick()
            compose.onNodeWithText("部分入库").performClick()
            compose.onNodeWithText("应用筛选").performClick()
            compose.waitUntil(10_000) {
                model.state.value.wall
                    .firstOrNull()
                    ?.title == "New result"
            }
            compose.onNodeWithText("New result").assertIsDisplayed()
            val query = synchronized(server.mediaQueries) { server.mediaQueries.last() }
            assertEquals("movie", query["type"])
            assertEquals(LIBRARY, query["library_id"])
            assertEquals("partial", query["local_status"])
            compose.onNodeWithText("清除筛选").performClick()
            compose.waitUntil(10_000) {
                model.state.value.wall
                    .firstOrNull()
                    ?.title == "Default result"
            }
            compose.onNodeWithText("Default result").assertIsDisplayed()
            compose.onNodeWithText("搜索媒体").performTextInput("missing")
            compose.onNodeWithText("搜索媒体").performImeAction()
            compose.waitUntil(10_000) { model.state.value.wallState == mediaflow.client.core.paging.PageState.Empty }
            compose.onNodeWithText("没有符合筛选条件的媒体，请调整或清除筛选").assertIsDisplayed()
        }

    @Test
    fun taskDetailsAndRetryUseAnExplicitSafeConfirmation() =
        withCompanion { model, server ->
            compose.onNodeWithText("任务").performClick()
            compose.onNodeWithText("电影/沙丘.mkv").performClick()
            compose.waitUntil(10_000) { model.state.value.selectedTask != null && !model.state.value.taskDetailLoading }
            compose.onNodeWithText("原因：整理计划已暂停，需要处理").assertIsDisplayed()
            compose.onNodeWithText("已尝试：2次").assertIsDisplayed()
            compose.onNodeWithText("重试任务").performClick()
            compose.onNodeWithText("重试任务：电影/沙丘.mkv").assertIsDisplayed()
            compose.onNodeWithText("取消").assertIsFocused().performClick()
            assertTrue(server.retryKeys.isEmpty())
            compose.onNodeWithText("重试任务").performClick()
            compose.onNodeWithText("确认提交").performClick()
            compose.waitUntil(10_000) {
                model.state.value.selectedTask
                    ?.attemptCount == 3L &&
                    !model.state.value.writeBusy
            }
            compose.onNodeWithText("已尝试：3次").assertIsDisplayed()
            assertEquals(1, server.retryKeys.size)
        }

    private fun withCompanion(block: (DesktopModel, DesktopDiscoveryServer) -> Unit) {
        DesktopDiscoveryServer().use { server ->
            val directory = Files.createTempDirectory("desktop-discovery-ui")
            val store = InMemoryDeviceAuthStore()
            runBlocking { store.save(StoredCredentials.DeviceToken("synthetic-discovery-ui", "discovery-ui-device")) }
            val model = DesktopModel(directory, Dispatchers.Main, { store })
            try {
                compose.setContent { Box(Modifier.size(1000.dp, 900.dp)) { IndexedTheme { Companion(model) } } }
                compose.runOnIdle { model.login(server.url, true, "", "", "", restore = true) }
                compose.waitUntil(10_000) {
                    val state = model.state.value
                    state.loggedIn && state.wall.isNotEmpty() && state.tasks.isNotEmpty() && state.mediaLibraries.isNotEmpty()
                }
                compose.waitForIdle()
                block(model, server)
            } finally {
                runBlocking { model.close() }
                directory.toFile().deleteRecursively()
            }
        }
    }
}
