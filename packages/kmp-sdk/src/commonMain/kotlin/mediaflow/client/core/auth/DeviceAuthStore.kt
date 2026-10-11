package mediaflow.client.core.auth

import kotlinx.serialization.Serializable

/**
 * 设备侧会话凭据。
 *
 * 第一批承载管理员会话（cookie `__Host-mediaflow_session` 值 + CSRF 令牌）；
 * 第二批 Core 设备配对落地后，同一抽象承载设备令牌——存储键与注入通道不变。
 */
@Serializable
data class StoredCredentials(
    val sessionCookieValue: String,
    val csrfToken: String,
    val accountId: String? = null,
    val administratorName: String? = null,
)

/**
 * 凭据的安全存储抽象：只定义读写清，不预设存储介质。
 *
 * 平台实现（iOS Keychain / Android Keystore 加密文件）由宿主 App 提供；
 * SDK 与测试使用 [InMemoryDeviceAuthStore]。真实设备认证流（第二批）之前，
 * 登录 Repository 是唯一写入口。
 */
interface DeviceAuthStore {
    suspend fun read(): StoredCredentials?

    suspend fun save(credentials: StoredCredentials)

    suspend fun clear()
}

/** 内存实现：单测、预览与 fixture server 场景；不做持久化。 */
class InMemoryDeviceAuthStore : DeviceAuthStore {
    private var cached: StoredCredentials? = null

    override suspend fun read(): StoredCredentials? = cached

    override suspend fun save(credentials: StoredCredentials) {
        cached = credentials
    }

    override suspend fun clear() {
        cached = null
    }
}
