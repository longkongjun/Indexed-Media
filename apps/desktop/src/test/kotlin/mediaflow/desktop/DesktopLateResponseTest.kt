package mediaflow.desktop

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import java.net.InetSocketAddress
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import kotlin.coroutines.CoroutineContext
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DesktopLateResponseTest {
    @Test
    fun closeAfterActivationCannotPublishTheCandidateSession() =
        runBlocking {
            LateServer().use { server ->
                val directory = Files.createTempDirectory("desktop-activated-close")
                val candidate = AtomicReference<DesktopSession>()
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { executor ->
                    val ui = ControlledUi(executor)
                    val store = credentials()
                    val model =
                        DesktopModel(directory, ui, { store }) { address, authStore ->
                            DesktopSession(address, authStore).also(candidate::set)
                        }
                    ui.holdWhen = {
                        candidate.get()?.let { session -> runCatching { session.events() }.isSuccess } == true
                    }
                    try {
                        val login =
                            withContext(ui) {
                                ui.captureNextJob().also { model.login(server.url, true, "", "", "", restore = true) }
                            }
                        val pending = withTimeout(10_000) { ui.held.await() }
                        assertTrue(Files.exists(directory.resolve("cache/${cacheIdentity(server.url, DEVICE_ID)}.db")))
                        assertFalse(Files.exists(directory.resolve("server.txt")))
                        val closing = async(start = CoroutineStart.UNDISPATCHED) { model.close() }
                        // UI barrier ensures close has cancelled the held login before it is released.
                        withContext(ui) { }
                        pending.resume()
                        withTimeout(10_000) {
                            closing.await()
                            login.await().join()
                        }
                        assertFalse(model.state.value.loggedIn)
                        assertFalse(Files.exists(directory.resolve("server.txt")))
                        assertEquals(0, server.mediaRequests.get())
                        assertNull(candidate.get().authCache.current())
                    } finally {
                        if (ui.held.isCompleted) ui.held.await().resume()
                        model.close()
                        candidate.get()?.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    @Test
    fun lateDetailSuccessCannotReplaceNewSelection() = detailRace(firstStatus = 200)

    @Test
    fun lateDetailFailureCannotSetAnErrorOnNewSelection() = detailRace(firstStatus = 503)

    private fun detailRace(firstStatus: Int) =
        runBlocking {
            LateServer(firstStatus).use { server ->
                val directory = Files.createTempDirectory("desktop-detail-late")
                val candidate = AtomicReference<DesktopSession>()
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { executor ->
                    val ui = ControlledUi(executor)
                    val store = credentials()
                    val model =
                        DesktopModel(directory, ui, { store }) { address, authStore ->
                            DesktopSession(address, authStore).also(candidate::set)
                        }
                    try {
                        withContext(ui) { model.login(server.url, true, "", "", "", restore = true) }
                        withTimeout(10_000) { model.state.first { it.loggedIn && it.wall.isNotEmpty() } }
                        val first = withContext(ui) { ui.captureNextJob().also { model.detail(FIRST_ID) } }
                        withTimeout(10_000) { server.firstDetailStarted.await() }
                        val second = withContext(ui) { ui.captureNextJob().also { model.detail(SECOND_ID) } }
                        withTimeout(10_000) { second.await().join() }
                        assertEquals(
                            SECOND_ID,
                            model.state.value.detail
                                ?.item
                                ?.id,
                        )
                        assertFalse(model.state.value.detailLoading)
                        server.releaseFirstDetail.countDown()
                        // Joining the original action proves the late result was processed before assertions.
                        withTimeout(10_000) { first.await().join() }
                        val state = model.state.value
                        assertEquals(SECOND_ID, state.detail?.item?.id)
                        assertEquals("new selection", state.detail?.item?.title)
                        assertFalse(state.detailLoading)
                        assertNull(state.message)
                        assertEquals(1, server.playbackRequests.get())
                        withContext(candidate.get().io) {
                            assertNull(candidate.get().cache.cachedDetail(FIRST_ID))
                            assertEquals(
                                SECOND_ID,
                                candidate
                                    .get()
                                    .cache
                                    .cachedDetail(SECOND_ID)
                                    ?.item
                                    ?.id,
                            )
                        }
                    } finally {
                        server.releaseFirstDetail.countDown()
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    private suspend fun credentials() = InMemoryDeviceAuthStore().apply { save(StoredCredentials.DeviceToken("synthetic-late", DEVICE_ID)) }

    private class ControlledUi(
        private val delegate: CoroutineDispatcher,
    ) : CoroutineDispatcher() {
        private val capture = AtomicReference<Pair<Thread, CompletableDeferred<Job>>?>()
        private val holding = AtomicBoolean(false)
        val held = CompletableDeferred<Pending>()

        @Volatile
        var holdWhen: (() -> Boolean)? = null

        fun captureNextJob(): CompletableDeferred<Job> = CompletableDeferred<Job>().also { capture.set(Thread.currentThread() to it) }

        override fun dispatch(
            context: CoroutineContext,
            block: Runnable,
        ) {
            capture.get()?.let { next ->
                // Other IO completions cannot steal the action launched synchronously by the UI thread.
                if (next.first === Thread.currentThread() && capture.compareAndSet(next, null)) {
                    next.second.complete(checkNotNull(context[Job]))
                }
            }
            if (holdWhen?.invoke() == true && holding.compareAndSet(false, true)) {
                held.complete(Pending(delegate, context, block))
            } else {
                delegate.dispatch(context, block)
            }
        }
    }

    private class Pending(
        private val dispatcher: CoroutineDispatcher,
        private val context: CoroutineContext,
        private val block: Runnable,
    ) {
        private val resumed = AtomicBoolean(false)

        fun resume() {
            if (resumed.compareAndSet(false, true)) dispatcher.dispatch(context, block)
        }
    }

    private class LateServer(
        private val firstStatus: Int = 200,
    ) : AutoCloseable {
        private val stopped = CountDownLatch(1)
        private val executor = Executors.newCachedThreadPool()
        private val http = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val firstDetailStarted = CompletableDeferred<Unit>()
        val releaseFirstDetail = CountDownLatch(1)
        val mediaRequests = AtomicInteger()
        val playbackRequests = AtomicInteger()
        val url get() = "http://127.0.0.1:${http.address.port}"

        init {
            http.executor = executor
            http.createContext("/api/v1") { exchange ->
                try {
                    val path = exchange.requestURI.path
                    when {
                        path.endsWith("/events") -> {
                            exchange.responseHeaders.add("Content-Type", "text/event-stream")
                            exchange.sendResponseHeaders(200, 0)
                            exchange.responseBody.write(": connected\n\n".toByteArray())
                            exchange.responseBody.flush()
                            stopped.await()
                        }
                        path.endsWith("/media-items") -> {
                            mediaRequests.incrementAndGet()
                            respond(exchange, fixture("media-item-page"))
                        }
                        path.endsWith("/media-items/$FIRST_ID") -> {
                            firstDetailStarted.complete(Unit)
                            releaseFirstDetail.await()
                            val body =
                                if (firstStatus == 200) {
                                    detail(FIRST_ID, "old selection")
                                } else {
                                    """{"error":{"code":"internal.error","message":"late failure","request_id":"fixture"}}"""
                                }
                            respond(exchange, body, firstStatus)
                        }
                        path.endsWith("/media-items/$SECOND_ID") -> respond(exchange, detail(SECOND_ID, "new selection"))
                        path.endsWith("/processing-tasks") -> respond(exchange, fixture("processing-task-page"))
                        path.endsWith("/review-cases") -> respond(exchange, fixture("review-case-page"))
                        path.endsWith("/integrations/jellyfin") -> {
                            playbackRequests.incrementAndGet()
                            respond(exchange, fixture("jellyfin-integration"))
                        }
                        else -> respond(exchange, "{}", 404)
                    }
                } finally {
                    exchange.close()
                }
            }
            http.start()
        }

        private fun fixture(name: String) = Files.readString(Path.of("../../contracts/examples/v1/$name.json"))

        private fun detail(
            id: String,
            title: String,
        ): String {
            val root = Json.parseToJsonElement(fixture("media-item-detail")).jsonObject
            val item = root.getValue("item").jsonObject
            val selected = JsonObject(item + mapOf("id" to JsonPrimitive(id), "title" to JsonPrimitive(title)))
            return JsonObject(root + ("item" to selected)).toString()
        }

        private fun respond(
            exchange: HttpExchange,
            body: String,
            code: Int = 200,
        ) {
            val bytes = body.toByteArray()
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(code, bytes.size.toLong())
            exchange.responseBody.use { it.write(bytes) }
        }

        override fun close() {
            releaseFirstDetail.countDown()
            stopped.countDown()
            http.stop(0)
            executor.shutdownNow()
        }
    }

    private companion object {
        const val DEVICE_ID = "late-response-device"
        const val FIRST_ID = "019f0000-0000-7000-8000-000000000060"
        const val SECOND_ID = "019f0000-0000-7000-8000-000000000050"
    }
}
