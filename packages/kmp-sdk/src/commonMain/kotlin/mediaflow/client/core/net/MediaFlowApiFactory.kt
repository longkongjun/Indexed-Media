package mediaflow.client.core.net

import io.ktor.client.HttpClient
import io.ktor.client.plugins.DefaultRequest
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.client.request.header
import io.ktor.http.HttpHeaders
import io.ktor.serialization.kotlinx.json.json
import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi

/**
 * 传输层组装：自建 [HttpClient]（平台引擎 + 超时 + 认证头注入 + 内容协商）注入
 * 契约生成的 [DefaultApi]——端点形状由契约生成，传输行为由 SDK 控制。
 *
 * 认证约定（对齐 Core 契约）：
 * - 读请求：cookie `__Host-mediaflow_session`（经 [SessionAuthCache] 动态注入）；
 * - 写请求：CSRF 令牌由 Repository 按端点显式传参（生成 API 的 `xCSRFToken` 参数），
 *   不做隐式注入；Origin 与 Sec-Fetch-Site 以同源固定值随默认头发送。
 */
class MediaFlowApiFactory(
    private val baseUrl: String,
    private val authCache: SessionAuthCache,
    private val json: Json = defaultJson(),
) {
    /** 构建配置完备的共享 HttpClient；SSE 等长连接场景请单独建 client 覆写超时。 */
    fun httpClient(): HttpClient =
        HttpClient(platformHttpClientEngine()) {
            expectSuccess = false
            install(ContentNegotiation) { json(json) }
            install(DefaultRequest) {
                url(baseUrl)
                header(HttpHeaders.Origin, originProof)
                header(SEC_FETCH_SITE_HEADER, "same-origin")
            }
            install(sessionCookiePlugin())
            install(HttpTimeout) {
                requestTimeoutMillis = REQUEST_TIMEOUT_MS
                connectTimeoutMillis = CONNECT_TIMEOUT_MS
            }
        }

    /** 生成 API 的组装入口。 */
    fun defaultApi(client: HttpClient = httpClient()): DefaultApi = DefaultApi(baseUrl = baseUrl, httpClient = client)

    /** `scheme://host[:port]`：同源证明头使用 base URL 的规范化来源。 */
    private val originProof: String by lazy {
        val parsed = io.ktor.http.Url(baseUrl)
        buildString {
            append(parsed.protocol.name)
            append("://")
            append(parsed.host)
            if (parsed.port != parsed.protocol.defaultPort && parsed.port != 0) {
                append(':')
                append(parsed.port)
            }
        }
    }

    private fun sessionCookiePlugin() = SessionCookiePlugin.create(authCache)

    companion object {
        const val SESSION_COOKIE_NAME = "__Host-mediaflow_session"
        const val SEC_FETCH_SITE_HEADER = "Sec-Fetch-Site"
        const val REQUEST_TIMEOUT_MS: Long = 20_000
        const val CONNECT_TIMEOUT_MS: Long = 8_000

        /** 契约演进宽容：未知键忽略；出参省略 null 与默认值（对齐服务端只读语义）。 */
        fun defaultJson(): Json =
            Json {
                ignoreUnknownKeys = true
                explicitNulls = false
                encodeDefaults = false
            }
    }
}
