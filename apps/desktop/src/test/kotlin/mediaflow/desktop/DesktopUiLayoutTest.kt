package mediaflow.desktop

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import dev.indexed.design.desktop.IndexedTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import org.junit.Rule
import org.junit.Test
import java.net.InetSocketAddress
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors

class DesktopUiLayoutTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun mediaListHasBoundedHeightAfterLogin() =
        withLoggedInCompanion { _ ->
            compose.onNodeWithText("沙丘").assertIsDisplayed().performClick()
            compose.onNodeWithText("返回媒体列表").assertIsDisplayed().performClick()
            compose.onNodeWithText("沙丘").assertIsDisplayed()
        }

    @Test
    fun taskListHasBoundedHeightAfterNavigation() =
        withLoggedInCompanion { _ ->
            compose.onNodeWithText("任务").performClick()
            compose.onNodeWithText(TASK_PATH).assertIsDisplayed()
            compose.onNodeWithText("取消任务").assertIsDisplayed().performClick()
            compose
                .onNodeWithText("取消")
                .assertIsDisplayed()
                .assertIsFocused()
                .performClick()
            compose.onNodeWithText(TASK_PATH).assertIsDisplayed()
        }

    @Test
    fun reviewListHasBoundedHeightAfterNavigation() =
        withLoggedInCompanion { model ->
            compose.onNodeWithText("人工确认").performClick()
            compose.onNodeWithText("流浪地球").assertIsDisplayed().performClick()
            compose.waitUntil(10_000) { model.state.value.selectedCase != null }
            compose.onNodeWithText("返回审核队列").assertIsDisplayed().performClick()
            compose.onNodeWithText("流浪地球").assertIsDisplayed()
        }

    private fun withLoggedInCompanion(check: (DesktopModel) -> Unit) {
        UiFixtureServer().use { server ->
            val directory = Files.createTempDirectory("mediaflow-desktop-layout")
            val store = InMemoryDeviceAuthStore()
            runBlocking { store.save(StoredCredentials.DeviceToken("synthetic-layout-token", "layout-fixture-device")) }
            val model = DesktopModel(directory, Dispatchers.Main, { store })
            try {
                compose.setContent {
                    Box(Modifier.size(1000.dp, 720.dp)) {
                        IndexedTheme { Companion(model) }
                    }
                }
                compose.runOnIdle { model.login(server.url, true, "", "", "", restore = true) }
                compose.waitUntil(10_000) {
                    val state = model.state.value
                    state.loggedIn && state.wall.isNotEmpty() && state.tasks.isNotEmpty() && state.cases.isNotEmpty()
                }
                compose.waitForIdle()
                check(model)
            } finally {
                runBlocking { model.close() }
                directory.toFile().deleteRecursively()
            }
        }
    }

    private class UiFixtureServer : AutoCloseable {
        private val stopped = CountDownLatch(1)
        private val executor = Executors.newCachedThreadPool()
        private val http = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val url get() = "http://127.0.0.1:${http.address.port}"

        init {
            http.executor = executor
            http.createContext("/api/v1") { exchange ->
                val path = exchange.requestURI.path
                when {
                    path.endsWith("/events") -> {
                        exchange.responseHeaders.add("Content-Type", "text/event-stream")
                        exchange.sendResponseHeaders(200, 0)
                        exchange.responseBody.write(": connected\n\n".toByteArray())
                        exchange.responseBody.flush()
                        stopped.await()
                        exchange.close()
                    }
                    path.endsWith("/media-items") -> respond(exchange, "media-item-page")
                    path.contains("/media-items/") -> respond(exchange, "media-item-detail")
                    path.endsWith("/processing-tasks") -> respond(exchange, "processing-task-page")
                    path.endsWith("/review-cases") -> respond(exchange, "review-case-page")
                    path.contains("/review-cases/") -> respond(exchange, "review-case")
                    path.endsWith("/integrations/jellyfin") -> respond(exchange, "jellyfin-integration")
                    else -> {
                        exchange.sendResponseHeaders(404, -1)
                        exchange.close()
                    }
                }
            }
            http.start()
        }

        private fun respond(
            exchange: HttpExchange,
            fixture: String,
        ) {
            val bytes = Files.readAllBytes(Path.of("../../contracts/examples/v1/$fixture.json"))
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(200, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }

        override fun close() {
            stopped.countDown()
            http.stop(0)
            executor.shutdownNow()
        }
    }

    private companion object {
        const val TASK_PATH = "电影/流浪地球 (2019)/流浪地球 (2019).mkv"
    }
}
