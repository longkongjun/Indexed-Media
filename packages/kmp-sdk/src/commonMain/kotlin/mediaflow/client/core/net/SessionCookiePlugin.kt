package mediaflow.client.core.net

import io.ktor.client.plugins.api.createClientPlugin
import io.ktor.client.request.header
import io.ktor.http.HttpHeaders

/**
 * 会话 cookie 注入插件：全部请求携带 `__Host-mediaflow_session`（经同步缓存读取，
 * 未登录时不注入）。REST 与 SSE 客户端共用。
 */
internal object SessionCookiePlugin {
    fun create(authCache: SessionAuthCache) =
        createClientPlugin(
            "MediaFlowSessionCookie",
            { authCache },
        ) {
            val cache = pluginConfig
            onRequest { request, _ ->
                val name = MediaFlowApiFactory.SESSION_COOKIE_NAME
                cache.current()?.let { credentials ->
                    request.header(HttpHeaders.Cookie, "$name=${credentials.sessionCookieValue}")
                }
            }
        }
}
