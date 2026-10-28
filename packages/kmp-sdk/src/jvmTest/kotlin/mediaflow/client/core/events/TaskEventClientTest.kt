package mediaflow.client.core.events

import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.http.ContentType
import io.ktor.http.HttpHeaders
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import io.ktor.utils.io.ByteReadChannel
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import mediaflow.client.core.net.SessionAuthCache
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class TaskEventClientTest {
    private val json = Json { ignoreUnknownKeys = true }

    private val sseHeaders = headersOf(HttpHeaders.ContentType, ContentType.Text.EventStream.toString())

    private fun sseBody(vararg frames: String) = ByteReadChannel(frames.joinToString(""))

    private fun frame(
        id: Long,
        type: String,
        payloadJson: String,
    ): String =
        "id: $id\nevent: $type\ndata: " +
            "{\"id\":$id,\"type\":\"$type\",\"schema_version\":\"1\"," +
            "\"occurred_at\":\"2026-07-17T00:00:00Z\",\"task_id\":null," +
            "\"payload\":$payloadJson}\n\n"

    private fun client(engine: MockEngine) =
        TaskEventClient(
            baseUrl = "https://nas.local",
            authCache = SessionAuthCache(),
            json = json,
            engine = engine,
            initialBackoffMs = 10,
            maxBackoffMs = 50,
        )

    @Test
    fun unauthorizedNotifiesHostWithoutTreatingForbiddenAsExpired() =
        runBlocking {
            withTimeout(10_000) {
                var requests = 0
                var expired = 0
                val engine =
                    MockEngine {
                        requests++
                        when (requests) {
                            1 -> respond("", HttpStatusCode.Unauthorized)
                            2 -> respond("", HttpStatusCode.Forbidden)
                            else -> respond(sseBody(frame(1, "task.progress", "{}")), headers = sseHeaders)
                        }
                    }
                val client =
                    TaskEventClient(
                        baseUrl = "https://nas.local",
                        authCache = SessionAuthCache(),
                        engine = engine,
                        initialBackoffMs = 1,
                        maxBackoffMs = 1,
                        onUnauthorized = { expired++ },
                    )
                try {
                    assertEquals(
                        1,
                        client
                            .events()
                            .take(1)
                            .toList()
                            .size,
                    )
                    assertEquals(1, expired)
                    assertEquals(3, requests)
                } finally {
                    client.close()
                }
            }
        }

    @Test
    fun emitsDecodedEventsAndDeduplicatesByCursor() =
        runBlocking {
            withTimeout(10_000) {
                val engine =
                    MockEngine {
                        respond(
                            content =
                                sseBody(
                                    frame(1, "task.progress", """{"visited_directories":1}"""),
                                    frame(2, "task.progress", """{"visited_directories":2}"""),
                                    // 重连补发窗口内的重复帧：id ≤ 游标，必须被丢弃。
                                    frame(2, "task.progress", """{"visited_directories":2}"""),
                                    frame(
                                        3,
                                        "catalog.media-changed",
                                        """{"media_item_id":"019f0000-0000-7000-8000-00000000000a"}""",
                                    ),
                                ),
                            headers = sseHeaders,
                        )
                    }
                val client = client(engine)
                val events = client.events().take(3).toList()
                assertEquals(listOf(1L, 2L, 3L), events.map { it.id })
                assertEquals(3L, client.cursor.value)
                client.close()
            }
        }

    @Test
    fun reconnectResumesFromLastAcceptedCursor() =
        runBlocking {
            withTimeout(10_000) {
                val seenRequests = mutableListOf<String?>()
                var call = 0
                val engine =
                    MockEngine { request ->
                        seenRequests.add(request.headers["Last-Event-ID"])
                        call += 1
                        if (call == 1) {
                            respond(
                                content = ByteReadChannel(frame(7, "task.state-changed", """{"status":"completed"}""")),
                                headers = headersOf(HttpHeaders.ContentType, ContentType.Text.EventStream.toString()),
                            )
                        } else {
                            respond(
                                content =
                                    ByteReadChannel(
                                        frame(
                                            9,
                                            "review-case.updated",
                                            """{"review_case_id":"019f0000-0000-7000-8000-00000000000b"}""",
                                        ),
                                    ),
                                headers = headersOf(HttpHeaders.ContentType, ContentType.Text.EventStream.toString()),
                            )
                        }
                    }
                val client = client(engine)
                val events = client.events().take(2).toList()
                assertEquals(listOf(7L, 9L), events.map { it.id })
                // 首连无游标头；重连携带 Last-Event-ID=7。
                assertEquals(null, seenRequests[0])
                assertEquals("7", seenRequests[1])
                client.close()
            }
        }

    @Test
    fun streamGapIsPassedThroughAsEnvelope() =
        runBlocking {
            withTimeout(10_000) {
                val engine =
                    MockEngine {
                        respond(
                            content = sseBody(frame(5, "stream.gap", """{"message":"replay unavailable"}""")),
                            headers = sseHeaders,
                        )
                    }
                val client = client(engine)
                val events = client.events().take(1).toList()
                assertEquals(TaskEventEnvelope.TYPE_STREAM_GAP, events.single().type)
                assertEquals(5L, client.cursor.value)
                assertTrue(events.single().taskId == null)
                client.close()
            }
        }

    @Test
    fun malformedFramesAreSkippedWithoutBreakingTheStream() =
        runBlocking {
            withTimeout(10_000) {
                val engine =
                    MockEngine(
                        {
                            respond(
                                content =
                                    ByteReadChannel(
                                        "id: 4\nevent: task.progress\ndata: not-json\n\n" +
                                            frame(6, "task.progress", """{"visited_directories":9}"""),
                                    ),
                                headers = headersOf(HttpHeaders.ContentType, ContentType.Text.EventStream.toString()),
                            )
                        },
                    )
                val client = client(engine)
                val events = client.events().take(1).toList()
                assertEquals(6L, events.single().id)
                client.close()
            }
        }
}
