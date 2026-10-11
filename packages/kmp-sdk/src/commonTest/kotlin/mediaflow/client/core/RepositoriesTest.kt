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
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mediaflow.api.apis.DefaultApi
import mediaflow.api.models.SelectProviderCandidateCommand
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.playback.PlaybackRepository
import mediaflow.client.core.review.ReviewRepository
import mediaflow.client.core.tasks.TaskRepository
import kotlin.test.Test
import kotlin.test.assertEquals
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
            val credentials = assertIs<SdkResult.Success<StoredCredentials>>(result).value
            assertEquals("abc123", credentials.sessionCookieValue)
            assertEquals("tok-tok-tok-tok-tok-tok-tok-tok-tok-tok-tok-1", credentials.csrfToken)
            assertEquals("abc123", cache.current()?.sessionCookieValue)
            assertEquals("admin", store.read()?.administratorName)
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
            cache.update(StoredCredentials("cookie", "csrf-token-1", null, null))
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
                        status = HttpStatusCode.OK,
                        headers = okJsonHeaders,
                    )
                }
            val (_, cache) = authStack()
            cache.update(StoredCredentials("cookie", "csrf-token-2", null, null))
            val repo = TaskRepository(apiOf(engine), cache, json)
            val result = repo.cancel("task-1", idempotencyKey = "cancel-key-0001")
            val task = assertIs<SdkResult.Success<mediaflow.api.models.ProcessingTask>>(result).value
            assertEquals("waiting-confirmation", task.status.toString())
            assertEquals("cancel-key-0001", captured!!.headers["Idempotency-Key"])
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
