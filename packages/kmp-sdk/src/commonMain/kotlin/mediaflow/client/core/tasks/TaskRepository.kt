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

/** 服务端任务视图与筛选上下文；换条件时创建新的分页器。 */
data class TaskFilter(
    val view: String = "pending",
    val stage: String? = null,
    val status: String? = null,
    val query: String? = null,
) {
    init {
        require(DefaultApi.ViewListProcessingTasks.entries.any { it.value == view })
        require(stage == null || DefaultApi.StageListProcessingTasks.entries.any { it.value == stage })
        require(status == null || DefaultApi.StatusListProcessingTasks.entries.any { it.value == status })
        require((query?.trim()?.length ?: 0) <= MAX_QUERY_LENGTH)
    }

    companion object {
        /** 契约任务搜索字段的最大字符数。 */
        const val MAX_QUERY_LENGTH = 200
    }
}

/**
 * 任务中心：处理任务筛选分页、详情、重试与取消。
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
    /** 保留既有调用入口与默认待处理视图。 */
    fun tasks(limit: Int = CursorPager.DEFAULT_LIMIT): CursorPager<ProcessingTask> = tasks(TaskFilter(), limit)

    /** 创建固定筛选上下文的处理任务分页器。 */
    fun tasks(
        filter: TaskFilter,
        limit: Int = CursorPager.DEFAULT_LIMIT,
    ): CursorPager<ProcessingTask> =
        CursorPager(
            fetch = { cursor, pageLimit ->
                val result =
                    apiCall(json) {
                        api.listProcessingTasks(
                            cursor = cursor,
                            limit = pageLimit.toLong(),
                            view = DefaultApi.ViewListProcessingTasks.entries.first { it.value == filter.view },
                            stage =
                                filter.stage?.let { value ->
                                    DefaultApi.StageListProcessingTasks.entries.first { it.value == value }
                                },
                            status =
                                filter.status?.let { value ->
                                    DefaultApi.StatusListProcessingTasks.entries.first { it.value == value }
                                },
                            q = filter.query?.trim()?.takeIf { it.isNotEmpty() },
                        )
                    }
                result.getOrNull()?.let { page -> PageResult(page.items, page.nextCursor) }
                    ?: throw MediaFlowErrorTunnel(result.errorOrNull!!)
            },
            limit = limit,
        )

    /** iOS 桥用的任务首页。 */
    internal suspend fun firstPageOfTasks(): mediaflow.api.infrastructure.HttpResponse<mediaflow.api.models.ProcessingTaskPage> =
        api.listProcessingTasks()

    /** 读取当前任务权威投影，包含原因、检查点与重试信息。 */
    suspend fun detail(processingTaskId: String): SdkResult<ProcessingTask> = apiCall(json) { api.getProcessingTask(processingTaskId) }

    /** 幂等创建一次可恢复任务的重试，沿用调用方保存的同一次操作键。 */
    suspend fun retry(
        processingTaskId: String,
        idempotencyKey: String,
    ): SdkResult<ProcessingTask> = writeTaskAction(processingTaskId, idempotencyKey, "attempts")

    /** 幂等取消一个处理任务；返回取消后的任务投影。 */
    suspend fun cancel(
        processingTaskId: String,
        idempotencyKey: String,
    ): SdkResult<ProcessingTask> = writeTaskAction(processingTaskId, idempotencyKey, "cancel")

    @Suppress("TooGenericExceptionCaught") // 统一映射传输/解码错误；先重抛 CancellationException。
    private suspend fun writeTaskAction(
        processingTaskId: String,
        idempotencyKey: String,
        action: String,
    ): SdkResult<ProcessingTask> =
        try {
            val response =
                httpClient.post("/api/v1/processing-tasks/$processingTaskId/$action") {
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
