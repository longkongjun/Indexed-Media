package mediaflow.desktop

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import java.net.InetSocketAddress
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class DesktopSessionRaceTest {
    @Test fun blockedLogoutRejectsNewLoginUntilOldSessionIsReleased() =
        runBlocking {
            RaceServer(blockLogout = true).use { old ->
                RaceServer().use { next ->
                    val directory = Files.createTempDirectory("desktop-logout-race")
                    Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                        val stores = mutableMapOf<String, DeviceAuthStore>()
                        stores[old.url] = credentials("old-device")
                        stores[next.url] = credentials("new-device")
                        val model = DesktopModel(directory, ui, { stores.getValue(it) })
                        try {
                            withContext(ui) { model.login(old.url, true, "", "", "", restore = true) }
                            awaitState(model) { it.loggedIn && it.tasks.isNotEmpty() }
                            withContext(ui) { model.logout() }
                            withTimeout(10_000) { old.logoutStarted.await() }
                            assertTrue(model.state.value.busy)
                            withContext(ui) { model.login(next.url, true, "", "", "", restore = true) }
                            assertFalse(model.state.value.loggedIn)
                            assertEquals(0, next.mediaRequests.get())
                            old.releaseLogout.countDown()
                            awaitState(model) { !it.loggedIn && !it.busy }
                            withContext(ui) { model.login(next.url, true, "", "", "", restore = true) }
                            awaitState(model) { it.loggedIn && it.tasks.isNotEmpty() }
                            assertEquals(next.url, model.rememberedServer())
                            withTimeout(10_000) { next.eventsStarted.await() }
                            assertEquals(1, next.eventRequests.get())
                        } finally {
                            old.releaseLogout.countDown()
                            model.close()
                            directory.toFile().deleteRecursively()
                        }
                    }
                }
            }
        }

    @Test fun closingDuringCredentialRestoreCannotPublishOldSession() =
        runBlocking {
            RaceServer().use { server ->
                val entered = CompletableDeferred<Unit>()
                val release = CompletableDeferred<Unit>()
                val returned = CompletableDeferred<Unit>()
                val store =
                    object : DeviceAuthStore {
                        override suspend fun read(): StoredCredentials =
                            withContext(NonCancellable) {
                                entered.complete(Unit)
                                release.await()
                                returned.complete(Unit)
                                StoredCredentials.DeviceToken("synthetic", "old-device")
                            }

                        override suspend fun save(credentials: StoredCredentials) = Unit

                        override suspend fun clear() = Unit
                    }
                val directory = Files.createTempDirectory("desktop-restore-race")
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                    val model = DesktopModel(directory, ui, { store })
                    try {
                        withContext(ui) { model.login(server.url, true, "", "", "", restore = true) }
                        withTimeout(10_000) { entered.await() }
                        model.close()
                        release.complete(Unit)
                        withTimeout(10_000) { returned.await() }
                        withContext(ui) { assertFalse(model.state.value.loggedIn) }
                        assertEquals(0, server.mediaRequests.get())
                        assertFalse(Files.exists(directory.resolve("server.txt")))
                    } finally {
                        release.complete(Unit)
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    @Test fun unknownTaskCancellationKeepsOperationKeyAndBlocksNewCancellation() =
        runBlocking {
            RaceServer().use { server ->
                val directory = Files.createTempDirectory("desktop-cancel-race")
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                    val store = credentials("fixture-device")
                    val model = DesktopModel(directory, ui, { store })
                    try {
                        withContext(ui) { model.login(server.url, true, "", "", "", restore = true) }
                        val task = awaitState(model) { it.tasks.isNotEmpty() }.tasks.first()
                        withContext(ui) { model.cancelTask(task) }
                        awaitState(model) { it.writeUnknown && !it.writeBusy }
                        withContext(ui) { model.cancelTask(task) }
                        assertEquals(1, server.cancelKeys.size)
                        withContext(ui) { model.retryTaskCancel() }
                        withTimeout(10_000) { server.secondCancel.await() }
                        awaitState(model) { it.writeUnknown && !it.writeBusy }
                        assertEquals(2, server.cancelKeys.size)
                        assertEquals(server.cancelKeys[0], server.cancelKeys[1])
                    } finally {
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    private suspend fun credentials(id: String): DeviceAuthStore =
        InMemoryDeviceAuthStore().apply {
            save(StoredCredentials.DeviceToken("synthetic", id))
        }

    private suspend fun awaitState(
        model: DesktopModel,
        predicate: (DesktopUi) -> Boolean,
    ): DesktopUi = withTimeout(10_000) { model.state.first(predicate) }

    private class RaceServer(
        private val blockLogout: Boolean = false,
    ) : AutoCloseable {
        private val stopped = CountDownLatch(1)
        val releaseLogout = CountDownLatch(1)
        val logoutStarted = CompletableDeferred<Unit>()
        val eventsStarted = CompletableDeferred<Unit>()
        val secondCancel = CompletableDeferred<Unit>()
        val mediaRequests = AtomicInteger()
        val eventRequests = AtomicInteger()
        val cancelKeys = java.util.Collections.synchronizedList(mutableListOf<String>())
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
                            eventRequests.incrementAndGet()
                            exchange.responseHeaders.add("Content-Type", "text/event-stream")
                            exchange.sendResponseHeaders(200, 0)
                            exchange.responseBody.write(": connected\n\n".toByteArray())
                            exchange.responseBody.flush()
                            eventsStarted.complete(Unit)
                            stopped.await()
                        }
                        exchange.requestMethod == "DELETE" && path.contains("/devices/") -> {
                            logoutStarted.complete(Unit)
                            if (blockLogout) releaseLogout.await()
                            respond(exchange, "", 204)
                        }
                        path.endsWith("/cancel") -> {
                            cancelKeys += exchange.requestHeaders.getFirst("Idempotency-Key")
                            respond(
                                exchange,
                                """{"error":{"code":"internal.error","message":"fixture failure","request_id":"fixture"}}""",
                                503,
                            )
                            if (cancelKeys.size == 2) secondCancel.complete(Unit)
                        }
                        path.endsWith("/media-items") -> {
                            mediaRequests.incrementAndGet()
                            respond(exchange, fixture("media-item-page"))
                        }
                        path.endsWith(
                            "/processing-tasks",
                        ) -> respond(exchange, fixture("processing-task-page"))
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
            code: Int = 200,
        ) {
            exchange.responseHeaders.add("Content-Type", "application/json")
            val bytes = body.toByteArray()
            exchange.sendResponseHeaders(code, if (code == 204) -1 else bytes.size.toLong())
            exchange.responseBody.use { if (code != 204) it.write(bytes) }
        }

        override fun close() {
            releaseLogout.countDown()
            stopped.countDown()
            http.stop(0)
            executor.shutdownNow()
        }
    }
}
