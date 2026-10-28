package mediaflow.desktop

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import mediaflow.client.cache.MediaFlowCache
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
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DesktopModelTest {
    @Test fun sessionRestoreLoadsFixturesAndClosesSingleEventConnection() =
        runBlocking {
            FixtureServer().use { server ->
                val directory = Files.createTempDirectory("mediaflow-desktop-test")
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                    val store = InMemoryDeviceAuthStore().apply { save(StoredCredentials.DeviceToken("synthetic", "fixture-device")) }
                    val model = DesktopModel(directory, ui, { store })
                    try {
                        model.login(server.url, true, "", "", "", restore = true)
                        val loaded =
                            withTimeout(10_000) { model.state.first { it.loggedIn && it.wall.isNotEmpty() && it.cases.isNotEmpty() } }
                        assertTrue(loaded.loggedIn)
                        withTimeout(10_000) { server.eventsStarted.await() }
                        model.refresh()
                        withTimeout(10_000) { server.secondWall.await() }
                        assertEquals(1, server.eventRequests.get())
                        model.logout(expired = true)
                        withTimeout(10_000) { model.state.first { !it.loggedIn } }
                        assertFalse(model.state.value.loggedIn)
                    } finally {
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    @Test fun unknownReviewWriteKeepsCaseAndIdempotencyKeyUntilRetry() =
        runBlocking {
            FixtureServer().use { server ->
                val directory = Files.createTempDirectory("mediaflow-desktop-review")
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                    val store = InMemoryDeviceAuthStore().apply { save(StoredCredentials.DeviceToken("synthetic", "fixture-device")) }
                    val model = DesktopModel(directory, ui, { store })
                    try {
                        model.login(server.url, true, "", "", "", restore = true)
                        val loaded = withTimeout(10_000) { model.state.first { it.cases.isNotEmpty() } }
                        val id = loaded.cases.first().id
                        model.selectCase(id)
                        withTimeout(10_000) { model.state.first { it.selectedCase?.id == id } }
                        model.chooseGeneric()
                        withTimeout(10_000) { model.state.first { it.writeUnknown } }
                        model.clearCase()
                        model.selectCase("another-case")
                        model.retryDecision()
                        withTimeout(10_000) { server.secondDecision.await() }
                        assertEquals(
                            id,
                            model.state.value.selectedCase
                                ?.id,
                        )
                        assertTrue(model.state.value.writeUnknown)
                        assertEquals(2, server.keys.size)
                        assertEquals(server.keys[0], server.keys[1])
                    } finally {
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    @Test fun cacheReadFailureCannotLeaveAPublishedClosedSession() =
        runBlocking {
            FixtureServer().use { server ->
                val directory = Files.createTempDirectory("desktop-cache-failure")
                val cacheDirectory = Files.createDirectories(directory.resolve("cache"))
                val file = cacheDirectory.resolve("${cacheIdentity(server.url, "fixture-device")}.db")
                val driver = JdbcSqliteDriver("jdbc:sqlite:$file")
                try {
                    MediaFlowCache.Schema.create(driver)
                    driver.execute(
                        null,
                        "INSERT INTO media_item_cache VALUES ('corrupt', 'movie', 'library', 'Cached', NULL, 'available', NULL, 'invalid', 0)",
                        0,
                    )
                } finally {
                    driver.close()
                }
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                    val store = InMemoryDeviceAuthStore().apply { save(StoredCredentials.DeviceToken("synthetic", "fixture-device")) }
                    lateinit var candidate: DesktopSession
                    val model =
                        DesktopModel(directory, ui, { store }) { address, authStore ->
                            DesktopSession(address, authStore).also { candidate = it }
                        }
                    try {
                        withContext(ui) { model.login(server.url, true, "", "", "", restore = true) }
                        withTimeout(10_000) { model.state.first { !it.busy && it.message != null } }
                        assertFalse(model.state.value.loggedIn, "failed cache attachment must roll back the published session")
                        assertTrue(
                            model.state.value.wall
                                .isEmpty(),
                        )
                        assertNull(candidate.authCache.current())
                        assertTrue(store.read() is StoredCredentials.DeviceToken)
                    } finally {
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    @Test fun acceptedReviewCannotRecoverAnUnrelatedUnknownCancellation() = unrelatedRecovery(202)

    @Test fun rejectedReviewCannotRecoverAnUnrelatedUnknownCancellation() = unrelatedRecovery(403)

    private fun unrelatedRecovery(decisionStatus: Int) =
        runBlocking {
            FixtureServer(decisionStatus).use { server ->
                val directory = Files.createTempDirectory("desktop-write-recovery")
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { ui ->
                    val store = InMemoryDeviceAuthStore().apply { save(StoredCredentials.DeviceToken("synthetic", "fixture-device")) }
                    val model = DesktopModel(directory, ui, { store })
                    try {
                        withContext(ui) { model.login(server.url, true, "", "", "", restore = true) }
                        val loaded = withTimeout(10_000) { model.state.first { it.cases.isNotEmpty() && it.tasks.isNotEmpty() } }
                        withContext(ui) { model.selectCase(loaded.cases.first().id) }
                        withTimeout(10_000) { model.state.first { it.selectedCase != null } }
                        withContext(ui) { model.chooseGeneric() }
                        withTimeout(10_000) {
                            model.state.first { !it.writeBusy && (if (decisionStatus == 202) it.receipt != null else it.message != null) }
                        }
                        withContext(ui) { model.cancelTask(loaded.tasks.first()) }
                        withTimeout(10_000) { model.state.first { it.writeUnknown && !it.writeBusy } }
                        withContext(ui) { model.retryDecision() }
                        assertFalse(model.state.value.writeBusy, "review retry must not start while task cancellation is unknown")
                        assertTrue(model.state.value.writeUnknown, "an old review receipt cannot resolve a different operation")
                        assertEquals(1, server.keys.size)
                        withContext(ui) { model.retryTaskCancel() }
                        withTimeout(10_000) { server.secondCancel.await() }
                        withTimeout(10_000) { model.state.first { it.writeUnknown && !it.writeBusy } }
                        assertEquals(2, server.cancelKeys.size)
                        assertEquals(server.cancelKeys[0], server.cancelKeys[1])
                    } finally {
                        model.close()
                        directory.toFile().deleteRecursively()
                    }
                }
            }
        }

    private class FixtureServer(
        private val decisionStatus: Int = 503,
    ) : AutoCloseable {
        private val stopped = CountDownLatch(1)
        private val executor = Executors.newCachedThreadPool()
        private val http = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        val eventRequests = AtomicInteger()
        private val wallRequests = AtomicInteger()
        val eventsStarted = CompletableDeferred<Unit>()
        val secondWall = CompletableDeferred<Unit>()
        val secondDecision = CompletableDeferred<Unit>()
        val secondCancel = CompletableDeferred<Unit>()
        val cancelKeys = java.util.Collections.synchronizedList(mutableListOf<String>())
        val keys = java.util.Collections.synchronizedList(mutableListOf<String>())
        val url get() = "http://127.0.0.1:${http.address.port}"

        init {
            http.executor = executor
            http.createContext("/api/v1") { exchange ->
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
                        exchange.close()
                    }
                    path.endsWith("/decisions") -> {
                        keys += exchange.requestHeaders.getFirst("Idempotency-Key")
                        respond(
                            exchange,
                            if (decisionStatus == 202) {
                                fixture("task-decision-receipt")
                            } else {
                                """{"error":{"code":"internal.error","message":"fixture failure","request_id":"fixture"}}"""
                            },
                            decisionStatus,
                        )
                        if (keys.size == 2) secondDecision.complete(Unit)
                    }
                    path.endsWith("/cancel") -> {
                        cancelKeys += exchange.requestHeaders.getFirst("Idempotency-Key")
                        respond(exchange, """{"error":{"code":"internal.error","message":"fixture failure","request_id":"fixture"}}""", 503)
                        if (cancelKeys.size == 2) secondCancel.complete(Unit)
                    }
                    path.endsWith("/media-items") -> {
                        respond(exchange, fixture("media-item-page"))
                        if (wallRequests.incrementAndGet() >= 2) secondWall.complete(Unit)
                    }
                    path.endsWith("/processing-tasks") -> respond(exchange, fixture("processing-task-page"))
                    path.endsWith("/review-cases") -> respond(exchange, fixture("review-case-page"))
                    path.contains("/review-cases/") -> respond(exchange, fixture("review-case"))
                    path.contains("/devices/") -> respond(exchange, "", 204)
                    else -> respond(exchange, "{}", 404)
                }
            }
            http.start()
        }

        private fun fixture(name: String) = Files.readString(Path.of("../../contracts/examples/v1/$name.json"))

        private fun respond(
            exchange: HttpExchange,
            json: String,
            code: Int = 200,
        ) {
            exchange.responseHeaders.add("Content-Type", "application/json")
            val bytes = json.toByteArray()
            exchange.sendResponseHeaders(code, if (code == 204) -1 else bytes.size.toLong())
            exchange.responseBody.use { if (code != 204) it.write(bytes) }
        }

        override fun close() {
            stopped.countDown()
            http.stop(0)
            executor.shutdownNow()
        }
    }
}
