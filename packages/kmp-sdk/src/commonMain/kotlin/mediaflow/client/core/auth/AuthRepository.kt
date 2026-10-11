package mediaflow.client.core.auth

import io.ktor.client.HttpClient
import io.ktor.client.request.delete
import io.ktor.client.request.header
import io.ktor.client.request.post
import io.ktor.client.request.setBody
import io.ktor.client.statement.bodyAsText
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.contentType
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.Json
import mediaflow.api.models.LoginRequest
import mediaflow.api.models.SessionResponse
import mediaflow.client.core.SdkResult
import mediaflow.client.core.error.ErrorMapping
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache

/**
 * 登录/登出/会话恢复：凭据唯一写入口。
 *
 * 登录走直连请求（不经生成 API）：需要从 `Set-Cookie` 头提取会话 cookie 值——
 * 生成客户端只返回反序列化 body。成功后先写 [SessionAuthCache]（请求立即可用）
 * 再落 [DeviceAuthStore]。
 */
class AuthRepository(
    private val httpClient: HttpClient,
    private val authStore: DeviceAuthStore,
    private val authCache: SessionAuthCache,
    private val json: Json,
) {
    /** 管理员登录；成功返回持久化的凭据。 */
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
                            mediaflow.client.core.error.MediaFlowError
                                .Unexpected("login response missing session cookie"),
                        )
                val credentials =
                    StoredCredentials(
                        sessionCookieValue = cookie,
                        csrfToken = session.csrfToken,
                        accountId = session.account.id,
                        administratorName = session.account.administratorName,
                    )
                authCache.update(credentials)
                authStore.save(credentials)
                SdkResult.Success(credentials)
            }
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (failure: Exception) {
            SdkResult.Failure(ErrorMapping.fromThrowable(failure))
        }

    /** 登出：失效服务端会话并清空本地凭据；网络失败也清本地（会话终将过期）。 */
    suspend fun logout(): SdkResult<Unit> =
        try {
            val csrf = authCache.current()?.csrfToken
            val response =
                httpClient.delete("/api/v1/session") {
                    if (csrf != null) header("X-CSRF-Token", csrf)
                }
            clearLocal()
            if (response.status == HttpStatusCode.NoContent) {
                SdkResult.Success(Unit)
            } else {
                SdkResult.Failure(ErrorMapping.fromResponse(response, json))
            }
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (failure: Exception) {
            clearLocal()
            SdkResult.Failure(ErrorMapping.fromThrowable(failure))
        }

    private suspend fun clearLocal() {
        authCache.update(null)
        authStore.clear()
    }

    companion object {
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
