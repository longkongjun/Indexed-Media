package mediaflow.client.core.net

import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import kotlin.concurrent.atomics.AtomicReference
import kotlin.concurrent.atomics.ExperimentalAtomicApi

/**
 * 会话凭据的请求期快照缓存。
 *
 * Ktor 请求头注入钩子运行在非挂起上下文，不能直接调用挂起的 [DeviceAuthStore]；
 * 登录/登出流程先写缓存（同步可见）再落盘。缓存是唯一事实源的镜像，启动时由
 * 宿主 App 调用 [load] 从持久存储预热。
 */
@OptIn(ExperimentalAtomicApi::class)
class SessionAuthCache {
    private val cached = AtomicReference<StoredCredentials?>(null)

    /** 当前凭据（可能为 null：未登录或已登出）。 */
    fun current(): StoredCredentials? = cached.load()

    fun update(credentials: StoredCredentials?) {
        cached.store(credentials)
    }

    /** 从持久存储预热；仅启动时调用一次。 */
    suspend fun load(store: DeviceAuthStore) {
        cached.store(store.read())
    }
}
