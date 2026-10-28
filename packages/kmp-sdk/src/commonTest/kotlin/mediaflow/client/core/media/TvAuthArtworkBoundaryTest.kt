package mediaflow.client.core.media

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.http.HeadersBuilder
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import io.ktor.utils.io.ByteChannel
import io.ktor.utils.io.writeFully
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest
import kotlinx.io.IOException
import mediaflow.client.core.SdkResult
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

class TvAuthArtworkBoundaryTest {
    private val json = MediaFlowApiFactory.defaultJson()
    private val jsonHeaders = headersOf("Content-Type", "application/json")
    private val credential = StoredCredentials.DeviceToken("boundary-fixture-token", DEVICE_ID)

    @Test
    fun cancellingLogoutClearsBothMemoryAndTheDurableCredential() =
        runTest {
            val store = InMemoryDeviceAuthStore().also { it.save(credential) }
            val cache = SessionAuthCache().also { it.update(credential) }
            val entered = CompletableDeferred<Unit>()
            val client =
                HttpClient(
                    MockEngine {
                        entered.complete(Unit)
                        awaitCancellation()
                    },
                )
            try {
                val auth = AuthRepository(client, store, cache, json)
                val request = async { auth.logout() }
                entered.await()
                request.cancelAndJoin()
                assertNull(cache.current())
                assertNull(store.read(), "an interrupted logout must not restore the device on next launch")
            } finally {
                client.close()
            }
        }

    @Test
    fun logoutKeepsTheOriginalCancellationWhenSecureCleanupAlsoFails() =
        runTest {
            val cancelled = CancellationException("logout cancellation sentinel")
            var clearAttempts = 0
            val store =
                object : DeviceAuthStore {
                    override suspend fun read(): StoredCredentials = credential

                    override suspend fun save(credentials: StoredCredentials) = Unit

                    override suspend fun clear() {
                        clearAttempts += 1
                        error("secure cleanup sentinel")
                    }
                }
            val cache = SessionAuthCache().also { it.update(credential) }
            val client = HttpClient(MockEngine { throw cancelled })
            try {
                val auth = AuthRepository(client, store, cache, json)
                val observed = assertFailsWith<CancellationException> { auth.logout() }
                assertEquals(cancelled.message, observed.message)
                assertEquals(1, clearAttempts)
                assertNull(cache.current())
            } finally {
                client.close()
            }
        }

    @Test
    fun successfulRevocationDoesNotReportSuccessWhenSecureCleanupFails() =
        runTest {
            val store =
                object : DeviceAuthStore {
                    override suspend fun read(): StoredCredentials = credential

                    override suspend fun save(credentials: StoredCredentials) = Unit

                    override suspend fun clear() {
                        error("secure cleanup sentinel")
                    }
                }
            val cache = SessionAuthCache().also { it.update(credential) }
            val client = HttpClient(MockEngine { respond("", HttpStatusCode.NoContent) })
            try {
                assertIs<SdkResult.Failure>(AuthRepository(client, store, cache, json).logout())
                assertNull(cache.current())
            } finally {
                client.close()
            }
        }

    @Test
    fun artworkDoesNotDrainFiveMiBStreamsOnAnyEarlyRejection() =
        runTest {
            val consumed = assertEarlyRejection(this, HttpStatusCode.ServiceUnavailable, "text/html")
            assertTrue(consumed <= MAX_IMAGE_BYTES + BUFFER_BYTES, "read $consumed bytes of a proxy error body")
            assertEarlyRejection(this, HttpStatusCode.OK, "text/html")
            assertEarlyRejection(this, HttpStatusCode.OK, "image/jpeg", ERROR_BODY_BYTES.toString())
            assertEarlyRejection(this, HttpStatusCode.OK, "image/jpeg", "0")
            assertEarlyRejection(this, HttpStatusCode.OK, "image/jpeg")
        }

    @Suppress("TooGenericExceptionCaught") // Fixture producer observes expected response disposal; other failures still propagate.
    private suspend fun assertEarlyRejection(
        scope: CoroutineScope,
        status: HttpStatusCode,
        mime: String,
        contentLength: String? = null,
    ): Int {
        val body = ByteChannel()
        var producedBytes = 0
        val producedAll = CompletableDeferred<Unit>()
        val producer =
            scope.launch {
                try {
                    val chunk = ByteArray(BUFFER_BYTES) { 'x'.code.toByte() }
                    repeat(ERROR_BODY_BYTES / BUFFER_BYTES) {
                        body.writeFully(chunk)
                        producedBytes += chunk.size
                        body.flush()
                    }
                    producedAll.complete(Unit)
                    body.close()
                } catch (failure: Exception) {
                    if (body.closedCause == null) throw failure
                }
            }
        val headers =
            HeadersBuilder()
                .apply {
                    append("Content-Type", mime)
                    contentLength?.let { append("Content-Length", it) }
                }.build()
        val client = HttpClient(MockEngine { respond(body, status, headers) })
        try {
            val result = assertIs<SdkResult.Failure>(ArtworkRepository(client, json).poster(MEDIA_ID))
            assertTrue(body.closedCause != null, "early rejection must cancel the exposed response channel")
            producer.join()
            if (status == HttpStatusCode.ServiceUnavailable) assertEquals(503, assertIs<MediaFlowError.Http>(result.error).status)
            assertTrue(!producedAll.isCompleted, "the response drained the entire five MiB stream for $status/$mime/$contentLength")
            return producedBytes
        } finally {
            producer.cancelAndJoin()
            client.close()
        }
    }

    @Test
    fun cancellationWhileReadingAnArtworkErrorBodyIsNeverMappedToHttpFailure() =
        runTest {
            val cancelled = CancellationException("artwork error body cancellation sentinel")
            val body = ByteChannel().also { it.cancel(cancelled) }
            val client = HttpClient(MockEngine { respond(body, HttpStatusCode.ServiceUnavailable, jsonHeaders) })
            try {
                val observed = assertFailsWith<CancellationException> { ArtworkRepository(client, json).poster(MEDIA_ID) }
                assertEquals(cancelled.message, observed.message)
            } finally {
                client.close()
            }
        }

    @Test
    fun boundedArtworkErrorsPreserveHttpStatusAndTheCoreEnvelope() =
        runTest {
            val client =
                HttpClient(
                    MockEngine {
                        respond(
                            """{"error":{"code":"session.expired","message":"device expired","request_id":"boundary-request"}}""",
                            HttpStatusCode.Unauthorized,
                            jsonHeaders,
                        )
                    },
                )
            try {
                val result = assertIs<SdkResult.Failure>(ArtworkRepository(client, json).poster(MEDIA_ID))
                val error = assertIs<MediaFlowError.Http>(result.error)
                assertEquals(401, error.status)
                assertEquals("session.expired", error.code)
                assertEquals("boundary-request", error.requestId)
            } finally {
                client.close()
            }
            val failedBody = ByteChannel().also { it.cancel(IOException("truncated error body sentinel")) }
            val unreadable = HttpClient(MockEngine { respond(failedBody, HttpStatusCode.Unauthorized, jsonHeaders) })
            try {
                val result = assertIs<SdkResult.Failure>(ArtworkRepository(unreadable, json).poster(MEDIA_ID))
                assertEquals(401, assertIs<MediaFlowError.Http>(result.error).status)
            } finally {
                unreadable.close()
            }
        }

    private companion object {
        const val DEVICE_ID = "019f0000-0000-7000-8000-000000000071"
        const val MEDIA_ID = "019f0000-0000-7000-8000-000000000072"
        const val ERROR_BODY_BYTES = 5 * 1024 * 1024
        const val MAX_IMAGE_BYTES = 2L * 1024 * 1024
        const val BUFFER_BYTES = 8192
    }
}
