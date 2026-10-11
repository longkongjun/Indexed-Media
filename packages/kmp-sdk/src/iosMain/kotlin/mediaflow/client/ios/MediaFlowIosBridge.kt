package mediaflow.client.ios

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlin.time.Clock
import mediaflow.client.core.SdkResult
import mediaflow.client.core.getOrNull
import mediaflow.client.core.apiCall
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.events.TaskEventClient
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache

/** 单条媒体墙条目的 Swift 友好投影（基本类型，避免泛型/时间类型跨桥）。 */
data class WallItem(
    val id: String,
    val title: String,
    val typeLabel: String,
    val yearLabel: String,
    val statusLabel: String,
    val hasArtwork: Boolean,
)

data class WallPage(val items: List<WallItem>, val nextCursor: String?)

data class DetailUi(
    val id: String,
    val title: String,
    val subtitle: String,
    val metadataLines: List<String>,
    val versionLines: List<String>,
    val childTitles: List<String>,
    val playbackUrl: String?,
)

data class ReviewCaseUi(
    val id: String,
    val displayTitle: String,
    val version: Long,
    val levelLabel: String,
)

data class CandidateUi(
    val title: String,
    val subtitle: String,
    val providerId: String,
    val mediaTypeRaw: String,
)

data class ReceiptUi(val stateLabel: String, val caseVersion: Long)

data class TaskUi(
    val id: String,
    val pathLabel: String,
    val statusLabel: String,
    val cancellable: Boolean,
)

data class WallOutcome(val page: WallPage?, val error: String?)

data class DetailOutcome(val detail: DetailUi?, val error: String?)

data class CasesOutcome(val cases: List<ReviewCaseUi>, val error: String?)

data class CandidatesOutcome(val candidates: List<CandidateUi>, val error: String?)

data class ReceiptOutcome(val receipt: ReceiptUi?, val error: String?)

data class TasksOutcome(val tasks: List<TaskUi>, val error: String?)

data class Outcome(val success: Boolean, val message: String? = null)

/**
 * iOS 接入桥：suspend 函数经 Kotlin/Native 自动导出为 completionHandler 形态；
 * 投影只用基本类型与 List<投影>，避免泛型 StateFlow / kotlinx 时间类型跨桥。
 * SSE 事件经 [setEventHandler] 以契约 type 字符串回调（Swift 侧映射为刷新信号）。
 */
class MediaFlowIosBridge {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var cache = SessionAuthCache()
    private var eventClient: TaskEventClient? = null
    private var eventJob: Job? = null
    private var eventHandler: ((String) -> Unit)? = null

    private var auth: mediaflow.client.core.auth.AuthRepository? = null
    private var media: mediaflow.client.core.media.MediaRepository? = null
    private var review: mediaflow.client.core.review.ReviewRepository? = null
    private var tasks: mediaflow.client.core.tasks.TaskRepository? = null
    private var playback: mediaflow.client.core.playback.PlaybackRepository? = null

    fun setEventHandler(handler: ((String) -> Unit)?) {
        eventHandler = handler
    }

    suspend fun login(
        baseUrl: String,
        name: String,
        password: String,
    ): Outcome {
        val json = MediaFlowApiFactory.defaultJson()
        val normalized = baseUrl.trimEnd('/')
        cache = SessionAuthCache()
        val transport = MediaFlowApiFactory(normalized, cache, json)
        val client = transport.httpClient()
        val api = mediaflow.api.apis.DefaultApi(baseUrl = normalized, httpClient = transport.httpClient())
        auth = mediaflow.client.core.auth.AuthRepository(client, mediaflow.client.core.auth.InMemoryDeviceAuthStore(), cache, json)
        return when (val result = auth!!.login(name, password)) {
            is SdkResult.Success -> {
                media = mediaflow.client.core.media.MediaRepository(api, json)
                review = mediaflow.client.core.review.ReviewRepository(api, client, cache, json)
                tasks = mediaflow.client.core.tasks.TaskRepository(api, cache, json)
                playback = mediaflow.client.core.playback.PlaybackRepository(api, json)
                eventClient =
                    TaskEventClient(
                        baseUrl = normalized,
                        authCache = cache,
                        json = json,
                        engine = mediaflow.client.core.net.platformHttpClientEngine(),
                    )
                startEvents()
                Outcome(true)
            }
            is SdkResult.Failure -> Outcome(false, readable(result.error))
        }
    }

    private fun startEvents() {
        eventJob?.cancel()
        val client = eventClient ?: return
        eventJob =
            scope.launch {
                client.events().collect { envelope -> eventHandler?.invoke(envelope.type) }
            }
    }

    suspend fun wallPage(cursor: String?): WallOutcome {
        val repo = media ?: return WallOutcome(null, "not logged in")
        val result = apiCall(MediaFlowApiFactory.defaultJson()) { repo.apiPage(cursor) }
        return when (result) {
            is SdkResult.Success ->
                WallOutcome(
                    WallPage(
                        result.value.items.map { item ->
                            WallItem(
                                id = item.id,
                                title = item.title,
                                typeLabel = typeLabel(item.type.toString()),
                                yearLabel = item.year?.toString() ?: "—",
                                statusLabel = item.localStatus.toString(),
                                hasArtwork = item.artworkRef?.state.toString() == "available",
                            )
                        },
                        result.value.nextCursor,
                    ),
                    null,
                )
            is SdkResult.Failure -> WallOutcome(null, readable(result.error))
        }
    }

    suspend fun detail(mediaItemId: String): DetailOutcome {
        val repo = media ?: return DetailOutcome(null, "not logged in")
        return when (val result = repo.detail(mediaItemId)) {
            is SdkResult.Success -> {
                val d = result.value
                DetailOutcome(
                    DetailUi(
                        id = d.item.id,
                        title = d.item.title,
                        subtitle = "${typeLabel(d.item.type.toString())} · ${d.item.year ?: "—"}",
                        metadataLines = d.metadata.map { "${it.field}: ${it.value ?: "—"}" },
                        versionLines = d.versions.map { "· ${it.label ?: "主版本"}（${it.files.size} 个文件）" },
                        childTitles = d.children.map { "· ${it.title}" },
                        playbackUrl = playback?.externalPlaybackUrl()?.getOrNull(),
                    ),
                    null,
                )
            }
            is SdkResult.Failure -> DetailOutcome(null, readable(result.error))
        }
    }

    suspend fun reviewCases(): CasesOutcome {
        val repo = review ?: return CasesOutcome(emptyList(), "not logged in")
        val result = apiCall(MediaFlowApiFactory.defaultJson()) { repo.firstPageOfQueue() }
        return when (result) {
            is SdkResult.Success ->
                CasesOutcome(
                    result.value.items.map { c ->
                        ReviewCaseUi(c.id, c.titleHint ?: c.relativePath, c.version, c.level.toString())
                    },
                    null,
                )
            is SdkResult.Failure -> CasesOutcome(emptyList(), readable(result.error))
        }
    }

    suspend fun candidates(
        reviewCaseId: String,
        query: String,
        mediaType: String,
    ): CandidatesOutcome {
        val repo = review ?: return CandidatesOutcome(emptyList(), "not logged in")
        return when (val result = repo.searchCandidates(reviewCaseId, query, mediaType, "zh-CN")) {
            is SdkResult.Success ->
                CandidatesOutcome(
                    result.value.map { c ->
                        CandidateUi(c.title, "${c.provider} · ${c.mediaType} · ${c.year ?: "—"}", c.providerId, c.mediaType.toString())
                    },
                    null,
                )
            is SdkResult.Failure -> CandidatesOutcome(emptyList(), readable(result.error))
        }
    }

    suspend fun submitCandidate(
        reviewCaseId: String,
        caseVersion: Long,
        providerId: String,
        mediaTypeRaw: String,
    ): ReceiptOutcome {
        val repo = review ?: return ReceiptOutcome(null, "not logged in")
        val command =
            mediaflow.api.models.SelectProviderCandidateCommand(
                kind = "select-provider-candidate",
                provider = "tmdb",
                mediaType =
                    if (mediaTypeRaw == "tv") {
                        mediaflow.api.models.SelectProviderCandidateCommand.MediaType.tv
                    } else {
                        mediaflow.api.models.SelectProviderCandidateCommand.MediaType.movie
                    },
                providerId = providerId,
                saveFeedback = false,
            )
        return submit(repo, reviewCaseId, caseVersion, ReviewCommand.candidate(command))
    }

    suspend fun submitGeneric(
        reviewCaseId: String,
        caseVersion: Long,
        displayTitle: String,
    ): ReceiptOutcome {
        val repo = review ?: return ReceiptOutcome(null, "not logged in")
        val command =
            mediaflow.api.models.SelectGenericVideoCommand(
                kind = "select-generic-video",
                displayTitle = displayTitle,
                groupHint = null,
                saveGroupingFeedback = false,
            )
        return submit(repo, reviewCaseId, caseVersion, ReviewCommand.generic(command))
    }

    private suspend fun submit(
        repo: mediaflow.client.core.review.ReviewRepository,
        reviewCaseId: String,
        caseVersion: Long,
        kind: ReviewCommand,
    ): ReceiptOutcome {
        val result =
            repo.submitDecision(
                reviewCaseId = reviewCaseId,
                caseVersion = caseVersion,
                idempotencyKey = "ios-decision-${Clock.System.now().toEpochMilliseconds()}",
                command = kind.wrapped,
            )
        return when (result) {
            is SdkResult.Success -> ReceiptOutcome(ReceiptUi(result.value.state.toString(), result.value.caseVersion), null)
            is SdkResult.Failure -> ReceiptOutcome(null, readable(result.error))
        }
    }

    suspend fun tasks(): TasksOutcome {
        val repo = tasks ?: return TasksOutcome(emptyList(), "not logged in")
        val result = apiCall(MediaFlowApiFactory.defaultJson()) { repo.firstPageOfTasks() }
        return when (result) {
            is SdkResult.Success ->
                TasksOutcome(
                    result.value.items.map { t ->
                        TaskUi(t.id, t.relativePath, "${t.status} · ${t.stage}", t.allowedActions.any { it.toString() == "cancel" })
                    },
                    null,
                )
            is SdkResult.Failure -> TasksOutcome(emptyList(), readable(result.error))
        }
    }

    suspend fun cancelTask(taskId: String): Outcome {
        val repo = tasks ?: return Outcome(false, "not logged in")
        return when (val result = repo.cancel(taskId, "ios-cancel-${Clock.System.now().toEpochMilliseconds()}")) {
            is SdkResult.Success -> Outcome(true)
            is SdkResult.Failure -> Outcome(false, readable(result.error))
        }
    }

    private fun readable(error: MediaFlowError): String =
        when (error) {
            is MediaFlowError.Http ->
                when (error.status) {
                    401 -> "登录已过期，请重新登录"
                    404 -> "内容不存在或已移除"
                    409 -> "内容已被他人更新，请刷新后重试"
                    else -> error.message.ifBlank { "服务错误（${error.status}）" }
                }
            is MediaFlowError.Network -> "网络异常或离线"
            else -> "发生错误"
        }
}

/** 决定命令的内部判别（桥内避免导出 sealed 泛型）。 */
private class ReviewCommand(val wrapped: mediaflow.client.core.review.ReviewRepository.ReviewDecisionKind) {
    companion object {
        fun candidate(command: mediaflow.api.models.SelectProviderCandidateCommand) =
            ReviewCommand(mediaflow.client.core.review.ReviewRepository.ReviewDecisionKind.SelectProviderCandidate(command))

        fun generic(command: mediaflow.api.models.SelectGenericVideoCommand) =
            ReviewCommand(mediaflow.client.core.review.ReviewRepository.ReviewDecisionKind.SelectGenericVideo(command))
    }
}

private fun typeLabel(type: String): String =
    when (type) {
        "movie" -> "电影"
        "series" -> "剧集"
        else -> "通用视频"
    }
