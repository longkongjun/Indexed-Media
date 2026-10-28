package mediaflow.client.core.tasks

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.client.request.HttpRequestData
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlinx.io.IOException
import mediaflow.api.apis.DefaultApi
import mediaflow.client.core.SdkResult
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

class TaskRepositoryFeatureTest {
    private val json = MediaFlowApiFactory.defaultJson()
    private val headers = headersOf("Content-Type", "application/json")

    @Test
    fun filteredPagesAndDetailKeepTheirRequestContext() =
        runTest {
            val requests = mutableListOf<HttpRequestData>()
            client(
                MockEngine { request ->
                    requests += request
                    val body =
                        if (request.url.encodedPath.endsWith("/processing-tasks")) {
                            if (request.url.parameters["cursor"] == null) page("task-first", "next-page") else page("task-second", null)
                        } else {
                            task("task-first")
                        }
                    respond(body, HttpStatusCode.OK, headers)
                },
            ).use { transport ->
                val repository = repository(transport)
                val pager = repository.tasks(limit = 1, filter = TaskFilter("all", "planning", "failed", "  沙丘  "))
                assertTrue(pager.reload())
                assertTrue(pager.hasMore())
                assertTrue(pager.loadNext())
                assertFalse(pager.hasMore())
                assertEquals(listOf("task-first", "task-second"), pager.items.value.map { it.id })
                assertIs<SdkResult.Success<*>>(repository.detail("task-first"))
                for (request in requests.take(2)) {
                    assertEquals("all", request.url.parameters["view"])
                    assertEquals("planning", request.url.parameters["stage"])
                    assertEquals("failed", request.url.parameters["status"])
                    assertEquals("沙丘", request.url.parameters["q"])
                    assertEquals("1", request.url.parameters["limit"])
                }
                assertNull(requests.first().url.parameters["cursor"])
                assertEquals("next-page", requests[1].url.parameters["cursor"])
                assertEquals("/api/v1/processing-tasks/task-first", requests.last().url.encodedPath)
            }
        }

    @Test
    fun retryUsesAcceptedStatusIdempotencyAndCookieProof() =
        runTest {
            val credentialsToTest =
                listOf(StoredCredentials.SessionCookie("cookie", "csrf-proof"), StoredCredentials.DeviceToken("token", "device"))
            for (credentials in credentialsToTest) {
                var captured: HttpRequestData? = null
                client(
                    MockEngine { request ->
                        captured = request
                        respond(task("task-first"), HttpStatusCode.Accepted, headers)
                    },
                ).use { transport ->
                    val cache = SessionAuthCache().apply { update(credentials) }
                    assertIs<SdkResult.Success<*>>(repository(transport, cache).retry("task-first", "same-attempt-key"))
                    assertEquals("/api/v1/processing-tasks/task-first/attempts", captured?.url?.encodedPath)
                    assertEquals("POST", captured?.method?.value)
                    assertEquals("same-attempt-key", captured?.headers?.get("Idempotency-Key"))
                    val expectedProof = if (credentials is StoredCredentials.SessionCookie) "csrf-proof" else null
                    assertEquals(expectedProof, captured?.headers?.get("X-CSRF-Token"))
                }
            }
        }

    @Test
    fun retryRetainsHttpFailuresAndPropagatesTransportCancellation() =
        runTest {
            val statuses =
                listOf(HttpStatusCode.Unauthorized, HttpStatusCode.Forbidden, HttpStatusCode.Conflict, HttpStatusCode.ServiceUnavailable)
            for (status in statuses) {
                client(
                    MockEngine {
                        respond(
                            """{"error":{"code":"internal.error","message":"retry rejected","request_id":"retry-fixture"}}""",
                            status,
                            headers,
                        )
                    },
                ).use { transport ->
                    val failure = assertIs<SdkResult.Failure>(repository(transport).retry("task-first", "same-attempt-key"))
                    val error = assertIs<MediaFlowError.Http>(failure.error)
                    assertEquals(status.value, error.status)
                    assertEquals("retry-fixture", error.requestId)
                }
            }
            client(MockEngine { throw IOException("response lost") }).use { transport ->
                val failure = assertIs<SdkResult.Failure>(repository(transport).retry("task-first", "same-attempt-key"))
                assertIs<MediaFlowError.Network>(failure.error)
            }
            client(MockEngine { throw CancellationException("cancelled retry") }).use { transport ->
                assertFailsWith<CancellationException> { repository(transport).retry("task-first", "same-attempt-key") }
            }
        }

    @Test
    fun taskFiltersRejectUnknownEnumsAndOversizedQueries() {
        assertFailsWith<IllegalArgumentException> { TaskFilter(view = "administration") }
        assertFailsWith<IllegalArgumentException> { TaskFilter(stage = "unknown") }
        assertFailsWith<IllegalArgumentException> { TaskFilter(status = "unknown") }
        assertFailsWith<IllegalArgumentException> { TaskFilter(query = "x".repeat(201)) }
    }

    private fun client(engine: MockEngine): HttpClient =
        HttpClient(engine) {
            expectSuccess = false
            install(ContentNegotiation) { json(json) }
        }

    private fun repository(
        client: HttpClient,
        cache: SessionAuthCache = SessionAuthCache(),
    ) = TaskRepository(DefaultApi("https://nas.local", client), client, cache, json)

    private fun page(
        id: String,
        cursor: String?,
    ): String {
        val next = cursor?.let { "\"$it\"" } ?: "null"
        return """
            {"items":[${task(id)}],"next_cursor":$next,
             "summary":{"pending":0,"running":0,"all":2,"completed":0,"snapshot_version":1}}
            """.trimIndent()
    }

    private fun task(id: String): String =
        """
        {"id":"$id","inbox_directory_id":"inbox","file_revision_id":"revision","relative_path":"电影/沙丘.mkv",
         "status":"failed","stage":"planning","checkpoint":"planning-paused","decision_checkpoint":null,
         "current_task_decision_id":null,"reason":"organization.plan-paused","recovering":false,"attempt_count":2,
         "next_retry_at":null,"allowed_actions":["retry","cancel"],"updated_at":"2026-10-28T08:00:00Z"}
        """.trimIndent()
}
