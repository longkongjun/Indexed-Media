package mediaflow.client.core.tasks

import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.api.models.ProcessingTask
import mediaflow.client.core.SdkResult
import mediaflow.client.core.apiCall
import mediaflow.client.core.errorOrNull
import mediaflow.client.core.getOrNull
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.MediaFlowErrorTunnel
import mediaflow.client.core.paging.PageResult

/**
 * 任务中心：处理任务分页 + 取消。
 *
 * 取消是幂等写（幂等键 + CSRF 显式传参，经生成 API）；幂等键由调用方生成，
 * 重复提交同一键由服务端按重放语义收敛。
 */
class TaskRepository(
    private val api: DefaultApi,
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

    /** 幂等取消一个处理任务；返回取消后的任务投影。 */
    suspend fun cancel(
        processingTaskId: String,
        idempotencyKey: String,
    ): SdkResult<ProcessingTask> {
        val csrf =
            authCache.current()?.csrfToken ?: return SdkResult.failure(
                mediaflow.client.core.error.MediaFlowError
                    .Unexpected("cancel requires an authenticated session"),
            )
        return apiCall(
            json,
        ) { api.cancelProcessingTask(xCSRFToken = csrf, idempotencyKey = idempotencyKey, processingTaskId = processingTaskId) }
    }
}
