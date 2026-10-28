package mediaflow.desktop

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import java.net.InetSocketAddress
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import kotlin.test.Test
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DesktopArtworkSessionTest {
    @Test
    fun oldInflightPosterCannotEnterNextSession() =
        runBlocking<Unit> {
            ArtworkServer(blockPoster = true).use { old ->
                ArtworkServer().use { next ->
                    val directory = Files.createTempDirectory("desktop-artwork-session")
                    Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                        val stores = mapOf(old.url to credentials(), next.url to credentials())
                        val model = DesktopModel(directory, ui, { stores.getValue(it) })
                        try {
                            withContext(ui) { model.login(old.url, true, "", "", "", restore = true) }
                            awaitState(model) { it.loggedIn && it.tasks.isNotEmpty() }
                            val oldImage = async { model.artwork(MEDIA_ID) }
                            withTimeout(10_000) { old.posterStarted.await() }
                            withContext(ui) { model.logout() }
                            awaitState(model) { !it.loggedIn && !it.busy }
                            withContext(ui) { model.login(next.url, true, "", "", "", restore = true) }
                            awaitState(model) { it.loggedIn && it.tasks.isNotEmpty() }
                            old.releasePoster.countDown()
                            assertNull(withTimeout(10_000) { oldImage.await() })
                            assertNotNull(model.artwork(MEDIA_ID))
                            assertTrue(model.state.value.loggedIn)
                            assertNotNull(stores.getValue(next.url).read())
                        } finally {
                            old.releasePoster.countDown()
                            model.close()
                            directory.toFile().deleteRecursively()
                        }
                    }
                }
            }
        }

    @Test
    fun unauthorizedPosterClearsCurrentIdentityAndCredentials() =
        runBlocking<Unit> {
            ArtworkServer(status = 401).use { server ->
                val directory = Files.createTempDirectory("desktop-artwork-unauthorized")
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                    val store = credentials()
                    val model = DesktopModel(directory, ui, { store })
                    try {
                        withContext(ui) { model.login(server.url, true, "", "", "", restore = true) }
                        awaitState(model) { it.loggedIn && it.tasks.isNotEmpty() }
                        assertNull(model.artwork(MEDIA_ID))
                        awaitState(model) { !it.loggedIn && !it.busy }
                        assertNull(store.read())
                        assertTrue(
                            model.state.value.wall
                                .isEmpty(),
                        )
                        assertNull(model.artwork(MEDIA_ID))
                    } finally {
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    private suspend fun credentials() =
        InMemoryDeviceAuthStore().also {
            it.save(StoredCredentials.DeviceToken("synthetic-artwork-token", "019f8a50-7271-7000-8000-000000000011"))
        }

    private suspend fun awaitState(
        model: DesktopModel,
        predicate: (DesktopUi) -> Boolean,
    ) = withTimeout(10_000) { model.state.first(predicate) }

    private class ArtworkServer(
        private val blockPoster: Boolean = false,
        private val status: Int = 200,
    ) : AutoCloseable {
        val posterStarted = CompletableDeferred<Unit>()
        val releasePoster = CountDownLatch(1)
        private val stopped = CountDownLatch(1)
        private val executor = Executors.newCachedThreadPool()
        private val http = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
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
                        path.endsWith("/artwork") -> {
                            posterStarted.complete(Unit)
                            if (blockPoster) releasePoster.await()
                            if (status == 200) {
                                exchange.responseHeaders.add("Content-Type", "image/png")
                                val bytes = byteArrayOf(1, 2, 3)
                                exchange.sendResponseHeaders(200, bytes.size.toLong())
                                exchange.responseBody.write(bytes)
                            } else {
                                respond(exchange, "{}", status)
                            }
                        }
                        exchange.requestMethod == "DELETE" -> exchange.sendResponseHeaders(204, -1)
                        path.endsWith("/media-items") -> respond(exchange, fixture("media-item-page"))
                        path.endsWith("/processing-tasks") -> respond(exchange, fixture("processing-task-page"))
                        path.endsWith("/review-cases") -> respond(exchange, fixture("review-case-page"))
                        else -> respond(exchange, "{}", 404)
                    }
                } finally {
                    exchange.close()
                }
            }
            http.start()
        }

        private fun fixture(name: String) = Files.readString(Path.of("../../contracts/examples/v1/$name.json"))

        private fun respond(
            exchange: HttpExchange,
            body: String,
            status: Int = 200,
        ) {
            val bytes = body.toByteArray()
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(status, bytes.size.toLong())
            exchange.responseBody.write(bytes)
        }

        override fun close() {
            releasePoster.countDown()
            stopped.countDown()
            http.stop(0)
            executor.shutdownNow()
        }
    }

    private companion object {
        const val MEDIA_ID = "019f8a50-7271-7000-8000-000000000001"
    }
}
