package mediaflow.client.core

import mediaflow.client.core.error.MediaFlowError

/** SDK 层统一返回：业务代码不跨层传播异常。 */
sealed interface SdkResult<out T> {
    data class Success<T>(
        val value: T,
    ) : SdkResult<T>

    data class Failure(
        val error: MediaFlowError,
    ) : SdkResult<Nothing>

    companion object {
        fun <T> success(value: T): SdkResult<T> = Success(value)

        fun failure(error: MediaFlowError): SdkResult<Nothing> = Failure(error)
    }
}

/** 成功取值，失败返回 null（窄化场景用）。 */
fun <T> SdkResult<T>.getOrNull(): T? = (this as? SdkResult.Success)?.value

/** 语义不变的错误透传。 */
val SdkResult<*>.errorOrNull: MediaFlowError?
    get() = (this as? SdkResult.Failure)?.error
