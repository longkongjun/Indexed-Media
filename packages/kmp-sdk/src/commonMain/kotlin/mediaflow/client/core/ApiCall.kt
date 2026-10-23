package mediaflow.client.core

import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.Json
import mediaflow.api.infrastructure.HttpResponse
import mediaflow.client.core.error.ErrorMapping
import mediaflow.client.core.error.MediaFlowError

/**
 * 生成 API 调用的统一包装：成功取 body，非 2xx 经 [ErrorMapping] 归类为
 * [MediaFlowError.Http]（尽力解码错误信封），传输异常不外抛。
 */
@Suppress("TooGenericExceptionCaught") // 统一映射传输/解码错误；先重抛 CancellationException。
suspend fun <T : Any> apiCall(
    json: Json,
    call: suspend () -> HttpResponse<T>,
): SdkResult<T> =
    try {
        val response = call()
        if (response.success) {
            SdkResult.Success(response.body())
        } else {
            SdkResult.Failure(ErrorMapping.fromResponse(response.response, json))
        }
    } catch (cancellation: CancellationException) {
        throw cancellation
    } catch (failure: Exception) {
        SdkResult.Failure(ErrorMapping.fromThrowable(failure))
    }
