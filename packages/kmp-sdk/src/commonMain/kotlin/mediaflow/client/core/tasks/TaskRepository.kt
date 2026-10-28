package mediaflow.client.core.tasks

import io.ktor.client.HttpClient
import io.ktor.client.request.header
import io.ktor.client.request.post
import io.ktor.client.statement.bodyAsText
import io.ktor.http.HttpStatusCode
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.api.models.ProcessingTask
import mediaflow.client.core.SdkResult
import mediaflow.client.core.apiCall
import mediaflow.client.core.error.ErrorMapping
import mediaflow.client.core.errorOrNull
import mediaflow.client.core.getOrNull
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.MediaFlowErrorTunnel
import mediaflow.client.core.paging.PageResult

/**
 * 任务中心：处理任务分页 + 取消。
 *
 * 取消是幂等写（幂等键显式传参）：cookie 会话通道附 CSRF 头，设备令牌通道
 * 豁免（令牌即写证明）——因此走直连请求而非要求非空 CSRF 的生成 API。
 * 幂等键由调用方生成，重复提交同一键由服务端按重放语义收敛。
 */
class TaskRepository(
    private val api: DefaultApi,
    private val httpClient: HttpClient,
    private val authCache: SessionAuthCache,
    private val json: Json,
) {
    /** 处理任务分页器（默认待处理视图，对齐契约默认值）。 */
    fun tasks(limit: Int = CursorPager.DEFAULT_LIMIT): CursorPager<ProcessingTask> =
        CursorPager(
            fetch = { cursor, pageLimit ->
                val result = apiCall(json) { api.listProcessingTasks(cursor = cursor, limit = pageLimit.toLong()) }
                result.getOrNull()?.let { page -> PageResult(page.items, page.nextCursor) }
                    ?: throw MediaFlowErrorTunnel(result.errorOrNull!!)
            },
            limit = limit,
        )

    /** iOS 桥用的任务首页。 */
    internal suspend fun firstPageOfTasks(): mediaflow.api.infrastructure.HttpResponse<mediaflow.api.models.ProcessingTaskPage> =
        api.listProcessingTasks()

    /** 幂等取消一个处理任务；返回取消后的任务投影。 */
    @Suppress("TooGenericExceptionCaught") // 统一映射传输/解码错误；先重抛 CancellationException。
    suspend fun cancel(
        processingTaskId: String,
        idempotencyKey: String,
    ): SdkResult<ProcessingTask> =
        try {
            val response =
                httpClient.post("/api/v1/processing-tasks/$processingTaskId/cancel") {
                    header("Idempotency-Key", idempotencyKey)
                    (authCache.current() as? mediaflow.client.core.auth.StoredCredentials.SessionCookie)?.let {
                        header("X-CSRF-Token", it.csrfToken)
                    }
                }
            if (response.status == HttpStatusCode.Accepted) {
                SdkResult.success(json.decodeFromString(ProcessingTask.serializer(), response.bodyAsText()))
            } else {
                SdkResult.failure(ErrorMapping.fromResponse(response, json))
            }
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (failure: Exception) {
            SdkResult.failure(ErrorMapping.fromThrowable(failure))
        }
}
