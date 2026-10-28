package mediaflow.tv

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.MockEngineConfig
import io.ktor.client.engine.mock.respond
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.client.plugins.defaultRequest
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** 真正的SDK与受控HTTP，验证TV所有者的身份、取消和重试边界。 */
@OptIn(ExperimentalCoroutinesApi::class)
class TvModelTest {
    @Test
    fun serverOriginAndStableFocusRejectUnsafeInputs() {
        assertEquals("https://nas.local", normalizeTvServer(" HTTPS://NAS.local:443/ ", false))
        assertEquals("http://127.0.0.1:8642", normalizeTvServer("http://127.0.0.1:8642/", true))
        for (input in listOf(
            "http://nas.local",
            "https://user@nas.local",
            "https://nas.local/api",
            "https://nas.local?q=x",
            "https://nas.local#x",
            "https://nas.local:0",
            "file:///etc/passwd",
        )) {
            assertFailsWith<IllegalArgumentException> { normalizeTvServer(input, false) }
        }
        assertEquals("b", restoredMediaId(listOf("a", "b"), "b", 0))
        assertEquals("b", restoredMediaId(listOf("a", "b"), "removed", 9))
        assertNull(restoredMediaId(emptyList(), "removed", 1))
    }

    @Test
    fun completePosterAndScaledCaptionFitTheAvailableFirstRow() {
        for ((width, height, scale) in listOf(Triple(864f, 358f, 1f), Triple(720f, 300f, 1.5f))) {
            val columns = tvPosterColumns(width, height, scale)
            val cell = (width - 16f - (columns - 1) * 24f) / columns
            assertTrue(cell * 1.5f + 90f * scale + 8f <= height - 16f)
        }
    }

    @Test
    fun failedInitialDetailCanRetryAndLogoutClosesOwner() =
        runTest {
            var details = 0
            val remembered = mutableListOf<String>()
            val owner = CoroutineScope(coroutineContext + SupervisorJob(coroutineContext[Job]))
            val cache = SessionAuthCache()
            val store = InMemoryDeviceAuthStore()
            val client =
                client(StandardTestDispatcher(testScheduler)) { path, _ ->
                    when (path) {
                        "/api/v1/devices/enrollment" -> enrollment to HttpStatusCode.Created
                        "/api/v1/media-items/$MEDIA" -> {
                            details++
                            if (details == 1) "{}" to HttpStatusCode.ServiceUnavailable else detail to HttpStatusCode.OK
                        }
                        "/api/v1/devices/$DEVICE" -> "" to HttpStatusCode.NoContent
                        else -> "{\"items\":[]}" to HttpStatusCode.OK
                    }
                }
            val model = TvModel(owner, { TvSession(it, store, cache, client, json) }, remembered::add)
            try {
                model.connect("https://nas.local", false, "once-only")
                advanceUntilIdle()
                assertTrue(model.state.value.connected)
                model.detail(MEDIA)
                advanceUntilIdle()
                assertEquals(1, details)
                assertFalse(model.state.value.detailLoading)
                assertNull(model.state.value.detail)
                model.refresh()
                advanceUntilIdle()
                assertEquals(2, details, "retry must remember the selected id after a failed initial detail")
                assertEquals(
                    MEDIA,
                    model.state.value.detail
                        ?.item
                        ?.id,
                )
                model.logout()
                advanceUntilIdle()
                assertFalse(model.state.value.connected)
                assertEquals("", remembered.last(), "logout must disable cold auto-restore even if storage later fails")
                assertNull(cache.current())
                assertNull(store.read())
                assertTrue(owner.coroutineContext[Job]!!.children.none { it.children.any() })
            } finally {
                model.close()
                owner.cancel()
            }
        }

    @Test
    fun filterSwitchCancelsOldPageAndCloseClearsCredentials() =
        runTest {
            val gate = CompletableDeferred<Unit>()
            var oldCancelled = false
            val owner = CoroutineScope(coroutineContext + SupervisorJob(coroutineContext[Job]))
            val cache = SessionAuthCache()
            val client =
                client(StandardTestDispatcher(testScheduler)) { path, query ->
                    if (path.endsWith("enrollment")) {
                        enrollment to HttpStatusCode.Created
                    } else if (path == "/api/v1/media-items" && query == "old") {
                        try {
                            gate.await()
                        } finally {
                            oldCancelled = true
                        }
                        "{\"items\":[]}" to HttpStatusCode.OK
                    } else {
                        "{\"items\":[]}" to HttpStatusCode.OK
                    }
                }
            val model = TvModel(owner, { TvSession(it, InMemoryDeviceAuthStore(), cache, client, json) })
            try {
                model.connect("https://nas.local", false, "code")
                advanceUntilIdle()
                model.filter(MediaFilter(query = "old"))
                runCurrent()
                model.filter(MediaFilter(query = "new"))
                advanceUntilIdle()
                assertTrue(oldCancelled)
                assertEquals("new", model.state.value.filter.query)
                assertTrue(
                    model.state.value.wall
                        .isEmpty(),
                )
                model.close()
                assertNull(cache.current())
                assertFalse(model.state.value.connected)
            } finally {
                gate.complete(Unit)
                model.close()
                owner.cancel()
            }
        }

    @Test
    fun restoreDoesNotPublishAnAdministratorDevice() =
        runTest {
            var mediaReads = 0
            val store = InMemoryDeviceAuthStore().also { it.save(StoredCredentials.DeviceToken("secret", DEVICE)) }
            val cache = SessionAuthCache()
            val owner = CoroutineScope(coroutineContext + SupervisorJob(coroutineContext[Job]))
            val client =
                client(StandardTestDispatcher(testScheduler)) { path, _ ->
                    if (path.startsWith("/api/v1/media")) mediaReads++
                    device.replace("media-read-only", "administrator") to HttpStatusCode.OK
                }
            val model = TvModel(owner, { TvSession(it, store, cache, client, json) })
            try {
                model.connect("https://nas.local", false, restore = true)
                advanceUntilIdle()
                assertFalse(model.state.value.connected)
                assertEquals(0, mediaReads)
                assertNull(store.read())
                assertNull(cache.current())
            } finally {
                model.close()
                owner.cancel()
            }
        }

    @Test
    fun logoutStorageFailureStillHidesContentAndDisablesAutoRestore() =
        runTest {
            val remembered = mutableListOf<String>()
            val owner = CoroutineScope(coroutineContext + SupervisorJob(coroutineContext[Job]))
            val cache = SessionAuthCache()
            val store =
                object : DeviceAuthStore {
                    override suspend fun read(): StoredCredentials? = null

                    override suspend fun save(credentials: StoredCredentials) = Unit

                    override suspend fun clear(): Unit = error("controlled secure-store failure")
                }
            val client =
                client(StandardTestDispatcher(testScheduler)) { path, _ ->
                    if (path.endsWith("enrollment")) {
                        enrollment to HttpStatusCode.Created
                    } else if (path.startsWith("/api/v1/devices/")) {
                        "{}" to HttpStatusCode.ServiceUnavailable
                    } else {
                        "{\"items\":[]}" to HttpStatusCode.OK
                    }
                }
            val model = TvModel(owner, { TvSession(it, store, cache, client, json) }, remembered::add)
            try {
                model.connect("https://nas.local", false, "code")
                advanceUntilIdle()
                model.logout()
                advanceUntilIdle()
                assertFalse(model.state.value.connected)
                assertEquals("", remembered.last())
                assertNull(cache.current())
                assertTrue(
                    model.state.value.message
                        ?.contains("Web") == true,
                )
            } finally {
                model.close()
                owner.cancel()
            }
        }

    @Test
    fun explicitAndSystemThemesResolveAndPersistWithoutSessionOwnership() =
        runTest {
            assertEquals(TvTheme.System, TvTheme.fromPreference("unknown"))
            assertTrue(TvTheme.System.isDark(true))
            assertFalse(TvTheme.System.isDark(false))
            assertFalse(TvTheme.Light.isDark(true))
            assertTrue(TvTheme.Dark.isDark(false))
            var saved = TvTheme.System
            val owner = CoroutineScope(coroutineContext + SupervisorJob(coroutineContext[Job]))
            val cache = SessionAuthCache()
            val client =
                client(StandardTestDispatcher(testScheduler)) { path, _ ->
                    when {
                        path.endsWith("enrollment") -> enrollment to HttpStatusCode.Created
                        path.startsWith("/api/v1/devices/") -> "" to HttpStatusCode.NoContent
                        else -> "{\"items\":[]}" to HttpStatusCode.OK
                    }
                }
            val model = TvModel(owner, { TvSession(it, InMemoryDeviceAuthStore(), cache, client, json) }, rememberTheme = { saved = it })
            try {
                model.setTheme(TvTheme.Light)
                assertEquals(TvTheme.Light, saved)
                model.connect("https://nas.local", false, "code")
                advanceUntilIdle()
                assertEquals("TV", model.state.value.deviceName)
                assertEquals(TvTheme.Light, model.theme.value)
                model.logout()
                advanceUntilIdle()
                assertEquals(TvTheme.Light, model.theme.value)
                val restored = TvModel(owner, { error("must not connect") }, initialTheme = saved)
                assertEquals(TvTheme.Light, restored.theme.value)
                restored.close()
            } finally {
                model.close()
                owner.cancel()
            }
        }

    @Test
    fun failedThemePersistenceKeepsThePreviousSelectionAndShowsRecovery() =
        runTest {
            val model = TvModel(backgroundScope, { error("must not connect") }, rememberTheme = { error("storage sentinel") })
            try {
                model.setTheme(TvTheme.Dark)
                assertEquals(TvTheme.System, model.theme.value)
                assertTrue(
                    model.state.value.message
                        ?.contains("主题") == true,
                )
            } finally {
                model.close()
            }
        }

    @Test
    fun staleOrDuplicateLogoutConfirmationCannotActOnAnotherIdentity() =
        runTest {
            var deletes = 0
            val owner = CoroutineScope(coroutineContext + SupervisorJob(coroutineContext[Job]))
            val model =
                TvModel(owner, { server ->
                    val client =
                        client(StandardTestDispatcher(testScheduler)) { path, _ ->
                            when {
                                path.endsWith("enrollment") -> enrollment to HttpStatusCode.Created
                                path.startsWith("/api/v1/devices/") -> {
                                    deletes++
                                    "" to HttpStatusCode.NoContent
                                }
                                else -> "{\"items\":[]}" to HttpStatusCode.OK
                            }
                        }
                    TvSession(server, InMemoryDeviceAuthStore(), SessionAuthCache(), client, json)
                })
            try {
                model.connect("https://nas.local", false, "code")
                advanceUntilIdle()
                val previousIdentity = model.state.value.identity
                model.connect("https://nas.local", false, "new-code")
                advanceUntilIdle()
                val identity = model.state.value.identity
                assertTrue(previousIdentity != identity)
                assertFalse(model.confirmLogout(previousIdentity))
                assertTrue(model.state.value.connected)
                assertEquals(0, deletes)
                assertTrue(model.confirmLogout(identity))
                assertFalse(model.confirmLogout(identity))
                advanceUntilIdle()
                assertEquals(1, deletes)
                assertFalse(model.state.value.connected)
            } finally {
                model.close()
                owner.cancel()
            }
        }

    private fun client(
        dispatcher: CoroutineDispatcher,
        response: suspend (String, String?) -> Pair<String, HttpStatusCode>,
    ): HttpClient =
        HttpClient(
            MockEngine(
                MockEngineConfig().apply {
                    this.dispatcher = dispatcher
                    addHandler { request ->
                        val (body, status) = response(request.url.encodedPath, request.url.parameters["q"])
                        respond(body, status, headersOf("Content-Type", "application/json"))
                    }
                },
            ),
        ) {
            defaultRequest { url("https://nas.local") }
            install(ContentNegotiation) { json(this@TvModelTest.json) }
        }

    private val json = MediaFlowApiFactory.defaultJson()
    private val device =
        """
        {
          "id": "$DEVICE",
          "display_name": "TV",
          "created_at": "2026-10-28T00:00:00Z",
          "expires_at": "2027-10-28T00:00:00Z",
          "access_mode": "media-read-only"
        }
        """.trimIndent()
    private val enrollment = """{"token":"fixture-secret","device":$device}"""
    private val detail =
        """
        {
          "item": {
            "id": "$MEDIA",
            "type": "movie",
            "title": "Test movie",
            "local_status": "partial",
            "library_id": "$DEVICE",
            "updated_at": "2026-10-28T00:00:00Z"
          },
          "metadata": [],
          "versions": [],
          "children": [],
          "nfo_status": "not-requested",
          "related_task_ids": []
        }
        """.trimIndent()

    private companion object {
        const val DEVICE = "019f0000-0000-7000-8000-000000000051"
        const val MEDIA = "019f0000-0000-7000-8000-000000000061"
    }
}
