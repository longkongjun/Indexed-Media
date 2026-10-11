package mediaflow.client.core.error

/**
 * SDK 统一错误模型。
 *
 * 对齐两处契约语义：
 * - Core 错误信封（`ApiErrorBody`：code/message/requestId）→ [MediaFlowError.Http]；
 * - 页面状态语义（ui-contract：Loading/Content/Empty/Error/Offline）→
 *   [isOffline] 为 true 的网络错误映射 Offline，其余映射 Error（stale 由 UI 层持有）。
 */
sealed interface MediaFlowError {
    /** 是否可安全自动重试：仅限幂等读或携带幂等键的写；由 Repository 层执行重试。 */
    val retryable: Boolean

    /** HTTP 错误响应（收到服务端错误信封）。 */
    data class Http(
        val status: Int,
        val code: String?,
        val message: String,
        val requestId: String?,
    ) : MediaFlowError {
        override val retryable: Boolean get() = status in RETRYABLE_STATUS
    }

    /** 网络层失败（未收到 HTTP 响应）。 */
    data class Network(
        val kind: NetworkKind,
        val cause: String?,
    ) : MediaFlowError {
        override val retryable: Boolean
            get() = kind == NetworkKind.Connect || kind == NetworkKind.Timeout || kind == NetworkKind.Dns
    }

    /** 响应体与契约不符（解码失败）。 */
    data class Serialization(
        val cause: String?,
    ) : MediaFlowError {
        override val retryable: Boolean = false
    }

    /** 兜底：非预期异常（SDK 边界不外抛原始异常）。 */
    data class Unexpected(
        val cause: String?,
    ) : MediaFlowError {
        override val retryable: Boolean = false
    }

    /** 网络失败细分；Closed 指连接被对端/系统关闭（SSE 断线重连依赖该分类）。 */
    enum class NetworkKind { Connect, Dns, Timeout, Ssl, Closed }

    /** 连接建立失败与域名解析失败视为离线（对齐 Web 端 isOffline 语义）。 */
    fun isOffline(): Boolean = this is Network && (kind == NetworkKind.Connect || kind == NetworkKind.Dns)

    /**
     * 服务端判定凭据失效（401 / session.expired）：令牌过期、被吊销或改密连坐。
     * 统一 401 处理由此触发——清空本地凭据并回到配对/登录界面。
     */
    fun isCredentialExpired(): Boolean = this is Http && status == UNAUTHORIZED

    companion object {
        /** 未授权：凭据缺失或失效（错误信封 code 通常为 session.expired）。 */
        const val UNAUTHORIZED: Int = 401

        /** 可自动重试的 HTTP 状态：请求超时/过早/限流/服务端瞬断。 */
        val RETRYABLE_STATUS: Set<Int> = setOf(408, 425, 429, 500, 502, 503, 504)
    }
}
