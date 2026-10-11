package mediaflow.client.core.auth

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * 设备侧会话凭据：cookie 会话通道（第一批/fixture server）或设备令牌通道
 * （第二批，Core 设备 enrollment）。
 *
 * 通道由类型承载——cookie+CSRF 与 Bearer 令牌互不可替换（Core 侧两表分离，
 * 交叉呈现即认证失败），注入与写证明按通道选择。
 */
@Serializable
sealed interface StoredCredentials {
    /** 显示名（登录页欢迎语/设备列表）；不可用时任一通道都可能为空。 */
    val displayName: String?

    /** cookie 会话通道：`__Host-mediaflow_session` 值 + CSRF 令牌。 */
    @Serializable
    @SerialName("session")
    data class SessionCookie(
        val sessionCookieValue: String,
        val csrfToken: String,
        val accountId: String? = null,
        override val displayName: String? = null,
    ) : StoredCredentials

    /** 设备令牌通道：`Authorization: Bearer mfdev_…` 的原始令牌。 */
    @Serializable
    @SerialName("device")
    data class DeviceToken(
        val token: String,
        val deviceId: String? = null,
        override val displayName: String? = null,
    ) : StoredCredentials
}

/**
 * 凭据的安全存储抽象：只定义读写清，不预设存储介质。
 *
 * 平台实现（iOS Keychain / Android Keystore 加密文件）由 SDK 平台源集提供；
 * 测试与预览使用 [InMemoryDeviceAuthStore]。AuthRepository 是唯一写入口。
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
