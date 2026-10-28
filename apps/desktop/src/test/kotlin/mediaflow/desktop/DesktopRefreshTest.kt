package mediaflow.desktop

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.unit.dp
import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import dev.indexed.design.desktop.IndexedTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.paging.PageState
import org.junit.Rule
import org.junit.Test
import java.awt.EventQueue
import java.awt.Frame
import java.awt.event.WindowEvent
import java.net.InetSocketAddress
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.assertEquals

class DesktopRefreshTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun windowFocusRefreshListenerUsesTheLatestCallbackAndIsRemovedOnDispose() {
        lateinit var window: Frame
        EventQueue.invokeAndWait { window = Frame() }
        val calls = AtomicInteger()
        var observing by mutableStateOf(true)
        var changed by mutableStateOf(false)
        try {
            compose.setContent {
                val amount = if (changed) 2 else 1
                if (observing) RefreshOnWindowFocus(window) { calls.addAndGet(amount) }
            }
            compose.waitForIdle()
            assertEquals(1, window.windowFocusListeners.size)
            EventQueue.invokeAndWait {
                window.windowFocusListeners.single().windowGainedFocus(WindowEvent(window, WindowEvent.WINDOW_GAINED_FOCUS))
            }
            assertEquals(1, calls.get())
            compose.runOnIdle { changed = true }
            compose.waitForIdle()
            assertEquals(1, window.windowFocusListeners.size)
            EventQueue.invokeAndWait {
                window.windowFocusListeners.single().windowGainedFocus(WindowEvent(window, WindowEvent.WINDOW_GAINED_FOCUS))
            }
            assertEquals(3, calls.get())
            compose.runOnIdle { observing = false }
            compose.waitForIdle()
            assertEquals(0, window.windowFocusListeners.size)
        } finally {
            EventQueue.invokeAndWait { window.dispose() }
        }
    }

    @Test
    fun enteringEachSectionRevalidatesItsProjection() =
        withCompanion { _, server ->
            val tasksBefore = server.taskRequests.get()
            compose.onNodeWithText("任务").performClick()
            compose.waitUntil(10_000) { server.taskRequests.get() > tasksBefore }
            compose.waitForIdle()
            val casesBefore = server.caseRequests.get()
            compose.onNodeWithText("人工确认").performClick()
            compose.waitUntil(10_000) { server.caseRequests.get() > casesBefore }
            compose.waitForIdle()
            val mediaBefore = server.mediaRequests.get()
            compose.onNodeWithText("媒体").performClick()
            compose.waitUntil(10_000) { server.mediaRequests.get() > mediaBefore }
        }

    @Test
    fun refreshRevalidatesTheCurrentlyOpenDetails() =
        withCompanion { model, server ->
            compose.onNodeWithText("沙丘").performClick()
            compose.waitUntil(10_000) { model.state.value.detail != null }
            server.updatedMedia.set(true)
            compose.runOnIdle { model.refresh() }
            compose.waitUntil(10_000) {
                model.state.value.detail
                    ?.item
                    ?.title == "沙丘（更新）"
            }
            compose.onNodeWithText("沙丘（更新）").assertIsDisplayed()
            compose.onNodeWithText("人工确认").performClick()
            compose.onNodeWithText("流浪地球").performClick()
            compose.waitUntil(10_000) {
                model.state.value.selectedCase
                    ?.version == 1L
            }
            compose.onNodeWithText("候选搜索").performTextReplacement("保留查询草稿")
            compose.onNodeWithText("保留查询草稿").assertIsDisplayed()
            server.updatedCase.set(true)
            compose.runOnIdle { model.refresh() }
            compose.waitUntil(10_000) {
                model.state.value.selectedCase
                    ?.version == 2L
            }
            compose.onNodeWithText("案件版本：2").assertIsDisplayed()
            compose.onNodeWithText("保留查询草稿").assertIsDisplayed()
        }

    @Test
    fun unknownTaskCancelDoesNotOfferReviewResubmission() =
        withCompanion { model, _ ->
            compose.onNodeWithText("人工确认").performClick()
            compose.onNodeWithText("流浪地球").performClick()
            compose.waitUntil(10_000) { model.state.value.selectedCase != null }
            compose.onNodeWithText("任务").performClick()
            compose.onNodeWithText("取消任务").performClick()
            compose.onNodeWithText("确认提交").performClick()
            compose.waitUntil(10_000) { model.state.value.writeUnknown }
            compose.onNodeWithText("人工确认").performClick()
            compose.onNodeWithText("重试同一次提交").assertDoesNotExist()
            compose.onNodeWithText("上次任务取消结果未知，请返回任务页核对。").assertIsDisplayed()
        }

    @Test
    fun unknownReviewDoesNotOfferTaskCancellationRetry() =
        withCompanion { model, _ ->
            compose.onNodeWithText("人工确认").performClick()
            compose.onNodeWithText("流浪地球").performClick()
            compose.waitUntil(10_000) { model.state.value.selectedCase != null }
            compose.onNodeWithText("标记为通用视频").performClick()
            compose.onNodeWithText("确认提交").performClick()
            compose.waitUntil(10_000) { model.state.value.writeUnknown }
            compose.onNodeWithText("任务").performClick()
            compose.onNodeWithText("重试同一次取消").assertDoesNotExist()
            compose.onNodeWithText("上次人工确认提交结果未知，请返回人工确认页核对。").assertIsDisplayed()
        }

    private fun withCompanion(check: (DesktopModel, RefreshServer) -> Unit) {
        RefreshServer().use { server ->
            val directory = Files.createTempDirectory("desktop-refresh")
            val store = InMemoryDeviceAuthStore()
            runBlocking { store.save(StoredCredentials.DeviceToken("refresh-token", "refresh-device")) }
            val model = DesktopModel(directory, Dispatchers.Main, { store })
            try {
                compose.setContent {
                    Box(Modifier.size(1000.dp, 720.dp)) { IndexedTheme { Companion(model) } }
                }
                compose.runOnIdle { model.login(server.url, true, "", "", "", restore = true) }
                compose.waitUntil(10_000) {
                    model.state.value.let {
                        it.loggedIn &&
                            it.wall.isNotEmpty() &&
                            it.tasks.isNotEmpty() &&
                            it.cases.isNotEmpty() &&
                            it.wallState == PageState.Content &&
                            it.taskState == PageState.Content &&
                            it.caseState == PageState.Content
                    }
                }
                compose.waitForIdle()
                check(model, server)
            } finally {
                runBlocking { model.close() }
                directory.toFile().deleteRecursively()
            }
        }
    }

    private class RefreshServer : AutoCloseable {
        private val stopped = CountDownLatch(1)
        private val executor = Executors.newCachedThreadPool()
        private val http = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val updatedMedia = AtomicBoolean()
        val updatedCase = AtomicBoolean()
        val mediaRequests = AtomicInteger()
        val taskRequests = AtomicInteger()
        val caseRequests = AtomicInteger()
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
                    path.endsWith("/decisions") || path.endsWith("/cancel") -> {
                        respondJson(
                            exchange,
                            """{"error":{"code":"internal.error","message":"synthetic failure","request_id":"refresh"}}""",
                            503,
                        )
                    }
                    path.contains("/media-items/") -> {
                        val json = fixture("media-item-detail")
                        respondJson(exchange, if (updatedMedia.get()) json.replace("沙丘", "沙丘（更新）") else json)
                    }
                    path.contains("/review-cases/") -> {
                        val json = fixture("review-case")
                        respondJson(exchange, if (updatedCase.get()) json.replace("\"version\":1", "\"version\":2") else json)
                    }
                    path.endsWith("/integrations/jellyfin") -> respondJson(exchange, fixture("jellyfin-integration"))
                    path.endsWith("/media-items") -> respond(exchange, "media-item-page", mediaRequests)
                    path.endsWith("/processing-tasks") -> respond(exchange, "processing-task-page", taskRequests)
                    path.endsWith("/review-cases") -> respond(exchange, "review-case-page", caseRequests)
                    else -> {
                        exchange.sendResponseHeaders(404, -1)
                        exchange.close()
                    }
                }
            }
            http.start()
        }

        private fun fixture(name: String) = Files.readString(Path.of("../../contracts/examples/v1/$name.json"))

        private fun respond(
            exchange: HttpExchange,
            fixtureName: String,
            count: AtomicInteger,
        ) {
            respondJson(exchange, fixture(fixtureName))
            count.incrementAndGet()
        }

        private fun respondJson(
            exchange: HttpExchange,
            json: String,
            status: Int = 200,
        ) {
            val bytes = json.toByteArray()
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(status, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }

        override fun close() {
            stopped.countDown()
            http.stop(0)
            executor.shutdownNow()
        }
    }
}
