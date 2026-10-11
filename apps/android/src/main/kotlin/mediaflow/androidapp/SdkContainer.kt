package mediaflow.androidapp

import android.content.Context
import app.cash.sqldelight.driver.android.AndroidSqliteDriver
import java.io.File
import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.client.cache.MediaFlowCache
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.cache.MediaCache
import mediaflow.client.core.events.TaskEventClient
import mediaflow.client.core.media.MediaRepository
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.net.platformHttpClientEngine
import mediaflow.client.core.playback.PlaybackRepository
import mediaflow.client.core.review.ReviewRepository
import mediaflow.client.core.tasks.TaskRepository

/**
 * App 级 SDK 组装点：仓库/传输/SSE/缓存的唯一实例来源。
 * base URL 可在登录页配置（fixture server 或局域 NAS）。
 */
class SdkContainer(context: Context) {
    val authStore: DeviceAuthStore = FileDeviceAuthStore(context.filesDir.resolve("session.json"))
    val authCache: SessionAuthCache = SessionAuthCache()
    val json: Json = MediaFlowApiFactory.defaultJson()

    private var factory: MediaFlowApiFactory? = null
    private var apiHolder: DefaultApi? = null
    private var eventClient: TaskEventClient? = null
    private val cache = MediaCache(AndroidSqliteDriver(MediaFlowCache.Schema, context, "mediaflow-cache.db"))

    lateinit var authRepository: AuthRepository
        private set
    lateinit var mediaRepository: MediaRepository
        private set
    lateinit var reviewRepository: ReviewRepository
        private set
    lateinit var taskRepository: TaskRepository
        private set
    lateinit var playbackRepository: PlaybackRepository
        private set

    val mediaCache: MediaCache get() = cache

    /** 登录前/切换服务器时重建传输层；幂等。 */
    fun configure(baseUrl: String) {
        val normalized = baseUrl.trimEnd('/')
        factory = MediaFlowApiFactory(normalized, authCache, json)
        val client = factory!!.httpClient()
        apiHolder = DefaultApi(baseUrl = normalized, httpClient = client)
        authRepository = AuthRepository(client, authStore, authCache, json)
        mediaRepository = MediaRepository(apiHolder!!, json)
        reviewRepository = ReviewRepository(apiHolder!!, client, authCache, json)
        taskRepository = TaskRepository(apiHolder!!, authCache, json)
        playbackRepository = PlaybackRepository(apiHolder!!, json)
        eventClient =
            TaskEventClient(
                baseUrl = normalized,
                authCache = authCache,
                json = json,
                engine = platformHttpClientEngine(),
            )
    }

    fun eventClientOrNull(): TaskEventClient? = eventClient

    suspend fun warmUp() {
        authCache.load(authStore)
    }
}

/** 文件落地凭据存储：第一批直存 JSON（本地文件私有目录）；Keystore 加密随第二批设备认证接入。 */
class FileDeviceAuthStore(private val file: File) : DeviceAuthStore {
    private val json = Json { ignoreUnknownKeys = true }

    override suspend fun read(): StoredCredentials? =
        runCatching { file.takeIf { it.exists() }?.readText()?.let { json.decodeFromString(StoredCredentials.serializer(), it) } }.getOrNull()

    override suspend fun save(credentials: StoredCredentials) {
        file.parentFile?.mkdirs()
        file.writeText(json.encodeToString(StoredCredentials.serializer(), credentials))
    }

    override suspend fun clear() {
        file.delete()
    }
}
