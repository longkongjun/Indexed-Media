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
class SdkContainer(
    context: Context,
) {
    private val prefs = context.getSharedPreferences("mediaflow-container", Context.MODE_PRIVATE)
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

    /** 分页状态流的 401 转发：墙/确认/任务三屏的 PageState.Error 同样触发统一登出。 */
    suspend fun propagateAuthFailure(collected: mediaflow.client.core.paging.PageState) {
        val error = (collected as? mediaflow.client.core.paging.PageState.Error)?.error ?: return
        handleAuthFailure(
            mediaflow.client.core.SdkResult
                .failure(error),
        )
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

    /** 登录成功后记住服务器地址，供冷启动恢复。 */
    fun rememberBaseUrl(baseUrl: String) {
        prefs.edit().putString("base-url", baseUrl.trimEnd('/')).apply()
    }

    /** 上次成功登录的地址（401 登出回登录页时预填，避免退回编译期默认值）。 */
    fun lastKnownBaseUrl(): String? = prefs.getString("base-url", null)

    /**
     * 冷启动恢复：Keystore 有凭据且上次登录地址在档则直接接线进入
     * （不做网络验证，首个请求自然校验；401 统一回登录页）。
     */
    suspend fun restoreSession(): Boolean {
        val saved = prefs.getString("base-url", null)
        if (saved != null) {
            configure(saved)
        }
        authCache.load(authStore)
        return saved != null && authCache.current() != null
    }
}
