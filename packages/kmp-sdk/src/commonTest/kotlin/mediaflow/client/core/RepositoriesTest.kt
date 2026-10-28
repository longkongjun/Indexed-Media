package mediaflow.client.core

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.client.request.HttpRequestData
import io.ktor.http.HeadersBuilder
import io.ktor.http.HttpStatusCode
import io.ktor.http.content.TextContent
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlinx.io.IOException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mediaflow.api.apis.DefaultApi
import mediaflow.api.models.SelectProviderCandidateCommand
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.playback.PlaybackRepository
import mediaflow.client.core.review.ReviewRepository
import mediaflow.client.core.tasks.TaskRepository
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

class RepositoriesTest {
    private val json = Json { ignoreUnknownKeys = true }
    private val okJsonHeaders = headersOf("Content-Type", "application/json")

    private fun apiOf(engine: MockEngine): DefaultApi =
        DefaultApi(
            baseUrl = "https://nas.local",
            httpClient =
                HttpClient(engine) {
                    expectSuccess = false
                    install(ContentNegotiation) { json(MediaFlowApiFactory.defaultJson()) }
                },
        )

    private fun authStack() = Pair(InMemoryDeviceAuthStore(), SessionAuthCache())

    @Test
    fun loginParsesCookieAndPersistsCredentials() =
        runTest {
            val engine =
                MockEngine {
                    respond(
                        content =
                            """{"account":{"id":"019f0000-0000-7000-8000-000000000001","administrator_name":"admin"},""" +
                                """"csrf_token":"tok-tok-tok-tok-tok-tok-tok-tok-tok-tok-tok-1","version":"v1"}""",
                        status = HttpStatusCode.Created,
                        headers =
                            HeadersBuilder()
                                .apply {
                                    append(
                                        "Set-Cookie",
                                        "__Host-mediaflow_session=abc123; Secure; HttpOnly; SameSite=Strict; Path=/",
                                    )
                                    append("Content-Type", "application/json")
                                }.build(),
                    )
                }
            val (store, cache) = authStack()
            val repo = AuthRepository(HttpClient(engine), store, cache, json)
            val result = repo.login("admin", "secret")
            val credentials =
                assertIs<SdkResult.Success<StoredCredentials.SessionCookie>>(result).value
            assertEquals("abc123", credentials.sessionCookieValue)
            assertEquals("tok-tok-tok-tok-tok-tok-tok-tok-tok-tok-tok-1", credentials.csrfToken)
            assertEquals(
                "abc123",
                (cache.current() as StoredCredentials.SessionCookie).sessionCookieValue,
            )
            assertEquals("admin", store.read()?.displayName)
        }

    @Test
    fun loginFailureMapsHttpError() =
        runTest {
            val engine =
                MockEngine {
                    respond(
                        content =
                            """{"error":{"code":"auth.invalid_credentials","message":"bad credentials",""" +
                                """"request_id":"r1"}}""",
                        status = HttpStatusCode.Unauthorized,
                        headers = okJsonHeaders,
                    )
                }
            val (store, cache) = authStack()
            val repo = AuthRepository(HttpClient(engine), store, cache, json)
            val failure = assertIs<SdkResult.Failure>(repo.login("admin", "wrong"))
            assertEquals(401, (failure.error as mediaflow.client.core.error.MediaFlowError.Http).status)
        }

    @Test
    fun pairingEnrollmentStoresDeviceTokenAndInjectsBearer() =
        runTest {
            val authHistory = mutableListOf<String?>()
            val engine =
                MockEngine { request ->
                    authHistory.add(request.headers["Authorization"])
                    if (request.url.encodedPath == "/api/v1/devices/enrollment") {
                        respond(
                            content =
                                """{"device":{"id":"019f8a50-7271-7000-8000-000000000011",""" +
                                    """"display_name":"Pixel 9","created_at":"2026-10-11T08:00:00.000001Z",""" +
                                    """"last_used_at":null,"expires_at":"2027-04-10T08:00:00.000001Z",""" +
                                    """"revoked_at":null},"token":"mfdev_TOKEN43","version":"v1"}""",
                            status = HttpStatusCode.Created,
                            headers = okJsonHeaders,
                        )
                    } else {
                        respond(
                            content = """{"items":[],"next_cursor":null}""",
                            status = HttpStatusCode.OK,
                            headers = okJsonHeaders,
                        )
                    }
                }
            val (store, cache) = authStack()
            val client =
                HttpClient(engine) {
                    install(
                        mediaflow.client.core.net.AuthHeaderPlugin
                            .create(cache),
                    )
                }
            val repo = AuthRepository(client, store, cache, json)
            val result = repo.enrollWithPairingCode("ABCD2345", "Pixel 9")
            val credentials =
                assertIs<SdkResult.Success<StoredCredentials.DeviceToken>>(result).value
            assertEquals("mfdev_TOKEN43", credentials.token)
            assertEquals("019f8a50-7271-7000-8000-000000000011", credentials.deviceId)
            assertEquals("Pixel 9", credentials.displayName)
            // enrollment 请求本身无 Bearer（预认证端点）。
            assertEquals(1, authHistory.size)
            assertNull(authHistory.single())
            // 设备凭据下的业务读：注入 Bearer 而非 cookie。
            val tasks =
                TaskRepository(
                    mediaflow.api.apis.DefaultApi(baseUrl = "https://nas.local", httpClient = client),
                    client,
                    cache,
                    json,
                ).firstPageOfTasks()
            assertEquals(200, tasks.status)
            assertEquals("Bearer mfdev_TOKEN43", authHistory.last())
            assertIs<StoredCredentials.DeviceToken>(store.read())
        }

    @Test
    fun deviceWriteOmitsCsrfBecauseTokenIsTheProof() =
        runTest {
            var captured: HttpRequestData? = null
            val engine =
                MockEngine { request ->
                    captured = request
                    respond(
                        content =
                            """{"id":"019f0000-0000-7000-8000-000000000043",""" +
                                """"inbox_directory_id":"019f0000-0000-7000-8000-000000000002",""" +
                                """"file_revision_id":"019f0000-0000-7000-8000-000000000044",""" +
                                """"relative_path":"电影/流浪地球 (2019)/流浪地球 (2019).mkv",""" +
                                """"status":"waiting-confirmation","stage":"identification",""" +
                                """"checkpoint":"waiting-confirmation","decision_checkpoint":null,""" +
                                """"current_task_decision_id":null,"reason":"identification.ambiguous",""" +
                                """"recovering":false,"attempt_count":1,"next_retry_at":null,""" +
                                """"allowed_actions":["review","retry","cancel"],""" +
                                """"updated_at":"2026-07-23T08:00:00Z"}""",
                        status = HttpStatusCode.Accepted,
                        headers = okJsonHeaders,
                    )
                }
            val (_, cache) = authStack()
            cache.update(StoredCredentials.DeviceToken(token = "mfdev_TOKEN43", deviceId = "019f8a50-7271-7000-8000-000000000011"))
            val repo = TaskRepository(apiOf(engine), HttpClient(engine), cache, json)
            val result = repo.cancel("task-1", idempotencyKey = "cancel-key-0001")
            assertIs<SdkResult.Success<mediaflow.api.models.ProcessingTask>>(result)
            // 设备令牌通道：幂等键照传，CSRF 头不出现（令牌即写证明）。
            assertEquals("cancel-key-0001", captured!!.headers["Idempotency-Key"])
            assertNull(captured!!.headers["X-CSRF-Token"])
        }

    @Test
    fun tokenRotationReplacesCredentialAndLogoutRevokesDevice() =
        runTest {
            val engine =
                MockEngine { request ->
                    when {
                        request.url.encodedPath == "/api/v1/devices/current/token-rotation" ->
                            respond(
                                content =
                                    """{"token":"mfdev_FRESH43","expires_at":"2027-04-10T09:00:00.000001Z",""" +
                                        """"version":"v1"}""",
                                status = HttpStatusCode.OK,
                                headers = okJsonHeaders,
                            )
                        request.url.encodedPath == "/api/v1/devices/019f8a50-7271-7000-8000-000000000011" &&
                            request.method.value == "DELETE" ->
                            respond(
                                content = "",
                                status = HttpStatusCode.NoContent,
                                headers = okJsonHeaders,
                            )
                        else -> respond(content = "", status = HttpStatusCode.NotFound, headers = okJsonHeaders)
                    }
                }
            val (store, cache) = authStack()
            cache.update(
                StoredCredentials.DeviceToken(
                    token = "mfdev_TOKEN43",
                    deviceId = "019f8a50-7271-7000-8000-000000000011",
                ),
            )
            store.save(cache.current()!!)
            val repo = AuthRepository(HttpClient(engine), store, cache, json)
            val rotated = repo.rotateDeviceToken()
            assertEquals("mfdev_FRESH43", (rotated as SdkResult.Success<StoredCredentials.DeviceToken>).value.token)
            assertEquals("mfdev_FRESH43", (store.read() as StoredCredentials.DeviceToken).token)
            val logout = repo.logout()
            assertIs<SdkResult.Success<Unit>>(logout)
            assertNull(store.read())
            assertNull(cache.current())
        }

    @Test
    fun submitDecisionSendsFlatKindBodyWithContractHeaders() =
        runTest {
            var captured: HttpRequestData? = null
            var capturedBody: String? = null
            val engine =
                MockEngine { request ->
                    captured = request
                    capturedBody = (request.body as? TextContent)?.text
                    respond(
                        content =
                            """{"id":"019f0000-0000-7000-8000-000000000099",""" +
                                """"task_id":"019f0000-0000-7000-8000-000000000003",""" +
                                """"review_case_id":"019f0000-0000-7000-8000-000000000002",""" +
                                """"case_version":3,"kind":"select-provider-candidate",""" +
                                """"state":"accepted","created_at":"2026-07-17T00:00:00Z"}""",
                        status = HttpStatusCode.Accepted,
                        headers = okJsonHeaders,
                    )
                }
            val (_, cache) = authStack()
            cache.update(StoredCredentials.SessionCookie("cookie", "csrf-token-1"))
            val repo = ReviewRepository(apiOf(engine), HttpClient(engine), cache, json)
            val command =
                SelectProviderCandidateCommand(
                    kind = "select-provider-candidate",
                    provider = "tmdb",
                    mediaType = SelectProviderCandidateCommand.MediaType.movie,
                    providerId = "438631",
                    saveFeedback = false,
                )
            val result =
                repo.submitDecision(
                    reviewCaseId = "case-1",
                    caseVersion = 3,
                    idempotencyKey = "decision-key-0001",
                    command = ReviewRepository.ReviewDecisionKind.SelectProviderCandidate(command),
                )
            assertIs<SdkResult.Success<mediaflow.api.models.TaskDecisionReceipt>>(result)
            val request = captured!!
            assertEquals("decision-key-0001", request.headers["Idempotency-Key"])
            assertEquals("3", request.headers["If-Match"])
            assertEquals("csrf-token-1", request.headers["X-CSRF-Token"])
            // 平铺 kind 判别：kind 在顶层，无包装对象（决策 D-0007 钉住形状）。
            val bodyJson = json.parseToJsonElement(capturedBody!!).jsonObject
            assertEquals("select-provider-candidate", bodyJson["kind"]?.jsonPrimitive?.content)
            assertEquals("438631", bodyJson["provider_id"]?.jsonPrimitive?.content)
        }

    @Test
    fun taskCancelPassesIdempotencyKeyAndCsrf() =
        runTest {
            var captured: HttpRequestData? = null
            val engine =
                MockEngine { request ->
                    captured = request
                    respond(
                        // 契约 golden fixture（contracts/examples/v1/processing-task.json）按字段安全分块内联。
                        content =
                            """{"id":"019f0000-0000-7000-8000-000000000043",""" +
                                """"inbox_directory_id":"019f0000-0000-7000-8000-000000000002",""" +
                                """"file_revision_id":"019f0000-0000-7000-8000-000000000044",""" +
                                """"relative_path":"电影/流浪地球 (2019)/流浪地球 (2019).mkv",""" +
                                """"status":"waiting-confirmation","stage":"identification",""" +
                                """"checkpoint":"waiting-confirmation","decision_checkpoint":null,""" +
                                """"current_task_decision_id":null,"reason":"identification.ambiguous",""" +
                                """"recovering":false,"attempt_count":1,"next_retry_at":null,""" +
                                """"allowed_actions":["review","retry","cancel"],""" +
                                """"updated_at":"2026-07-23T08:00:00Z"}""",
                        status = HttpStatusCode.Accepted,
                        headers = okJsonHeaders,
                    )
                }
            val (_, cache) = authStack()
            cache.update(StoredCredentials.SessionCookie("cookie", "csrf-token-2"))
            val repo = TaskRepository(apiOf(engine), HttpClient(engine), cache, json)
            val result = repo.cancel("task-1", idempotencyKey = "cancel-key-0001")
            val task = assertIs<SdkResult.Success<mediaflow.api.models.ProcessingTask>>(result).value
            assertEquals("waiting-confirmation", task.status.toString())
            assertEquals("cancel-key-0001", captured!!.headers["Idempotency-Key"])
        }

    @Test
    fun taskCancelPreservesHttpFailureDetails() =
        runTest {
            for ((status, code) in listOf(
                HttpStatusCode.Unauthorized to "session.expired",
                HttpStatusCode.Forbidden to "permission.denied",
                HttpStatusCode.ServiceUnavailable to "internal.error",
            )) {
                val client =
                    HttpClient(
                        MockEngine {
                            respond(
                                content = """{"error":{"code":"$code","message":"cancel rejected","request_id":"cancel-fixture"}}""",
                                status = status,
                                headers = okJsonHeaders,
                            )
                        },
                    )
                try {
                    val repo =
                        TaskRepository(DefaultApi(baseUrl = "https://nas.local", httpClient = client), client, SessionAuthCache(), json)
                    val failure = assertIs<SdkResult.Failure>(repo.cancel("task-1", "cancel-failure-0001"))
                    val error = assertIs<MediaFlowError.Http>(failure.error)
                    assertEquals(status.value, error.status)
                    assertEquals(code, error.code)
                    assertEquals("cancel-fixture", error.requestId)
                } finally {
                    client.close()
                }
            }
        }

    @Test
    fun taskCancelMapsLostResponseAndPropagatesCancellation() =
        runTest {
            val lost = HttpClient(MockEngine { throw IOException("response lost") })
            try {
                val repo = TaskRepository(DefaultApi(baseUrl = "https://nas.local", httpClient = lost), lost, SessionAuthCache(), json)
                val failure = assertIs<SdkResult.Failure>(repo.cancel("task-1", "cancel-lost-0001"))
                assertEquals(MediaFlowError.NetworkKind.Closed, assertIs<MediaFlowError.Network>(failure.error).kind)
            } finally {
                lost.close()
            }
            val cancelled = HttpClient(MockEngine { throw CancellationException("cancel request") })
            try {
                val repo =
                    TaskRepository(DefaultApi(baseUrl = "https://nas.local", httpClient = cancelled), cancelled, SessionAuthCache(), json)
                assertFailsWith<CancellationException> { repo.cancel("task-1", "cancel-interrupted-0001") }
            } finally {
                cancelled.close()
            }
        }

    @Test
    fun playbackUrlOnlyWhenConfiguredAndHealthy() =
        runTest {
            fun jellyfinEngine(
                health: String,
                configured: Boolean,
            ) = MockEngine {
                respond(
                    content =
                        """{"kind":"jellyfin","configured":$configured,"base_url":"https://jf.local",""" +
                            """"display_name":"Home","server_version":"10.9","config_version":1,""" +
                            """"health":"$health","failure_code":null,"checked_at":"2026-07-17T00:00:00Z"}""",
                    status = HttpStatusCode.OK,
                    headers = okJsonHeaders,
                )
            }
            val healthy = PlaybackRepository(apiOf(jellyfinEngine("healthy", true)), json)
            assertEquals("https://jf.local", healthy.externalPlaybackUrl().getOrNull())
            val degraded = PlaybackRepository(apiOf(jellyfinEngine("degraded", true)), json)
            assertNull(degraded.externalPlaybackUrl().getOrNull())
            val unconfigured = PlaybackRepository(apiOf(jellyfinEngine("unconfigured", false)), json)
            assertNull(unconfigured.externalPlaybackUrl().getOrNull())
        }

    @Test
    fun mediaWallPagerDecodesContractPage() =
        runTest {
            val engine =
                MockEngine {
                    respond(
                        content =
                            """{"items":[{"id":"019f0000-0000-7000-8000-000000000011","type":"movie",""" +
                                """"library_id":"019f0000-0000-7000-8000-000000000010","title":"Dune",""" +
                                """"year":2021,"local_status":"complete","artwork_ref":null,""" +
                                """"updated_at":"2026-07-17T00:00:00Z"}],"next_cursor":null}""",
                        status = HttpStatusCode.OK,
                        headers = okJsonHeaders,
                    )
                }
            val repo =
                mediaflow.client.core.media
                    .MediaRepository(apiOf(engine), json)
            val pager = repo.mediaWall()
            assertTrue(pager.reload())
            assertEquals(1, pager.items.value.size)
            assertEquals(
                "Dune",
                pager.items.value
                    .first()
                    .title,
            )
        }
}
