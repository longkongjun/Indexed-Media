package mediaflow.androidapp

import android.content.Context
import app.cash.sqldelight.driver.android.AndroidSqliteDriver
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.client.cache.MediaFlowCache
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.auth.KeystoreDeviceAuthStore
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
 * base URL 可在登录页配置（fixture server 或局域 NAS）；凭据经 Android Keystore
 * 加密落盘（第二批），任意屏 401 经 [sessionExpired] 统一回登录页。
 */
class SdkContainer(context: Context) {
    val authStore = KeystoreDeviceAuthStore(context)
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

    /** 会话/令牌失效信号（任意屏 401 → 清凭据 → 根路由回登录页）。 */
    val sessionExpired = MutableSharedFlow<Unit>(extraBufferCapacity = 1)

    /** 失败结果的统一 401 处理：凭据失效时清本地并发信号；其余失败原样返回。 */
    suspend fun <T> handleAuthFailure(result: mediaflow.client.core.SdkResult<T>) {
        if (result is mediaflow.client.core.SdkResult.Failure && result.error.isCredentialExpired()) {
            authRepository.discardLocalCredentials()
            sessionExpired.emit(Unit)
        }
    }

    /** 登录前/切换服务器时重建传输层；幂等。 */
    fun configure(baseUrl: String) {
        val normalized = baseUrl.trimEnd('/')
        factory = MediaFlowApiFactory(normalized, authCache, json)
        val client = factory!!.httpClient()
        apiHolder = DefaultApi(baseUrl = normalized, httpClient = client)
        authRepository = AuthRepository(client, authStore, authCache, json)
        mediaRepository = MediaRepository(apiHolder!!, json)
        reviewRepository = ReviewRepository(apiHolder!!, client, authCache, json)
        taskRepository = TaskRepository(apiHolder!!, client, authCache, json)
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
