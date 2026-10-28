package mediaflow.desktop

import com.sun.net.httpserver.HttpExchange
import com.sun.net.httpserver.HttpServer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Job
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.tasks.TaskFilter
import java.net.InetSocketAddress
import java.net.URLDecoder
import java.nio.file.Files
import java.nio.file.Path
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicReference
import kotlin.coroutines.CoroutineContext
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DesktopDiscoveryTest {
    @Test
    fun mediaFiltersOwnTheirPagesAndCannotOverwriteTheDefaultCache() =
        runBlocking {
            DesktopDiscoveryServer(delayOldWall = true).use { server ->
                withModel(server) { model, ui, session ->
                    val old = withContext(ui) { ui.captureNext().also { model.applyMediaFilter(MediaFilter(query = "old")) } }
                    withTimeout(10_000) { server.oldWallStarted.await() }
                    withContext(
                        ui,
                    ) { model.applyMediaFilter(MediaFilter(type = "movie", libraryId = LIBRARY, localStatus = "partial", query = "new")) }
                    withTimeout(10_000) { model.state.first { it.wall.firstOrNull()?.title == "New result" } }
                    server.releaseOldWall.countDown()
                    withTimeout(10_000) { old.await().join() }
                    assertEquals("new", model.state.value.mediaFilter.query)
                    assertEquals(
                        listOf("New result"),
                        model.state.value.wall
                            .map { it.title },
                    )
                    withContext(ui) { model.reloadWall(next = true) }
                    withTimeout(10_000) { model.state.first { it.wall.size == 2 } }
                    assertEquals(
                        listOf("New result", "New second"),
                        model.state.value.wall
                            .map { it.title },
                    )
                    assertFalse(model.state.value.wallHasMore)
                    withContext(ui) { model.refresh() }
                    withTimeout(10_000) { model.state.first { it.wall.size == 1 && it.wall.first().title == "New result" } }
                    val filtered = synchronized(server.mediaQueries) { server.mediaQueries.filter { it["q"] == "new" } }
                    assertTrue(filtered.size >= 3)
                    assertTrue(filtered.all { it["type"] == "movie" && it["library_id"] == LIBRARY && it["local_status"] == "partial" })
                    withContext(session.io) { assertEquals(listOf("Default result"), session.cache.cachedWall().map { it.title }) }
                    withContext(ui) { model.applyMediaFilter(MediaFilter(query = "missing")) }
                    withTimeout(10_000) { model.state.first { it.wallState == mediaflow.client.core.paging.PageState.Empty } }
                    assertTrue(
                        model.state.value.wall
                            .isEmpty(),
                    )
                    withContext(session.io) { assertEquals(listOf("Default result"), session.cache.cachedWall().map { it.title }) }
                    withContext(ui) { model.applyMediaFilter(MediaFilter()) }
                    withTimeout(10_000) { model.state.first { it.wall.firstOrNull()?.title == "Default result" } }
                    assertEquals(MediaFilter(), model.state.value.mediaFilter)
                    assertNull(model.state.value.message)
                }
            }
        }

    @Test
    fun taskFiltersPersistAcrossNextPageAndRefresh() =
        runBlocking {
            DesktopDiscoveryServer().use { server ->
                withModel(server) { model, ui, _ ->
                    val filter = TaskFilter("all", "planning", "failed", "沙丘")
                    withContext(ui) { model.applyTaskFilter(filter) }
                    withTimeout(10_000) {
                        model.state.first {
                            it.taskFilter == filter &&
                                it.tasks.firstOrNull()?.id == TASK_A &&
                                it.taskHasMore
                        }
                    }
                    withContext(ui) { model.reloadTasks(next = true) }
                    withTimeout(10_000) { model.state.first { it.tasks.size == 2 } }
                    assertFalse(model.state.value.taskHasMore)
                    withContext(ui) { model.selectTask(TASK_A) }
                    withTimeout(10_000) { model.state.first { it.selectedTask?.id == TASK_A && !it.taskDetailLoading } }
                    assertEquals(
                        2L,
                        model.state.value.selectedTask
                            ?.attemptCount,
                    )
                    val requestsBeforeRefresh = server.taskDetailRequests.size
                    withContext(ui) { model.refresh() }
                    withTimeout(10_000) { model.state.first { it.tasks.size == 1 && !it.taskDetailLoading } }
                    assertTrue(server.taskDetailRequests.size > requestsBeforeRefresh)
                    val filtered = synchronized(server.taskQueries) { server.taskQueries.filter { it["q"] == "沙丘" } }
                    assertTrue(filtered.size >= 3)
                    assertTrue(filtered.all { it["view"] == "all" && it["stage"] == "planning" && it["status"] == "failed" })
                    assertEquals(filter, model.state.value.taskFilter)
                }
            }
        }

    @Test
    fun lateTaskDetailCannotReplaceTheCurrentTask() =
        runBlocking {
            DesktopDiscoveryServer(delayFirstTask = true).use { server ->
                withModel(server) { model, ui, _ ->
                    val first = withContext(ui) { ui.captureNext().also { model.selectTask(TASK_A) } }
                    withTimeout(10_000) { server.firstTaskStarted.await() }
                    withContext(ui) { model.selectTask(TASK_B) }
                    withTimeout(10_000) { model.state.first { it.selectedTask?.id == TASK_B && !it.taskDetailLoading } }
                    server.releaseFirstTask.countDown()
                    withTimeout(10_000) { first.await().join() }
                    assertEquals(
                        TASK_B,
                        model.state.value.selectedTask
                            ?.id,
                    )
                    assertFalse(model.state.value.taskDetailLoading)
                    assertNull(model.state.value.message)
                }
            }
        }

    @Test
    fun taskEventsRefreshTheOpenDetailWithoutClearingFilters() =
        runBlocking {
            DesktopDiscoveryServer().use { server ->
                withModel(server) { model, ui, _ ->
                    val filter = TaskFilter(view = "all", status = "failed")
                    withContext(ui) { model.applyTaskFilter(filter) }
                    withTimeout(10_000) { model.state.first { it.taskFilter == filter && it.tasks.isNotEmpty() } }
                    withContext(ui) { model.selectTask(TASK_A) }
                    withTimeout(10_000) { model.state.first { it.selectedTask?.attemptCount == 2L } }
                    withTimeout(10_000) { server.eventsStarted.await() }
                    server.publishTaskState()
                    withTimeout(10_000) { model.state.first { it.selectedTask?.attemptCount == 3L && it.tasks.isEmpty() } }
                    assertEquals(filter, model.state.value.taskFilter)
                    assertEquals(
                        TASK_A,
                        model.state.value.selectedTask
                            ?.id,
                    )
                }
            }
        }

    @Test
    fun unknownTaskRetryReusesItsKeyAndCannotReplayCancellation() =
        runBlocking {
            DesktopDiscoveryServer(unknownFirstRetry = true).use { server ->
                withModel(server) { model, ui, _ ->
                    val task =
                        model.state.value.tasks
                            .first()
                    withContext(ui) { model.selectTask(task.id) }
                    withTimeout(10_000) { model.state.first { it.selectedTask?.id == task.id } }
                    withContext(ui) { model.retryTask(task) }
                    withTimeout(10_000) { model.state.first { it.writeUnknown && !it.writeBusy } }
                    assertEquals(DesktopWriteOperation.TaskRetry, model.state.value.writeOperation)
                    withContext(ui) {
                        model.retryTaskCancel()
                        model.retryDecision()
                        model.applyTaskFilter(TaskFilter(view = "completed"))
                    }
                    assertTrue(server.cancelKeys.isEmpty())
                    assertEquals(1, server.retryKeys.size)
                    assertEquals("pending", model.state.value.taskFilter.view)
                    withContext(ui) { model.retryTaskRetry() }
                    withTimeout(10_000) { model.state.first { !it.writeUnknown && !it.writeBusy && it.selectedTask?.attemptCount == 3L } }
                    assertEquals(2, server.retryKeys.size)
                    assertEquals(server.retryKeys[0], server.retryKeys[1])
                    assertNull(model.state.value.writeOperation)
                }
            }
        }

    @Test
    fun unknownRetryStartedFromTheListCanStillInspectItsOwnTask() =
        runBlocking {
            DesktopDiscoveryServer(unknownFirstRetry = true).use { server ->
                withModel(server) { model, ui, _ ->
                    val task =
                        model.state.value.tasks
                            .first()
                    withContext(ui) { model.retryTask(task) }
                    withTimeout(10_000) { model.state.first { it.writeUnknown && !it.writeBusy } }
                    val selection = withContext(ui) { ui.captureNext().also { model.selectTask(task.id) } }
                    // Selecting the operation's own object is a read and must remain available for reconciliation.
                    assertTrue(selection.isCompleted, "unknown write must allow a read of its own task")
                    withTimeout(10_000) { selection.await().join() }
                    withContext(ui) { model.selectTask(TASK_B) }
                    assertEquals(
                        task.id,
                        model.state.value.selectedTask
                            ?.id,
                    )
                    assertTrue(model.state.value.writeUnknown)
                    assertEquals(1, server.retryKeys.size)
                }
            }
        }

    private suspend fun withModel(
        server: DesktopDiscoveryServer,
        block: suspend (DesktopModel, CapturingUi, DesktopSession) -> Unit,
    ) {
        val directory = Files.createTempDirectory("desktop-discovery")
        val created = AtomicReference<DesktopSession>()
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { executor ->
            val ui = CapturingUi(executor)
            val store = InMemoryDeviceAuthStore().apply { save(StoredCredentials.DeviceToken("synthetic-discovery", "discovery-device")) }
            val model =
                DesktopModel(directory, ui, { store }) { address, authStore -> DesktopSession(address, authStore).also(created::set) }
            try {
                withContext(ui) { model.login(server.url, true, "", "", "", restore = true) }
                withTimeout(10_000) { model.state.first { it.loggedIn && it.wall.isNotEmpty() && it.tasks.isNotEmpty() } }
                block(model, ui, assertNotNull(created.get()))
            } finally {
                server.releaseOldWall.countDown()
                server.releaseFirstTask.countDown()
                model.close()
                directory.toFile().deleteRecursively()
            }
        }
    }

    private class CapturingUi(
        private val delegate: CoroutineDispatcher,
    ) : CoroutineDispatcher() {
        private val next = AtomicReference<Pair<Thread, CompletableDeferred<Job>>?>()

        fun captureNext(): CompletableDeferred<Job> = CompletableDeferred<Job>().also { next.set(Thread.currentThread() to it) }

        override fun dispatch(
            context: CoroutineContext,
            block: Runnable,
        ) {
            next.get()?.let { capture ->
                if (capture.first === Thread.currentThread() &&
                    next.compareAndSet(capture, null)
                ) {
                    capture.second.complete(checkNotNull(context[Job]))
                }
            }
            delegate.dispatch(context, block)
        }
    }
}

internal class DesktopDiscoveryServer(
    private val delayOldWall: Boolean = false,
    private val delayFirstTask: Boolean = false,
    private val unknownFirstRetry: Boolean = false,
) : AutoCloseable {
    private val stopped = CountDownLatch(1)
    private val executor = Executors.newCachedThreadPool()
    private val http = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)

    @Volatile
    private var retried = false
    val mediaQueries = Collections.synchronizedList(mutableListOf<Map<String, String>>())
    val taskQueries = Collections.synchronizedList(mutableListOf<Map<String, String>>())
    val taskDetailRequests = Collections.synchronizedList(mutableListOf<String>())
    val retryKeys = Collections.synchronizedList(mutableListOf<String>())
    val cancelKeys = Collections.synchronizedList(mutableListOf<String>())
    val oldWallStarted = CompletableDeferred<Unit>()
    val firstTaskStarted = CompletableDeferred<Unit>()
    val eventsStarted = CompletableDeferred<Unit>()

    @Volatile
    private var eventExchange: HttpExchange? = null
    val releaseOldWall = CountDownLatch(1)
    val releaseFirstTask = CountDownLatch(1)
    val url get() = "http://127.0.0.1:${http.address.port}"

    init {
        http.executor = executor
        http.createContext("/api/v1") { exchange ->
            try {
                val path = exchange.requestURI.path
                val query = query(exchange)
                when {
                    path.endsWith("/events") -> {
                        exchange.responseHeaders.add("Content-Type", "text/event-stream")
                        exchange.sendResponseHeaders(200, 0)
                        exchange.responseBody.write(": connected\n\n".toByteArray())
                        exchange.responseBody.flush()
                        eventExchange = exchange
                        eventsStarted.complete(Unit)
                        stopped.await()
                    }
                    path.endsWith("/media-libraries") -> respond(exchange, """{"items":[{"id":"$LIBRARY","name":"电影库"}]}""")
                    path.endsWith("/media-items") -> {
                        mediaQueries += query
                        if (delayOldWall && query["q"] == "old") {
                            oldWallStarted.complete(Unit)
                            releaseOldWall.await()
                        }
                        val second = query["cursor"] != null
                        val title =
                            when (query["q"]) {
                                "new" -> if (second) "New second" else "New result"
                                "old" -> "Old result"
                                else -> "Default result"
                            }
                        val items = if (query["q"] == "missing") "" else media(if (second) MEDIA_B else MEDIA_A, title)
                        respond(
                            exchange,
                            """{"items":[$items],"next_cursor":${if (query["q"] == "new" && !second) "\"more\"" else "null"}}""",
                        )
                    }
                    path.endsWith("/processing-tasks") -> {
                        taskQueries += query
                        val second = query["cursor"] != null
                        val body =
                            if (retried && query["status"] == "failed") {
                                """{"items":[],"next_cursor":null,"summary":{"pending":0,"running":0,"all":2,"completed":0,"snapshot_version":2}}"""
                            } else {
                                taskPage(if (second) TASK_B else TASK_A, if (query["q"] != null && !second) "more" else null)
                            }
                        respond(exchange, body)
                    }
                    path.endsWith("/attempts") -> {
                        retryKeys += exchange.requestHeaders.getFirst("Idempotency-Key")
                        if (unknownFirstRetry && retryKeys.size == 1) {
                            respond(exchange, error(), 503)
                        } else {
                            retried = true
                            respond(exchange, task(TASK_A), 202)
                        }
                    }
                    path.endsWith("/cancel") -> {
                        cancelKeys += exchange.requestHeaders.getFirst("Idempotency-Key")
                        respond(exchange, task(TASK_A), 202)
                    }
                    path.contains("/processing-tasks/") -> {
                        val id = path.substringAfterLast('/')
                        taskDetailRequests += id
                        if (delayFirstTask && id == TASK_A) {
                            firstTaskStarted.complete(Unit)
                            releaseFirstTask.await()
                        }
                        respond(exchange, task(id))
                    }
                    path.endsWith("/review-cases") -> respond(exchange, fixture("review-case-page"))
                    path.endsWith("/integrations/jellyfin") -> respond(exchange, fixture("jellyfin-integration"))
                    else -> respond(exchange, error(), 404)
                }
            } catch (_: java.io.IOException) {
                // Cancelled filter requests may close their transport before this controlled reply.
            } finally {
                exchange.close()
            }
        }
        http.start()
    }

    private fun query(exchange: HttpExchange): Map<String, String> =
        exchange.requestURI.rawQuery
            ?.split('&')
            ?.associate { part ->
                val pair = part.split('=', limit = 2)
                URLDecoder.decode(pair[0], "UTF-8") to URLDecoder.decode(pair.getOrElse(1) { "" }, "UTF-8")
            }.orEmpty()

    private fun fixture(name: String) = Files.readString(Path.of("../../contracts/examples/v1/$name.json"))

    fun publishTaskState() {
        retried = true
        val event = checkNotNull(eventExchange)
        val frame = """id: 1
data: {"id":1,"type":"processing-task.state-changed","occurred_at":"2026-10-28T08:00:00Z","task_id":"$TASK_A","payload":{}}

"""
        event.responseBody.write(frame.toByteArray())
        event.responseBody.flush()
    }

    private fun media(
        id: String,
        title: String,
    ) =
        """{"id":"$id","type":"movie","library_id":"$LIBRARY","title":"$title","year":2021,"local_status":"partial","artwork_ref":null,"updated_at":"2026-10-28T08:00:00Z"}"""

    private fun taskPage(
        id: String,
        cursor: String?,
    ) = """{"items":[${task(
        id,
    )}],"next_cursor":${cursor?.let {
        "\"$it\""
    } ?: "null"},"summary":{"pending":1,"running":0,"all":2,"completed":0,"snapshot_version":1}}"""

    private fun task(id: String) =
        """{"id":"$id","inbox_directory_id":"019f0000-0000-7000-8000-000000000002","file_revision_id":"019f0000-0000-7000-8000-000000000044","relative_path":"电影/${if (id == TASK_A) "沙丘" else "另一部电影"}.mkv","status":"${if (retried) "queued" else "failed"}","stage":"planning","checkpoint":"planning-paused","decision_checkpoint":null,"current_task_decision_id":null,"reason":"organization.plan-paused","recovering":false,"attempt_count":${if (retried) 3 else 2},"next_retry_at":null,"allowed_actions":["retry","cancel"],"updated_at":"2026-10-28T08:00:00Z"}"""

    private fun error() = """{"error":{"code":"internal.error","message":"controlled failure","request_id":"discovery-fixture"}}"""

    private fun respond(
        exchange: HttpExchange,
        body: String,
        status: Int = 200,
    ) {
        val bytes = body.toByteArray()
        exchange.responseHeaders.add("Content-Type", "application/json")
        exchange.sendResponseHeaders(status, bytes.size.toLong())
        exchange.responseBody.use { it.write(bytes) }
    }

    override fun close() {
        releaseOldWall.countDown()
        releaseFirstTask.countDown()
        stopped.countDown()
        http.stop(0)
        executor.shutdownNow()
    }
}

internal const val LIBRARY = "019f0000-0000-7000-8000-000000000051"
internal const val MEDIA_A = "019f0000-0000-7000-8000-000000000050"
internal const val MEDIA_B = "019f0000-0000-7000-8000-000000000052"
internal const val TASK_A = "019f0000-0000-7000-8000-000000000043"
internal const val TASK_B = "019f0000-0000-7000-8000-000000000045"
