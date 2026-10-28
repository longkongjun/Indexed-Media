package mediaflow.client.core.auth

import io.ktor.client.HttpClient
import io.ktor.client.request.delete
import io.ktor.client.request.get
import io.ktor.client.request.header
import io.ktor.client.request.post
import io.ktor.client.request.setBody
import io.ktor.client.statement.bodyAsText
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.contentType
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import mediaflow.api.models.DeviceAccessMode
import mediaflow.api.models.DeviceEnrollment
import mediaflow.api.models.DeviceEnrollmentInput
import mediaflow.api.models.DeviceTokenRotation
import mediaflow.api.models.DeviceView
import mediaflow.api.models.LoginRequest
import mediaflow.api.models.SessionResponse
import mediaflow.client.core.SdkResult
import mediaflow.client.core.error.ErrorMapping
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache

/**
 * 登录/配对/登出/会话恢复：凭据唯一写入口。
 *
 * 三条进入路径：管理员密码登录（cookie 会话通道，fixture server/调试）、一次性
 * 配对码 enrollment 与凭据 enrollment（设备令牌通道，移动端主路径）。登录与
 * enrollment 走直连请求（不经生成 API）：需要读取 `Set-Cookie` 或一次性令牌原文。
 * 成功后先写 [SessionAuthCache]（请求立即可用）再落 [DeviceAuthStore]。
 */
@Suppress("TooManyFunctions") // Cookie、设备和只读设备共用唯一凭据写入口，避免拆分持久化所有权。
class AuthRepository(
    private val httpClient: HttpClient,
    private val authStore: DeviceAuthStore,
    private val authCache: SessionAuthCache,
    private val json: Json,
) {
    /** 管理员登录（cookie 会话通道）；成功返回持久化的凭据。 */
    @Suppress("TooGenericExceptionCaught") // 统一映射传输/解码错误；先重抛 CancellationException。
    suspend fun login(
        administratorName: String,
        password: String,
    ): SdkResult<StoredCredentials> =
        try {
            val response =
                httpClient.post("/api/v1/sessions") {
                    contentType(ContentType.Application.Json)
                    setBody(
                        json.encodeToString(
                            LoginRequest.serializer(),
                            LoginRequest(administratorName = administratorName, password = password),
                        ),
                    )
                }
            if (response.status != HttpStatusCode.Created) {
                SdkResult.Failure(ErrorMapping.fromResponse(response, json))
            } else {
                val session = json.decodeFromString(SessionResponse.serializer(), response.bodyAsText())
                val cookie =
                    parseSessionCookie(response.headers["Set-Cookie"])
                        ?: return SdkResult.Failure(
                            MediaFlowError.Unexpected("login response missing session cookie"),
                        )
                val credentials =
                    StoredCredentials.SessionCookie(
                        sessionCookieValue = cookie,
                        csrfToken = session.csrfToken,
                        accountId = session.account.id,
                        displayName = session.account.administratorName,
                    )
                persist(credentials)
            }
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (failure: Exception) {
            SdkResult.Failure(ErrorMapping.fromThrowable(failure))
        }

    /** 一次性配对码 enrollment（设备令牌通道，移动端主路径）。 */
    suspend fun enrollWithPairingCode(
        pairingCode: String,
        deviceName: String,
    ): SdkResult<StoredCredentials> = enroll(DeviceEnrollmentInput(pairingCode = pairingCode, deviceName = deviceName))

    /** TV配对只申请媒体读取；服务端未明确返回该权限时不保存令牌。 */
    suspend fun enrollReadOnlyWithPairingCode(
        pairingCode: String,
        deviceName: String,
    ): SdkResult<StoredCredentials> =
        enroll(
            DeviceEnrollmentInput(
                pairingCode = pairingCode,
                deviceName = deviceName,
                accessMode = DeviceAccessMode.mediaMinusReadMinusOnly,
            ),
        )

    /** 恢复只读设备前向服务端核对自身身份；401或权限不匹配清本地，瞬时网络失败保留凭据供重试。 */
    @Suppress("TooGenericExceptionCaught") // SDK边界映射失败，取消必须原样传播。
    suspend fun validateReadOnlyDevice(): SdkResult<Unit> {
        val credentials = authCache.current() as? StoredCredentials.DeviceToken
        val id = credentials?.deviceId
        if (id == null || !DEVICE_ID.matches(id)) {
            clearLocal()
            return SdkResult.failure(MediaFlowError.Unexpected("read-only device identity is missing"))
        }
        return try {
            val response = httpClient.get("/api/v1/devices/$id")
            if (response.status != HttpStatusCode.OK) {
                val error = ErrorMapping.fromResponse(response, json)
                if (error.isCredentialExpired()) clearLocal()
                SdkResult.failure(error)
            } else {
                val device = json.decodeFromString(DeviceView.serializer(), response.bodyAsText())
                if (device.id != id || device.accessMode != DeviceAccessMode.mediaMinusReadMinusOnly || device.revokedAt != null) {
                    clearLocal()
                    SdkResult.failure(
                        MediaFlowError.Http(
                            HttpStatusCode.Forbidden.value,
                            null,
                            "device is not authorized for read-only media access",
                            null,
                        ),
                    )
                } else {
                    SdkResult.success(Unit)
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            SdkResult.failure(ErrorMapping.fromThrowable(failure))
        }
    }

    /** 管理员凭据 enrollment（移动端登录页凭据模式；走登录限流）。 */
    suspend fun enrollWithCredentials(
        administratorName: String,
        password: String,
        deviceName: String,
    ): SdkResult<StoredCredentials> =
        enroll(
            DeviceEnrollmentInput(
                administratorName = administratorName,
                password = password,
                deviceName = deviceName,
            ),
        )

    /**
     * 设备令牌自轮换：服务端原子替换摘要后返回新令牌；失败时旧令牌保持可用。
     * 成功即更新本地凭据。
     */
    @Suppress("TooGenericExceptionCaught") // 统一映射传输/解码错误；先重抛 CancellationException。
    suspend fun rotateDeviceToken(): SdkResult<StoredCredentials> =
        try {
            val response = httpClient.post("/api/v1/devices/current/token-rotation")
            if (response.status != HttpStatusCode.OK) {
                SdkResult.Failure(ErrorMapping.fromResponse(response, json))
            } else {
                val rotation = json.decodeFromString(DeviceTokenRotation.serializer(), response.bodyAsText())
                val previous = authCache.current()
                if (previous !is StoredCredentials.DeviceToken) {
                    SdkResult.Failure(MediaFlowError.Unexpected("token rotation requires a device credential"))
                } else {
                    persist(previous.copy(token = rotation.token))
                }
            }
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (failure: Exception) {
            SdkResult.Failure(ErrorMapping.fromThrowable(failure))
        }

    /**
     * 登出：按通道失效服务端凭据并清空本地；网络失败也清本地（会话终将过期，
     * 设备令牌可在管理面吊销）。
     */
    @Suppress("TooGenericExceptionCaught") // 网络与安全存储错误映射；不可取消清理完成后保留原取消。
    suspend fun logout(): SdkResult<Unit> {
        var cancellation: CancellationException? = null
        var cleanupFailure: Exception? = null
        val result =
            try {
                val response =
                    when (val credentials = authCache.current()) {
                        is StoredCredentials.SessionCookie ->
                            httpClient.delete("/api/v1/session") {
                                header("X-CSRF-Token", credentials.csrfToken)
                            }
                        is StoredCredentials.DeviceToken -> {
                            val deviceId = requireNotNull(credentials.deviceId) { "device credential is missing its device id" }
                            httpClient.delete("/api/v1/devices/$deviceId")
                        }
                        null -> null
                    }
                if (response == null || response.status == HttpStatusCode.NoContent) {
                    SdkResult.success(Unit)
                } else {
                    SdkResult.failure(ErrorMapping.fromResponse(response, json))
                }
            } catch (cancelled: CancellationException) {
                cancellation = cancelled
                null
            } catch (failure: Exception) {
                SdkResult.failure(ErrorMapping.fromThrowable(failure))
            } finally {
                try {
                    withContext(NonCancellable) { clearLocal() }
                } catch (failure: Exception) {
                    cleanupFailure = failure
                }
            }
        cancellation?.let { throw it }
        currentCoroutineContext().ensureActive()
        val clearError = cleanupFailure
        if (clearError is CancellationException) throw clearError
        return if (clearError != null) SdkResult.failure(ErrorMapping.fromThrowable(clearError)) else requireNotNull(result)
    }

    @Suppress("TooGenericExceptionCaught") // 统一映射传输/解码错误；先重抛 CancellationException。
    private suspend fun enroll(input: DeviceEnrollmentInput): SdkResult<StoredCredentials> =
        try {
            val response =
                httpClient.post("/api/v1/devices/enrollment") {
                    contentType(ContentType.Application.Json)
                    setBody(json.encodeToString(DeviceEnrollmentInput.serializer(), input))
                }
            if (response.status != HttpStatusCode.Created) {
                SdkResult.Failure(ErrorMapping.fromResponse(response, json))
            } else {
                val enrollment = json.decodeFromString(DeviceEnrollment.serializer(), response.bodyAsText())
                if (input.accessMode == DeviceAccessMode.mediaMinusReadMinusOnly &&
                    enrollment.device.accessMode != DeviceAccessMode.mediaMinusReadMinusOnly
                ) {
                    return SdkResult.failure(MediaFlowError.Unexpected("server did not grant read-only media access"))
                }
                persist(
                    StoredCredentials.DeviceToken(
                        token = enrollment.token,
                        deviceId = enrollment.device.id,
                        displayName = enrollment.device.displayName,
                    ),
                )
            }
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (failure: Exception) {
            SdkResult.Failure(ErrorMapping.fromThrowable(failure))
        }

    @Suppress("TooGenericExceptionCaught") // 安全存储失败时回滚内存认证镜像，原异常交调用方处理。
    private suspend fun persist(credentials: StoredCredentials): SdkResult<StoredCredentials> {
        val previous = authCache.current()
        authCache.update(credentials)
        try {
            authStore.save(credentials)
        } catch (failure: Exception) {
            authCache.update(previous)
            throw failure
        }
        return SdkResult.Success(credentials)
    }

    /** 只清本地凭据（服务端 401 后的统一处理：令牌已失效，无谓再打吊销请求）。 */
    suspend fun discardLocalCredentials() = clearLocal()

    private suspend fun clearLocal() {
        authCache.update(null)
        authStore.clear()
    }

    companion object {
        private val DEVICE_ID = Regex("[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")

        /** 从 `__Host-mediaflow_session=<value>; Secure; ...` 提取 cookie 值。 */
        @Suppress("ReturnCount") // 早退即解析失败语义（无 header / 无前缀 / 空值）。
        fun parseSessionCookie(setCookie: String?): String? {
            val header = setCookie ?: return null
            val prefix = "${MediaFlowApiFactory.SESSION_COOKIE_NAME}="
            val start = header.indexOf(prefix)
            if (start < 0) return null
            val valueStart = start + prefix.length
            val valueEnd = header.indexOf(';', valueStart).let { if (it < 0) header.length else it }
            val value = header.substring(valueStart, valueEnd)
            return value.ifEmpty { null }
        }
    }
}
