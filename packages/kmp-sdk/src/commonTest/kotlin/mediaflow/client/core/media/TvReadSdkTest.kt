package mediaflow.client.core.media

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.http.HttpStatusCode
import io.ktor.http.content.TextContent
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mediaflow.api.apis.DefaultApi
import mediaflow.client.core.SdkResult
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertNull

class TvReadSdkTest {
    private val json = MediaFlowApiFactory.defaultJson()
    private val jsonHeaders = headersOf("Content-Type", "application/json")

    @Test
    fun pairingRequestsReadOnlyAndPersistsOnlyTheConfirmedMode() =
        runTest {
            val store = InMemoryDeviceAuthStore()
            val cache = SessionAuthCache()
            val client =
                HttpClient(
                    MockEngine { request ->
                        val body = json.parseToJsonElement((request.body as TextContent).text).jsonObject
                        assertEquals("media-read-only", body["access_mode"]?.jsonPrimitive?.content)
                        respond(enrollment("media-read-only"), HttpStatusCode.Created, jsonHeaders)
                    },
                )
            try {
                val auth = AuthRepository(client, store, cache, json)
                assertIs<SdkResult.Success<StoredCredentials>>(auth.enrollReadOnlyWithPairingCode("12345678", "Living room"))
                assertIs<StoredCredentials.DeviceToken>(store.read())
            } finally {
                client.close()
            }
            val incompatible = HttpClient(MockEngine { respond(enrollment("administrator"), HttpStatusCode.Created, jsonHeaders) })
            val emptyStore = InMemoryDeviceAuthStore()
            val emptyCache = SessionAuthCache()
            try {
                val auth = AuthRepository(incompatible, emptyStore, emptyCache, json)
                assertIs<SdkResult.Failure>(auth.enrollReadOnlyWithPairingCode("12345678", "Living room"))
                assertNull(emptyStore.read())
                assertNull(emptyCache.current())
            } finally {
                incompatible.close()
            }
        }

    @Test
    fun restoringRejectsAnElevatedOrRevokedDeviceAndClearsItsLocalCredential() =
        runTest {
            for (mode in listOf("administrator", "revoked", "media-read-only")) {
                val store = InMemoryDeviceAuthStore()
                val credential = StoredCredentials.DeviceToken("fixture-device-token", DEVICE_ID)
                store.save(credential)
                val cache = SessionAuthCache().also { it.update(credential) }
                val client =
                    HttpClient(
                        MockEngine { request ->
                            assertEquals("/api/v1/devices/$DEVICE_ID", request.url.encodedPath)
                            if (mode == "revoked") {
                                respond("{}", HttpStatusCode.Unauthorized, jsonHeaders)
                            } else {
                                respond(device(mode), HttpStatusCode.OK, jsonHeaders)
                            }
                        },
                    )
                try {
                    val auth = AuthRepository(client, store, cache, json)
                    val result = auth.validateReadOnlyDevice()
                    if (mode == "media-read-only") {
                        assertIs<SdkResult.Success<Unit>>(result)
                        assertEquals(credential, store.read())
                    } else {
                        assertIs<SdkResult.Failure>(result)
                        assertNull(store.read())
                        assertNull(cache.current())
                    }
                } finally {
                    client.close()
                }
            }
        }

    @Test
    fun artworkUsesOpaqueMediaIdsAndBoundsBodyAndContentType() =
        runTest {
            val image = byteArrayOf(0xff.toByte(), 0xd8.toByte(), 0xff.toByte(), 0x01)
            val client =
                HttpClient(
                    MockEngine { request ->
                        assertEquals("/api/v1/media-items/$MEDIA_ID/artwork", request.url.encodedPath)
                        respond(image, HttpStatusCode.OK, headersOf("Content-Type", "image/jpeg"))
                    },
                )
            try {
                val art = ArtworkRepository(client, json)
                assertContentEquals(image, assertIs<SdkResult.Success<Artwork>>(art.poster(MEDIA_ID)).value.bytes)
                assertIs<SdkResult.Failure>(art.poster("../devices"))
            } finally {
                client.close()
            }
            for ((bytes, mime) in listOf(ByteArray(MAX_IMAGE_BYTES + 1) to "image/jpeg", "<html>".encodeToByteArray() to "text/html")) {
                val invalid = HttpClient(MockEngine { respond(bytes, HttpStatusCode.OK, headersOf("Content-Type", mime)) })
                try {
                    assertIs<SdkResult.Failure>(ArtworkRepository(invalid, json).poster(MEDIA_ID))
                } finally {
                    invalid.close()
                }
            }
        }

    @Test
    fun safeLibraryOptionsDecodeAndArtworkPreservesHttpAndCancellationFailures() =
        runTest {
            val client =
                HttpClient(
                    MockEngine {
                        respond("""{"items":[{"id":"$MEDIA_ID","name":"家庭电影"}]}""", HttpStatusCode.OK, jsonHeaders)
                    },
                ) { install(ContentNegotiation) { json(this@TvReadSdkTest.json) } }
            try {
                val api = DefaultApi(baseUrl = "https://nas.local", httpClient = client)
                val libraries =
                    assertIs<SdkResult.Success<List<mediaflow.api.models.MediaLibrary>>>(
                        MediaRepository(api, json).libraries(),
                    ).value
                assertEquals("家庭电影", libraries.single().name)
            } finally {
                client.close()
            }
            val denied = HttpClient(MockEngine { respond("{}", HttpStatusCode.Forbidden, jsonHeaders) })
            try {
                val result = assertIs<SdkResult.Failure>(ArtworkRepository(denied, json).poster(MEDIA_ID))
                assertEquals(403, assertIs<MediaFlowError.Http>(result.error).status)
            } finally {
                denied.close()
            }
            val cancelled = HttpClient(MockEngine { throw CancellationException("controlled cancellation") })
            try {
                assertFailsWith<CancellationException> { ArtworkRepository(cancelled, json).poster(MEDIA_ID) }
            } finally {
                cancelled.close()
            }
        }

    @Test
    fun failedSecurePersistenceDoesNotPublishADeviceCredential() =
        runTest {
            val cache = SessionAuthCache()
            val store =
                object : mediaflow.client.core.auth.DeviceAuthStore {
                    override suspend fun read(): StoredCredentials? = null

                    override suspend fun save(credentials: StoredCredentials) {
                        error("controlled secure-store failure")
                    }

                    override suspend fun clear() = Unit
                }
            val client = HttpClient(MockEngine { respond(enrollment("media-read-only"), HttpStatusCode.Created, jsonHeaders) })
            try {
                val auth = AuthRepository(client, store, cache, json)
                assertIs<SdkResult.Failure>(auth.enrollReadOnlyWithPairingCode("12345678", "Living room"))
                assertNull(cache.current())
            } finally {
                client.close()
            }
        }

    private fun device(mode: String) =
        """
        {
          "id":"$DEVICE_ID",
          "display_name":"Living room",
          "created_at":"2026-10-28T00:00:00Z",
          "last_used_at":null,
          "expires_at":"2027-10-28T00:00:00Z",
          "revoked_at":null,
          "access_mode":"$mode"
        }
        """.trimIndent()

    private fun enrollment(mode: String) = """{"token":"fixture-device-token","device":${device(mode)}}"""

    private companion object {
        const val DEVICE_ID = "019f0000-0000-7000-8000-000000000051"
        const val MEDIA_ID = "019f0000-0000-7000-8000-000000000061"
        const val MAX_IMAGE_BYTES = 2 * 1024 * 1024
    }
}
