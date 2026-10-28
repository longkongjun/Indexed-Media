package mediaflow.desktop

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.shareIn
import kotlinx.coroutines.withContext
import mediaflow.client.cache.MediaFlowCache
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.cache.MediaCache
import mediaflow.client.core.events.TaskEventClient
import mediaflow.client.core.events.TaskEventEnvelope
import mediaflow.client.core.media.MediaRepository
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.net.platformHttpClientEngine
import mediaflow.client.core.playback.PlaybackRepository
import mediaflow.client.core.review.ReviewRepository
import mediaflow.client.core.tasks.TaskRepository
import java.nio.file.Files
import java.nio.file.Path

internal class DesktopSession(
    val server: String,
    store: DeviceAuthStore,
) {
    val io = Dispatchers.IO.limitedParallelism(1)
    private val scope = CoroutineScope(SupervisorJob() + io)
    val authCache = SessionAuthCache()
    private val factory = MediaFlowApiFactory(server, authCache)
    private val transport = factory.httpClient()
    private val json = MediaFlowApiFactory.defaultJson()
    private val api = factory.defaultApi(transport)
    val auth = AuthRepository(transport, store, authCache, json)
    val media = MediaRepository(api, json)
    val review = ReviewRepository(api, transport, authCache, json)
    val tasks = TaskRepository(api, transport, authCache, json)
    val playback = PlaybackRepository(api, json)
    private val credentialExpired = MutableStateFlow(false)
    val authExpired = credentialExpired.asStateFlow()
    private val eventClient =
        TaskEventClient(server, authCache, json, platformHttpClientEngine(), onUnauthorized = {
            credentialExpired.value = true
        })
    private var driver: JdbcSqliteDriver? = null
    lateinit var cache: MediaCache
        private set
    private var activated = false
    private lateinit var sharedEvents: SharedFlow<TaskEventEnvelope>

    suspend fun restore(store: DeviceAuthStore): Boolean {
        authCache.load(store)
        return authCache.current() is StoredCredentials.DeviceToken
    }

    suspend fun activate(directory: Path) =
        withContext(io) {
            check(!activated)
            val device = authCache.current() as? StoredCredentials.DeviceToken ?: error("设备凭据缺失")
            val id = device.deviceId ?: error("设备身份缺失")
            Files.createDirectories(directory)
            val file = directory.resolve("${cacheIdentity(server, id)}.db")
            val exists = Files.exists(file)
            val created = JdbcSqliteDriver("jdbc:sqlite:$file")
            driver = created
            try {
                if (!exists) MediaFlowCache.Schema.create(created)
                cache = MediaCache(created)
                sharedEvents = sharedSessionEvents(eventClient.events(cache.eventCursor()), scope) { cache.storeEventCursor(it.id) }
                activated = true
            } catch (failure: Exception) {
                created.close()
                driver = null
                if (!exists) Files.deleteIfExists(file)
                throw failure
            }
        }

    fun events(): SharedFlow<TaskEventEnvelope> {
        check(activated)
        return sharedEvents
    }

    suspend fun close() =
        withContext(NonCancellable + io) {
            scope.cancel()
            eventClient.close()
            transport.close()
            driver?.close()
            driver = null
            authCache.update(null)
        }
}

internal fun <T> sharedSessionEvents(
    source: Flow<T>,
    scope: CoroutineScope,
    accept: suspend (T) -> Unit,
): SharedFlow<T> = source.onEach(accept).shareIn(scope, SharingStarted.Eagerly, replay = 0)
