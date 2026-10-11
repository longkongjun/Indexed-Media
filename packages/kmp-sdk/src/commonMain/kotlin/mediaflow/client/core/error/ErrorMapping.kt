package mediaflow.client.core.error

import io.ktor.client.network.sockets.ConnectTimeoutException
import io.ktor.client.plugins.HttpRequestTimeoutException
import io.ktor.client.statement.HttpResponse
import io.ktor.client.statement.bodyAsText
import kotlinx.io.IOException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mediaflow.api.models.ApiErrorBody

/**
 * 异常与错误响应 → [MediaFlowError] 的唯一映射入口。
 *
 * 这是 SDK 的指定兜底边界：允许在此捕获 Throwable 并归类——业务代码只面对
 * sealed 错误模型，不外抛传输层异常。
 */
object ErrorMapping {
    /** 传输层异常分类（无 HTTP 响应）。 */
    fun fromThrowable(throwable: Throwable): MediaFlowError =
        when (throwable) {
            is HttpRequestTimeoutException ->
                MediaFlowError.Network(MediaFlowError.NetworkKind.Timeout, throwable.message)
            is ConnectTimeoutException ->
                MediaFlowError.Network(MediaFlowError.NetworkKind.Connect, throwable.message)
            is kotlinx.serialization.SerializationException -> MediaFlowError.Serialization(throwable.message)
            is IOException -> classifyIOException(throwable)
            else -> MediaFlowError.Unexpected(throwable.message)
        }

    /**
     * IO 异常细分：Ktor 3 无 common UnknownHostException，DNS 失败经类型名识别；
     * 其余按对端关闭处理（SSE 断线重连依赖 Closed 分类）。
     */
    private fun classifyIOException(throwable: IOException): MediaFlowError.Network {
        val name = throwable::class.simpleName.orEmpty()
        return when {
            name.contains("UnknownHost") -> MediaFlowError.Network(MediaFlowError.NetworkKind.Dns, throwable.message)
            name.contains("Connect") -> MediaFlowError.Network(MediaFlowError.NetworkKind.Connect, throwable.message)
            name.contains("SSL", ignoreCase = true) || name.contains("Tls", ignoreCase = true) ->
                MediaFlowError.Network(MediaFlowError.NetworkKind.Ssl, throwable.message)
            else -> MediaFlowError.Network(MediaFlowError.NetworkKind.Closed, throwable.message)
        }
    }

    /**
     * HTTP 错误响应 → 信封化错误；尽力解码 `ApiErrorBody`，失败时退回状态行描述。
     */
    suspend fun fromResponse(
        response: HttpResponse,
        json: Json,
    ): MediaFlowError.Http {
        val body = runCatching { response.bodyAsText() }.getOrNull()
        val envelope =
            body
                ?.let { text -> runCatching { json.decodeFromString(ApiErrorBody.serializer(), text) }.getOrNull() }
        return MediaFlowError.Http(
            status = response.status.value,
            code = envelope?.error?.code?.value,
            message = envelope?.error?.message ?: "HTTP ${response.status.value}: ${response.status.description}",
            requestId = envelope?.error?.requestId,
        )
    }

    /** 供响应体不可读时从信封 JSON 原文抽取 code/message（宽松路径；status 由调用方补）。 */
    fun fromEnvelopeJson(
        text: String,
        json: Json,
    ): MediaFlowError.Http? {
        val error =
            runCatching {
                json.parseToJsonElement(text).jsonObject["error"]?.jsonObject
            }.getOrNull() ?: return null
        return MediaFlowError.Http(
            status = 0,
            code = error["code"]?.jsonPrimitive?.content,
            message = error["message"]?.jsonPrimitive?.content ?: "",
            requestId = error["request_id"]?.jsonPrimitive?.content,
        )
    }
}
