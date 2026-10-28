package mediaflow.tv

import io.ktor.client.HttpClient
import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.media.ArtworkRepository
import mediaflow.client.core.media.MediaRepository
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.playback.PlaybackRepository

/** TV会话只装配媒体读取能力；HTTP由会话关闭，不创建任务/管理SSE。 */
internal class TvSession(
    server: String,
    private val store: DeviceAuthStore,
    val credentials: SessionAuthCache,
    private val client: HttpClient,
    json: Json,
) : AutoCloseable {
    private val api = DefaultApi(baseUrl = server, httpClient = client)
    val auth = AuthRepository(client, store, credentials, json)
    val media = MediaRepository(api, json)
    val artwork = ArtworkRepository(client, json)
    val playback = PlaybackRepository(api, json)

    suspend fun restore(): Boolean {
        credentials.load(store)
        return credentials.current() != null
    }

    override fun close() {
        credentials.update(null)
        client.close()
    }
}
