package mediaflow.desktop

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.joinAll
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import mediaflow.api.models.ProcessingTask
import mediaflow.api.models.SelectGenericVideoCommand
import mediaflow.client.core.SdkResult
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.events.TaskEventEnvelope
import mediaflow.client.core.getOrNull
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.media.MediaRepository
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.paging.PageState
import mediaflow.client.core.review.ReviewRepository
import mediaflow.client.core.tasks.TaskFilter
import mediaflow.client.core.tasks.TaskRepository
import org.junit.Test
import java.net.InetAddress
import java.net.ServerSocket
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.nio.ByteBuffer
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermissions
import java.sql.Connection
import java.sql.DriverManager
import java.time.Duration
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.coroutines.CoroutineContext
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Real serve process and public HTTP; stopped, run-owned DB projections do not test TMDB or catalog publication. */
class DesktopRealCoreTest {
    @Test(timeout = 120_000)
    fun realCorePairingReadsWritesRestoreRevocationAndClose() =
        runBlocking {
            RealCore().use { core ->
                core.start()
                core.bootstrap()
                val tasks = core.discoverFiles()
                val reviewIds = core.seedReadProjections(tasks.take(2))
                Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { executor ->
                    val ui = CapturedUi(executor)
                    val store = InMemoryDeviceAuthStore()
                    var session: DesktopSession? = null

                    fun model() =
                        DesktopModel(core.directory.resolve("desktop"), ui, { store }) { server, authStore ->
                            DesktopSession(server, authStore).also { session = it }
                        }
                    var current = model()
                    try {
                        val pairing = core.request("POST", "/api/v1/device-pairing-codes", status = 201).text("pairing_code")
                        withContext(ui) { current.login(core.origin, true, pairing, "", "") }
                        awaitState(current) { it.loggedIn && it.wall.size == 2 && it.tasks.size == 4 && it.cases.size == 2 }
                        val firstCredential = assertNotNull(store.read() as? StoredCredentials.DeviceToken)

                        val active = assertNotNull(session)
                        val pager = active.media.mediaWall(limit = 1)
                        withContext(active.io) {
                            assertTrue(pager.reload())
                            assertEquals(1, pager.items.value.size)
                            assertTrue(pager.hasMore())
                            assertTrue(pager.loadNext())
                            assertEquals(
                                2,
                                pager.items.value
                                    .map { it.id }
                                    .distinct()
                                    .size,
                            )
                            assertFalse(pager.hasMore())
                        }
                        val mediaId =
                            current.state.value.wall
                                .first()
                                .id
                        withContext(ui) { current.detail(mediaId) }
                        awaitState(current) { it.detail?.item?.id == mediaId && !it.detailLoading }
                        assertTrue(
                            current.state.value.detail
                                ?.versions
                                ?.isEmpty() == true,
                        )

                        verifyMediaFilters(current, ui, active)
                        verifyTaskFiltersAndRetry(current, ui, active, core, tasks[3])
                        verifyReadOnlyPairing(core, mediaId, tasks[3], assertNotNull(firstCredential.deviceId))

                        val decisionEvent =
                            async(start = CoroutineStart.UNDISPATCHED) {
                                withTimeout(30_000) {
                                    active.events().first {
                                        it.type == TaskEventEnvelope.TYPE_TASK_DECISION_ACCEPTED &&
                                            it.taskId == tasks.first()
                                    }
                                }
                            }
                        withContext(ui) { current.selectCase(reviewIds.first()) }
                        awaitState(current) { it.selectedCase?.id == reviewIds.first() }
                        withContext(ui) { current.chooseGeneric() }
                        val submitted = awaitState(current) { it.receipt != null && !it.writeBusy }
                        assertTrue(submitted.receipt?.reviewCaseId == reviewIds.first())
                        assertFalse(submitted.writeUnknown)
                        val event = decisionEvent.await()
                        withContext(active.io) { assertTrue(active.cache.eventCursor() >= event.id) }

                        val second = core.request("GET", "/api/v1/review-cases/${reviewIds.last()}")
                        val command =
                            ReviewRepository.ReviewDecisionKind.SelectGenericVideo(
                                SelectGenericVideoCommand("select-generic-video", "Integration generic", null, false),
                            )
                        val key = "desktop-real-${UUID.randomUUID()}"
                        val receipt = active.review.submitDecision(reviewIds.last(), second.number("version"), key, command)
                        val replay = active.review.submitDecision(reviewIds.last(), second.number("version"), key, command)
                        assertTrue(receipt is SdkResult.Success && replay is SdkResult.Success)
                        assertEquals(receipt.getOrNull()?.id, replay.getOrNull()?.id)
                        core.awaitTask(tasks[1]) { it.text("status") == "paused" && it.text("stage") == "planning" }
                        assertEquals(2L, core.decisionCount())

                        val cancelling =
                            current.state.value.tasks
                                .first { it.id == tasks[2] }
                        withContext(ui) { current.cancelTask(cancelling) }
                        val cancelled = awaitState(current) { !it.writeBusy && it.tasks.none { task -> task.id == tasks[2] } }
                        assertFalse(cancelled.writeUnknown)
                        assertNull(cancelled.message)
                        assertEquals("cancelled", core.request("GET", "/api/v1/processing-tasks/${tasks[2]}").text("status"))

                        val refreshJobs =
                            withContext(ui) {
                                current.clearCase()
                                ui.capture { current.refresh() }
                            }
                        withTimeout(30_000) { refreshJobs.joinAll() }
                        // Drain page observers queued by the completed IO actions before checking their snapshots.
                        withContext(ui) { }
                        val reconciled = current.state.value
                        assertTrue(
                            listOf(reconciled.wallState, reconciled.taskState, reconciled.caseState).all {
                                it == PageState.Content ||
                                    it == PageState.Empty
                            },
                            "real Core page DTOs must finish refresh without Error/Offline",
                        )
                        assertNull(reconciled.message)

                        current.close()
                        assertTrue(core.alive())
                        assertEquals("paused", core.request("GET", "/api/v1/processing-tasks/${tasks[3]}").text("status"))
                        assertTrue(active.authCache.current() == null, "closed session must clear in-memory credentials")
                        current = model()
                        withContext(ui) { current.login(core.origin, true, "", "", "", restore = true) }
                        awaitState(current) { it.loggedIn && it.wall.size == 2 }
                        val restored = assertNotNull(store.read() as? StoredCredentials.DeviceToken)
                        assertTrue(restored.deviceId == firstCredential.deviceId, "cold restore must reuse the enrolled identity")

                        core.request("DELETE", "/api/v1/devices/${restored.deviceId}", status = 204)
                        withContext(ui) { current.refresh() }
                        awaitState(current) { !it.loggedIn && !it.busy }
                        assertTrue(store.read() == null, "revocation/logout must clear credentials")

                        withContext(ui) { current.login(core.origin, false, "", "integration-admin", core.password) }
                        awaitState(current) { it.loggedIn && !it.busy }
                        val loggingOut = assertNotNull(store.read() as? StoredCredentials.DeviceToken)
                        withContext(ui) { current.logout() }
                        awaitState(current) { !it.loggedIn && !it.busy }
                        assertTrue(store.read() == null, "revocation/logout must clear credentials")
                        core.request("GET", "/api/v1/media-items", status = 401, bearer = loggingOut.token)
                        assertEquals(2L, core.decisionCount())
                    } finally {
                        current.close()
                    }
                }
            }
        }

    private suspend fun verifyMediaFilters(
        model: DesktopModel,
        ui: CapturedUi,
        session: DesktopSession,
    ) {
        val initial = awaitState(model) { it.mediaLibraries.size == 2 && it.wall.size == 2 }
        val item = initial.wall.single { it.title == "Integration movie 1" }
        assertTrue(initial.mediaLibraries.any { it.id == item.libraryId && it.name == "电影库" })
        val filter = MediaFilter(type = "movie", libraryId = item.libraryId, localStatus = "partial", query = "Integration movie 1")
        withContext(ui) { model.applyMediaFilter(filter.copy(query = "  Integration movie 1  ")) }
        awaitState(model) { it.mediaFilter == filter && it.wall.size == 1 && it.wall.single().id == item.id }
        val refreshing = withContext(ui) { ui.capture { model.refresh() } }
        withTimeout(30_000) { refreshing.joinAll() }
        withContext(ui) { }
        assertEquals(filter, model.state.value.mediaFilter)
        assertEquals(
            listOf(item.id),
            model.state.value.wall
                .map { it.id },
        )
        withContext(session.io) { assertEquals(2, session.cache.cachedWall().size) }
        withContext(ui) { model.applyMediaFilter(filter.copy(query = "no matching integration media")) }
        awaitState(model) { it.wallState == PageState.Empty }
        assertTrue(
            model.state.value.wall
                .isEmpty(),
        )
        withContext(ui) { model.applyMediaFilter(MediaFilter()) }
        awaitState(model) { it.mediaFilter == MediaFilter() && it.wall.size == 2 }
    }

    private suspend fun verifyTaskFiltersAndRetry(
        model: DesktopModel,
        ui: CapturedUi,
        session: DesktopSession,
        core: RealCore,
        taskId: String,
    ) {
        for (view in listOf("running", "completed")) {
            withContext(ui) { model.applyTaskFilter(TaskFilter(view = view)) }
            awaitState(model) { it.taskFilter.view == view && it.taskState == PageState.Empty }
        }
        val filter = TaskFilter(view = "all", stage = "identification", status = "paused", query = "Keep Pending")
        withContext(ui) { model.applyTaskFilter(filter) }
        awaitState(model) { it.taskFilter == filter && it.tasks.map { task -> task.id } == listOf(taskId) }
        withContext(ui) { model.selectTask(taskId) }
        val before = assertNotNull(awaitState(model) { it.selectedTask?.id == taskId && !it.taskDetailLoading }.selectedTask)
        assertEquals("paused", before.status.toString())
        assertEquals("identification", before.stage.toString())
        assertNotNull(before.reason)
        assertTrue(before.allowedActions.any { it.toString() == "retry" })
        val originalAttempts = core.attemptCount(taskId)
        val key = "desktop-real-retry-${UUID.randomUUID()}"
        val retried = assertIs<SdkResult.Success<ProcessingTask>>(session.tasks.retry(taskId, key)).value
        val replay = assertIs<SdkResult.Success<ProcessingTask>>(session.tasks.retry(taskId, key)).value
        assertEquals(taskId, retried.id)
        assertEquals(taskId, replay.id)
        assertEquals(before.attemptCount + 1, retried.attemptCount)
        assertEquals(retried.attemptCount, replay.attemptCount)
        core.awaitTask(taskId) { it.text("status") == "paused" && it.number("attempt_count") == before.attemptCount + 1 }
        assertEquals(originalAttempts + 1, core.attemptCount(taskId), "same SDK retry key must create exactly one processing attempt")
        val refreshing = withContext(ui) { ui.capture { model.refresh() } }
        withTimeout(30_000) { refreshing.joinAll() }
        val after = awaitState(model) { it.selectedTask?.attemptCount == before.attemptCount + 1 && !it.taskDetailLoading }
        assertEquals(filter, after.taskFilter)
        assertEquals(listOf(taskId), after.tasks.map { it.id })
        val authority = core.request("GET", "/api/v1/processing-tasks/$taskId")
        val detail = assertNotNull(after.selectedTask)
        assertEquals(authority.text("reason"), detail.reason?.toString())
        assertEquals(authority.text("checkpoint"), detail.checkpoint.toString())
        val nextRetry = authority.text("next_retry_at").takeUnless { it == "null" }?.let { kotlin.time.Instant.parse(it) }
        assertEquals(nextRetry, detail.nextRetryAt)
        withContext(ui) { model.applyTaskFilter(TaskFilter()) }
        awaitState(model) { it.taskFilter == TaskFilter() && it.tasks.size == 4 && it.selectedTaskId == null }
    }

    private suspend fun verifyReadOnlyPairing(
        core: RealCore,
        mediaId: String,
        taskId: String,
        otherDeviceId: String,
    ) {
        val issued = core.request("POST", "/api/v1/device-pairing-codes/media-read-only", status = 201)
        assertEquals("media-read-only", issued.text("access_mode"))
        val store = InMemoryDeviceAuthStore()
        val cache = SessionAuthCache()
        val factory = MediaFlowApiFactory(core.origin, cache)
        val json = MediaFlowApiFactory.defaultJson()
        factory.httpClient().use { transport ->
            val auth = AuthRepository(transport, store, cache, json)
            val elevated = auth.enrollWithPairingCode(issued.text("pairing_code"), "Invalid elevated TV")
            assertTrue(elevated is SdkResult.Failure, "read-only pairing must reject elevated enrollment")
            assertEquals(403, assertIs<MediaFlowError.Http>(elevated.error).status)
            assertTrue(store.read() == null, "rejected enrollment must leave storage empty")
            assertTrue(cache.current() == null, "rejected enrollment must leave memory empty")
            val enrolled = auth.enrollReadOnlyWithPairingCode(issued.text("pairing_code"), "Integration TV")
            assertTrue(enrolled is SdkResult.Success, "read-only SDK enrollment must succeed")
            val saved = store.read()
            assertTrue(saved is StoredCredentials.DeviceToken, "read-only SDK must save a device credential")
            val device = saved
            val deviceId = assertNotNull(device.deviceId)
            assertEquals("media-read-only", core.request("GET", "/api/v1/devices/$deviceId", bearer = device.token).text("access_mode"))
            val api = factory.defaultApi(transport)
            val media = MediaRepository(api, json)
            val libraries = assertNotNull(media.libraries().getOrNull())
            assertEquals(2, libraries.size)
            val wall = media.mediaWall(limit = 1)
            assertTrue(wall.reload())
            assertTrue(wall.loadNext())
            assertEquals(2, wall.items.value.size)
            assertTrue(media.detail(mediaId) is SdkResult.Success)
            val tasks = TaskRepository(api, transport, cache, json)
            val denied = tasks.tasks()
            assertTrue(denied.reload())
            assertTrue(denied.items.value.isEmpty())
            assertEquals(403, assertIs<MediaFlowError.Http>(assertIs<PageState.Error>(denied.state.value).error).status)
            assertEquals(403, assertIs<MediaFlowError.Http>(assertIs<SdkResult.Failure>(tasks.detail(taskId)).error).status)
            assertEquals(
                403,
                assertIs<MediaFlowError.Http>(assertIs<SdkResult.Failure>(tasks.retry(taskId, "readonly-denied-retry")).error).status,
            )
            core.request("GET", "/api/v1/events", status = 403, bearer = device.token)
            core.request("GET", "/api/v1/devices/$otherDeviceId", status = 403, bearer = device.token)
        }
        // A new transport loads the persisted credential, then validates only its own device projection.
        cache.update(null)
        val restoredCache = SessionAuthCache()
        restoredCache.load(store)
        val restoredFactory = MediaFlowApiFactory(core.origin, restoredCache)
        restoredFactory.httpClient().use { transport ->
            val auth = AuthRepository(transport, store, restoredCache, json)
            assertIs<SdkResult.Success<Unit>>(auth.validateReadOnlyDevice())
            val saved = store.read()
            assertTrue(saved is StoredCredentials.DeviceToken, "restored read-only identity must remain a device credential")
            val credential = saved
            val own = core.request("GET", "/api/v1/devices/${credential.deviceId}", bearer = credential.token)
            assertEquals("media-read-only", own.text("access_mode"))
            assertIs<SdkResult.Success<Unit>>(auth.logout())
            assertTrue(store.read() == null, "revoked identity must leave storage empty")
            assertTrue(restoredCache.current() == null, "revoked identity must leave memory empty")
            core.request("GET", "/api/v1/media-items", status = 401, bearer = credential.token)
            store.save(credential)
            restoredCache.update(credential)
            val expired = assertIs<SdkResult.Failure>(auth.validateReadOnlyDevice())
            assertEquals(401, assertIs<MediaFlowError.Http>(expired.error).status)
            assertTrue(store.read() == null, "revoked identity must leave storage empty")
            assertTrue(restoredCache.current() == null, "revoked identity must leave memory empty")
        }
    }

    private suspend fun awaitState(
        model: DesktopModel,
        predicate: (DesktopUi) -> Boolean,
    ): DesktopUi = withTimeout(30_000) { model.state.first(predicate) }

    private class CapturedUi(
        private val delegate: CoroutineDispatcher,
    ) : CoroutineDispatcher() {
        @Volatile private var capturing: Pair<Thread, MutableList<Job>>? = null

        fun capture(block: () -> Unit): List<Job> {
            val jobs = mutableListOf<Job>()
            capturing = Thread.currentThread() to jobs
            try {
                block()
            } finally {
                capturing = null
            }
            return jobs.distinct()
        }

        override fun dispatch(
            context: CoroutineContext,
            block: Runnable,
        ) {
            capturing?.let { capture ->
                if (capture.first === Thread.currentThread()) capture.second += checkNotNull(context[Job])
            }
            delegate.dispatch(context, block)
        }
    }

    private class RealCore : AutoCloseable {
        private val binary =
            System.getenv("MEDIAFLOW_CORE_BINARY")?.let(Path::of)?.also {
                require(it.isAbsolute && Files.isRegularFile(it) && Files.isExecutable(it)) {
                    "MEDIAFLOW_CORE_BINARY must be a freshly built absolute mediaflow-core executable; integration does not skip."
                }
            } ?: error("Set MEDIAFLOW_CORE_BINARY to the freshly built Core executable; integration does not skip.")
        val directory: Path = Files.createTempDirectory("mediaflow-desktop-core-").toRealPath()
        private val config = Files.createDirectory(directory.resolve("config"))
        private val incoming = Files.createDirectory(directory.resolve("incoming"))
        private val report = Files.createDirectories(Path.of("build/reports/real-core/${UUID.randomUUID()}"))
        private val log = report.resolve("core.log")
        private val requests = report.resolve("requests.tsv")
        private val http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(3)).build()
        private val port = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { it.localPort }
        val origin = "http://127.0.0.1:$port"
        val password = "integration-${UUID.randomUUID()}"
        private val bootstrapSecret = "bootstrap-${UUID.randomUUID()}"
        private var process: Process? = null
        private var cookie = ""
        private var csrf = ""

        init {
            Files.writeString(config.resolve("bootstrap.secret"), bootstrapSecret)
            if (directory.fileSystem.supportedFileAttributeViews().contains("posix")) {
                Files.setPosixFilePermissions(config.resolve("bootstrap.secret"), PosixFilePermissions.fromString("rw-------"))
            }
            Files.writeString(directory.resolve("owned-test-directory"), "desktop-real-core")
            Files.writeString(
                config.resolve("deployment-roots.json"),
                """{"roots":[{"id":"incoming","label":"Desktop integration","container_path":${kotlinx.serialization.json.JsonPrimitive(
                    incoming.toString(),
                )},"access":"read-write"}]}""",
            )
        }

        suspend fun start() {
            check(process?.isAlive != true)
            val builder =
                ProcessBuilder(binary.toString(), "serve")
                    .directory(directory.toFile())
                    .redirectErrorStream(true)
                    .redirectOutput(ProcessBuilder.Redirect.appendTo(log.toFile()))
            val environment = builder.environment()
            environment.keys.filter { it.startsWith("MEDIAFLOW_") }.forEach(environment::remove)
            environment.putAll(
                mapOf(
                    "MEDIAFLOW_MODE" to "development",
                    "MEDIAFLOW_LISTEN" to "127.0.0.1:$port",
                    "MEDIAFLOW_CONFIG_DIR" to config.toString(),
                    "MEDIAFLOW_PUBLIC_ORIGIN" to origin,
                    "MEDIAFLOW_DEPLOYMENT_ROOTS_FILE" to config.resolve("deployment-roots.json").toString(),
                    "MEDIAFLOW_WEB_DIST" to directory.resolve("absent-web-dist").toString(),
                ),
            )
            process = builder.start()
            withTimeout(30_000) {
                while (true) {
                    check(alive()) { "Core exited before readiness; see $log" }
                    val ready =
                        runCatching {
                            http
                                .send(
                                    HttpRequest
                                        .newBuilder(URI("$origin/health/ready"))
                                        .timeout(Duration.ofSeconds(2))
                                        .GET()
                                        .build(),
                                    HttpResponse.BodyHandlers.discarding(),
                                ).statusCode()
                        }.getOrNull()
                    if (ready == 200) break
                    delay(50)
                }
            }
        }

        fun alive(): Boolean = process?.isAlive == true

        fun bootstrap() {
            request(
                "POST",
                "/api/v1/system/bootstrap",
                buildJsonObject {
                    put("bootstrap_secret", bootstrapSecret)
                    put("administrator_name", "integration-admin")
                    put("password", password)
                },
                201,
            )
            val login =
                request(
                    "POST",
                    "/api/v1/sessions",
                    buildJsonObject {
                        put("administrator_name", "integration-admin")
                        put("password", password)
                    },
                    201,
                )
            csrf = login.text("csrf_token")
        }

        suspend fun discoverFiles(): List<String> {
            for (name in listOf("Journey One (2024).mkv", "Journey Two (2025).mkv", "Cancel Me (2023).mkv", "Keep Pending (2022).mkv")) {
                Files.writeString(incoming.resolve(name), "isolated desktop integration media")
            }
            val inbox =
                request(
                    "POST",
                    "/api/v1/inbox-directories",
                    buildJsonObject {
                        put("root_id", "incoming")
                        put("relative_path", ".")
                    },
                    201,
                ).text("id")
            request(
                "PUT",
                "/api/v1/inbox-directories/$inbox/discovery-policy",
                buildJsonObject {
                    put("minimum_age_seconds", 0)
                    put("stable_observation_interval_seconds", 1)
                    put("reconcile_interval_seconds", 60)
                    put("watcher_enabled", false)
                },
                match = "1",
            )
            scan(inbox)
            // The production policy requires observations at least one second apart.
            delay(1_100)
            scan(inbox)
            val names = listOf("Journey One (2024).mkv", "Journey Two (2025).mkv", "Cancel Me (2023).mkv", "Keep Pending (2022).mkv")
            return withTimeout(30_000) {
                while (true) {
                    val tasks = request("GET", "/api/v1/processing-tasks").getValue("items").jsonArray.map { it.jsonObject }
                    if (tasks.size == 4 && tasks.all { it.text("status") == "paused" }) {
                        return@withTimeout names.map { name -> tasks.single { it.text("relative_path") == name }.text("id") }
                    }
                    delay(100)
                }
                @Suppress("UNREACHABLE_CODE")
                emptyList()
            }
        }

        private suspend fun scan(inbox: String) {
            val task = request("POST", "/api/v1/inbox-directories/$inbox/scan-tasks", status = 202, key = "scan-${UUID.randomUUID()}")
            withTimeout(15_000) {
                while (request("GET", "/api/v1/scan-tasks/${task.text("id")}").text("status") !in
                    listOf("succeeded", "completed")
                ) {
                    delay(50)
                }
            }
        }

        suspend fun awaitTask(
            id: String,
            accept: (JsonObject) -> Boolean,
        ) {
            withTimeout(30_000) {
                while (!accept(request("GET", "/api/v1/processing-tasks/$id"))) delay(100)
            }
        }

        fun request(
            method: String,
            path: String,
            body: JsonElement? = null,
            status: Int = 200,
            bearer: String? = null,
            match: String? = null,
            key: String? = null,
        ): JsonObject {
            val builder =
                HttpRequest
                    .newBuilder(URI("$origin$path"))
                    .timeout(Duration.ofSeconds(5))
                    .header("Origin", origin)
                    .header("Sec-Fetch-Site", "same-origin")
            if (bearer != null) {
                builder.header("Authorization", "Bearer $bearer")
            } else if (cookie.isNotEmpty()) {
                builder.header("Cookie", cookie)
                if (csrf.isNotEmpty()) builder.header("X-CSRF-Token", csrf)
            }
            match?.let { builder.header("If-Match", it) }
            key?.let { builder.header("Idempotency-Key", it) }
            if (body != null) builder.header("Content-Type", "application/json")
            val response =
                http.send(
                    builder
                        .method(
                            method,
                            body?.let { HttpRequest.BodyPublishers.ofString(it.toString()) } ?: HttpRequest.BodyPublishers.noBody(),
                        ).build(),
                    HttpResponse.BodyHandlers.ofString(),
                )
            Files.writeString(
                requests,
                "$method\t$path\t${response.statusCode()}\n",
                java.nio.file.StandardOpenOption.CREATE,
                java.nio.file.StandardOpenOption.APPEND,
            )
            assertEquals(status, response.statusCode(), "$method $path status; response bodies and credentials are not logged")
            if (path == "/api/v1/sessions" &&
                status == 201
            ) {
                cookie =
                    response
                        .headers()
                        .firstValue("Set-Cookie")
                        .orElseThrow()
                        .substringBefore(';')
            }
            return if (status == 204) {
                JsonObject(emptyMap())
            } else {
                runCatching { Json.parseToJsonElement(response.body()).jsonObject }
                    .getOrElse { error("$method $path returned invalid JSON; response body is not logged") }
            }
        }

        suspend fun seedReadProjections(taskIds: List<String>): List<String> {
            stop()
            val db = config.resolve("mediaflow.db").toRealPath()
            check(
                !alive() &&
                    db.parent == config.toRealPath() &&
                    config.parent == directory &&
                    Files.readString(directory.resolve("owned-test-directory")) == "desktop-real-core",
            )
            val cases = mutableListOf<String>()
            DriverManager.getConnection("jdbc:sqlite:$db").use { connection ->
                connection.createStatement().use { it.execute("PRAGMA foreign_keys=ON") }
                connection.autoCommit = false
                try {
                    for (task in taskIds) {
                        val processing = bytes(UUID.randomUUID())
                        val attempt = bytes(UUID.randomUUID())
                        val decision = bytes(UUID.randomUUID())
                        val review = UUID.randomUUID()
                        val now = System.currentTimeMillis() * 1_000
                        execute(
                            connection,
                            "INSERT INTO tasks_processing_attempts (id,task_id,reason,ordinal,status,stage,finished_at_us,created_at_us) SELECT ?,id,'manual-retry',attempt_count+1,'succeeded','identification',?,? FROM tasks_processing_tasks WHERE id=? AND status='paused' AND current_task_decision_id IS NULL",
                            processing,
                            now,
                            now,
                            bytes(UUID.fromString(task)),
                        )
                        execute(
                            connection,
                            "INSERT INTO identification_attempts (id,task_id,processing_attempt_id,file_revision_id,ordinal,parser_version,provider_version,rule_version,status,started_at_us,finished_at_us) SELECT ?,id,?,file_revision_id,(SELECT COALESCE(MAX(ordinal),0)+1 FROM identification_attempts WHERE task_id=tasks_processing_tasks.id),'desktop-integration','preloaded',1,'decided',?,? FROM tasks_processing_tasks WHERE id=?",
                            attempt,
                            processing,
                            now,
                            now,
                            bytes(UUID.fromString(task)),
                        )
                        execute(
                            connection,
                            "INSERT INTO identification_decisions (id,attempt_id,level,reason,rule_version,decided_at_us) VALUES (?,?,'unidentified','identification.no-candidate',1,?)",
                            decision,
                            attempt,
                            now,
                        )
                        execute(
                            connection,
                            "INSERT INTO identification_review_cases (id,account_id,task_id,attempt_id,decision_id,file_revision_id,inbox_directory_id,level,reason,title_hint,status,created_at_us,updated_at_us) SELECT ?,account_id,id,?,?,file_revision_id,inbox_directory_id,'unidentified','identification.no-candidate','Integration review','active',?,? FROM tasks_processing_tasks WHERE id=?",
                            bytes(review),
                            attempt,
                            decision,
                            now,
                            now,
                            bytes(UUID.fromString(task)),
                        )
                        execute(
                            connection,
                            "UPDATE tasks_processing_tasks SET current_attempt_id=?,attempt_count=attempt_count+1,status='waiting-confirmation',stage='identification',checkpoint='waiting-confirmation',reason='identification.no-candidate',lease_owner=NULL,lease_expires_at_us=NULL,next_retry_at_us=NULL,cancel_requested=0,version=version+1 WHERE id=?",
                            processing,
                            bytes(UUID.fromString(task)),
                        )
                        cases += review.toString()
                    }
                    // Preload partial catalog reads, not verified file assets or an asserted publication journey.
                    for (index in 1..2) {
                        execute(
                            connection,
                            "INSERT INTO catalog_media_items (id,account_id,library_id,kind,title,title_normalized,local_status,nfo_status,projection_version,created_at_us,updated_at_us) SELECT ?,id,?,'movie',?,?,'partial','not-requested',1,?,? FROM identity_accounts",
                            bytes(UUID.randomUUID()),
                            bytes(UUID.randomUUID()),
                            "Integration movie $index",
                            "integration movie $index",
                            System.currentTimeMillis() * 1_000 + index,
                            System.currentTimeMillis() * 1_000 + index,
                        )
                    }
                    connection.commit()
                } catch (failure: Throwable) {
                    connection.rollback()
                    throw failure
                }
            }
            Files.writeString(
                report.resolve("data-boundary.txt"),
                "Public API: bootstrap, pairing, inbox, scan, four real paused tasks.\nStopped owned DB: two review projections and two partial catalog read projections.\nNo TMDB, automatic review creation, organization or catalog publication claim.\n",
            )
            start()
            return cases
        }

        fun attemptCount(taskId: String): Long =
            DriverManager.getConnection("jdbc:sqlite:${config.resolve("mediaflow.db")}").use { connection ->
                connection.prepareStatement("SELECT COUNT(*) FROM tasks_processing_attempts WHERE task_id=?").use { statement ->
                    statement.setBytes(1, bytes(UUID.fromString(taskId)))
                    statement.executeQuery().use { rows ->
                        check(rows.next())
                        rows.getLong(1)
                    }
                }
            }

        fun decisionCount(): Long =
            DriverManager.getConnection("jdbc:sqlite:${config.resolve("mediaflow.db")}").use { connection ->
                connection.createStatement().use { statement ->
                    statement.executeQuery("SELECT COUNT(*) FROM identification_task_decisions").use { rows ->
                        rows.next()
                        rows.getLong(1)
                    }
                }
            }

        private fun execute(
            connection: Connection,
            sql: String,
            vararg values: Any,
        ) {
            connection.prepareStatement(sql).use { statement ->
                values.forEachIndexed { index, value -> statement.setObject(index + 1, value) }
                assertEquals(1, statement.executeUpdate(), "run-owned fixture mutation must affect exactly one row")
            }
        }

        private fun bytes(id: UUID): ByteArray =
            ByteBuffer
                .allocate(16)
                .putLong(id.mostSignificantBits)
                .putLong(id.leastSignificantBits)
                .array()

        private fun stop() {
            process?.let { child ->
                if (child.isAlive) child.destroy()
                if (!child.waitFor(10, TimeUnit.SECONDS)) {
                    child.destroyForcibly()
                    check(child.waitFor(5, TimeUnit.SECONDS)) { "owned Core process did not exit" }
                }
            }
        }

        override fun close() {
            stop()
            http.close()
            check(directory.toFile().deleteRecursively()) { "run-owned temporary files could not be removed" }
        }
    }
}

private fun JsonObject.text(name: String): String = getValue(name).jsonPrimitive.content

private fun JsonObject.number(name: String): Long = text(name).toLong()
