package mediaflow.client.core.review

import io.ktor.client.HttpClient
import io.ktor.client.request.header
import io.ktor.client.request.post
import io.ktor.client.request.setBody
import io.ktor.client.statement.bodyAsText
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.http.contentType
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.api.models.RematchWithHintsCommand
import mediaflow.api.models.ReviewCandidate
import mediaflow.api.models.ReviewCase
import mediaflow.api.models.SelectGenericVideoCommand
import mediaflow.api.models.SelectProviderCandidateCommand
import mediaflow.api.models.TaskDecisionReceipt
import mediaflow.client.core.SdkResult
import mediaflow.client.core.apiCall
import mediaflow.client.core.error.ErrorMapping
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.errorOrNull
import mediaflow.client.core.getOrNull
import mediaflow.client.core.net.SessionAuthCache
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.MediaFlowErrorTunnel
import mediaflow.client.core.paging.PageResult

/**
 * 人工确认读模型 + 决定写路径。
 *
 * 决定提交走直连请求：生成的 sealed `ReviewDecisionRequest` 序列化器不兼容契约
 * 的"平铺 kind 判别"形状（决策 D-0007），这里用**具体命令类**的序列化器编码
 * （形状正确），头三件套（Idempotency-Key / If-Match / X-CSRF-Token）显式传参。
 */
class ReviewRepository(
    private val api: DefaultApi,
    private val httpClient: HttpClient,
    private val authCache: SessionAuthCache,
    private val json: Json,
) {
    /** 审核队列分页器（decision_level 筛选可扩展）。 */
    fun reviewQueue(limit: Int = CursorPager.DEFAULT_LIMIT): CursorPager<ReviewCase> =
        CursorPager(
            fetch = { cursor, pageLimit ->
                val result = apiCall(json) { api.listReviewCases(cursor = cursor, limit = pageLimit.toLong()) }
                result.getOrNull()?.let { page -> PageResult(page.items, page.nextCursor) }
                    ?: throw MediaFlowErrorTunnel(result.errorOrNull!!)
            },
            limit = limit,
        )

    /** 单个审核案件（含候选与当前决定状态）。 */
    suspend fun reviewCase(reviewCaseId: String): SdkResult<ReviewCase> = apiCall(json) { api.getReviewCase(reviewCaseId) }

    /** iOS 桥用的队列首页。 */
    internal suspend fun firstPageOfQueue(): mediaflow.api.infrastructure.HttpResponse<mediaflow.api.models.ReviewCasePage> =
        api.listReviewCases()

    /** 提供方候选搜索（临时性安全摘要，读取无副作用）。 */
    suspend fun searchCandidates(
        reviewCaseId: String,
        query: String,
        mediaType: String,
        locale: String,
    ): SdkResult<List<ReviewCandidate>> {
        val parsedType =
            DefaultApi.MediaTypeSearchReviewCandidates.entries.firstOrNull { it.value == mediaType }
                ?: return SdkResult.failure(MediaFlowError.Serialization("unsupported media type: $mediaType"))
        val result = apiCall(json) { api.searchReviewCandidates(reviewCaseId, query, parsedType, locale) }
        return when (result) {
            is SdkResult.Success -> SdkResult.success(result.value.items)
            is SdkResult.Failure -> result
        }
    }

    /**
     * 提交一次不可变人工决定。幂等键由调用方生成（≥8 字符）；`caseVersion` 为
     * 提交时的活跃案件版本（If-Match），版本冲突返回 Http(409)。
     */
    suspend fun submitDecision(
        reviewCaseId: String,
        caseVersion: Long,
        idempotencyKey: String,
        command: ReviewDecisionKind,
    ): SdkResult<TaskDecisionReceipt> =
        try {
            val session = authCache.current() as? mediaflow.client.core.auth.StoredCredentials.SessionCookie
            val response =
                httpClient.post("/api/v1/review-cases/$reviewCaseId/decisions") {
                    contentType(ContentType.Application.Json)
                    header("Idempotency-Key", idempotencyKey)
                    header("If-Match", caseVersion.toString())
                    // CSRF 仅 cookie 会话通道需要；设备令牌通道豁免（令牌即写证明）。
                    if (session != null) header("X-CSRF-Token", session.csrfToken)
                    setBody(encodeCommand(command))
                }
            if (response.status == HttpStatusCode.Accepted) {
                SdkResult.success(json.decodeFromString(TaskDecisionReceipt.serializer(), response.bodyAsText()))
            } else {
                SdkResult.failure(ErrorMapping.fromResponse(response, json))
            }
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (failure: Exception) {
            SdkResult.failure(ErrorMapping.fromThrowable(failure))
        }

    /** 三种决定命令的判别密封（携带生成具体类，序列化按具体形状）。 */
    sealed interface ReviewDecisionKind {
        data class SelectProviderCandidate(
            val command: SelectProviderCandidateCommand,
        ) : ReviewDecisionKind

        data class RematchWithHints(
            val command: RematchWithHintsCommand,
        ) : ReviewDecisionKind

        data class SelectGenericVideo(
            val command: SelectGenericVideoCommand,
        ) : ReviewDecisionKind
    }

    private fun encodeCommand(kind: ReviewDecisionKind): String =
        when (kind) {
            is ReviewDecisionKind.SelectProviderCandidate ->
                json.encodeToString(SelectProviderCandidateCommand.serializer(), kind.command)
            is ReviewDecisionKind.RematchWithHints ->
                json.encodeToString(RematchWithHintsCommand.serializer(), kind.command)
            is ReviewDecisionKind.SelectGenericVideo ->
                json.encodeToString(SelectGenericVideoCommand.serializer(), kind.command)
        }
}
