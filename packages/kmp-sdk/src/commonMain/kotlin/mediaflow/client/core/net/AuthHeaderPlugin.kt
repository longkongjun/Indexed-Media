package mediaflow.client.core.net

import io.ktor.client.plugins.api.createClientPlugin
import io.ktor.client.request.header
import io.ktor.http.HttpHeaders
import mediaflow.client.core.auth.StoredCredentials

/**
 * 凭据注入插件：按通道附加 `__Host-mediaflow_session` Cookie 或
 * `Authorization: Bearer` 设备令牌（经同步缓存读取，未登录时不注入）。
 * REST 与 SSE 客户端共用。
 */
internal object AuthHeaderPlugin {
    fun create(authCache: SessionAuthCache) =
        createClientPlugin(
            "MediaFlowCredentials",
            { authCache },
        ) {
            val cache = pluginConfig
            onRequest { request, _ ->
                when (val credentials = cache.current()) {
                    is StoredCredentials.SessionCookie ->
                        request.header(
                            HttpHeaders.Cookie,
                            "${MediaFlowApiFactory.SESSION_COOKIE_NAME}=${credentials.sessionCookieValue}",
                        )
                    is StoredCredentials.DeviceToken ->
                        request.header(HttpHeaders.Authorization, "Bearer ${credentials.token}")
                    null -> {}
                }
            }
        }
}
