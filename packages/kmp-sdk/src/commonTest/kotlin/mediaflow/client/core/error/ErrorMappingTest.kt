package mediaflow.client.core.error

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.client.request.get
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import kotlinx.io.IOException
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertTrue

/** 模拟 JVM/原生侧的 DNS 解析失败（无 common UnknownHostException，映射靠类型名识别）。 */
private class UnknownHostException(
    message: String,
) : IOException(message)

class ErrorMappingTest {
    private val json = Json { ignoreUnknownKeys = true }

    @Test
    fun unknownHostMapsToOfflineDnsError() {
        val error = ErrorMapping.fromThrowable(UnknownHostException("nas.local"))
        val network = assertIs<MediaFlowError.Network>(error)
        assertEquals(MediaFlowError.NetworkKind.Dns, network.kind)
        assertTrue(error.isOffline())
    }

    @Test
    fun ioFailureMapsToClosedAndIsNotOffline() {
        val error = ErrorMapping.fromThrowable(IOException("connection reset"))
        val network = assertIs<MediaFlowError.Network>(error)
        assertEquals(MediaFlowError.NetworkKind.Closed, network.kind)
        assertFalse(error.isOffline())
    }

    @Test
    fun serializationFailureMapsToSerializationError() {
        val error = ErrorMapping.fromThrowable(SerializationException("missing field"))
        assertIs<MediaFlowError.Serialization>(error)
    }

    @Test
    fun unexpectedThrowableIsContained() {
        val error = ErrorMapping.fromThrowable(IllegalStateException("boom"))
        assertIs<MediaFlowError.Unexpected>(error)
    }

    @Test
    fun httpErrorEnvelopeIsDecodedWithRequestId() =
        kotlinx.coroutines.test.runTest {
            val client =
                HttpClient(
                    MockEngine {
                        respond(
                            content =
                                """{"error":{"code":"validation.failed","message":"limit out of range",""" +
                                    """"request_id":"req-42"}}""",
                            status = HttpStatusCode.UnprocessableEntity,
                            headers = headersOf("Content-Type", "application/json"),
                        )
                    },
                )
            val response = client.get("/api/v1/media-items")
            val error = ErrorMapping.fromResponse(response, json)
            val http = assertIs<MediaFlowError.Http>(error)
            assertEquals(422, http.status)
            assertEquals("validation.failed", http.code)
            assertEquals("limit out of range", http.message)
            assertEquals("req-42", http.requestId)
            client.close()
        }

    @Test
    fun httpErrorWithoutEnvelopeFallsBackToStatusLine() =
        kotlinx.coroutines.test.runTest {
            val client =
                HttpClient(
                    MockEngine {
                        respond(content = "gateway busy", status = HttpStatusCode.ServiceUnavailable)
                    },
                )
            val response = client.get("/api/v1/media-items")
            val error = ErrorMapping.fromResponse(response, json)
            val http = assertIs<MediaFlowError.Http>(error)
            assertEquals(503, http.status)
            assertTrue(http.message.contains("503"))
            client.close()
        }

    @Test
    fun retryClassificationFollowsContractSemantics() {
        assertTrue(MediaFlowError.Http(429, null, "", null).retryable)
        assertTrue(MediaFlowError.Http(503, null, "", null).retryable)
        assertFalse(MediaFlowError.Http(401, null, "", null).retryable)
        assertFalse(MediaFlowError.Http(409, null, "", null).retryable)
        assertTrue(MediaFlowError.Network(MediaFlowError.NetworkKind.Connect, null).retryable)
        assertFalse(MediaFlowError.Network(MediaFlowError.NetworkKind.Closed, null).retryable)
    }
}
