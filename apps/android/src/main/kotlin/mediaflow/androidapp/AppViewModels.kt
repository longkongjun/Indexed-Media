package mediaflow.androidapp

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import java.util.UUID
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.ProcessingTask
import mediaflow.api.models.ReviewCase
import mediaflow.client.core.SdkResult
import mediaflow.client.core.getOrNull
import mediaflow.client.core.events.ProjectionRefresher
import mediaflow.client.core.events.TaskEventEnvelope
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.PageState

/** 登录屏状态机：双模式 enrollment（配对码 / 管理员凭据），统一产设备令牌。 */
class LoginViewModel(private val container: SdkContainer) : ViewModel() {
    val busy = MutableStateFlow(false)
    val error = MutableStateFlow<String?>(null)
    val loggedIn = MutableStateFlow(false)

    /** 配对码模式：Web 管理面签发的一次性码 → 设备令牌。 */
    fun pair(baseUrl: String, pairingCode: String, deviceName: String) {
        enroll(baseUrl) { container.authRepository.enrollWithPairingCode(pairingCode, deviceName) }
    }

    /** 凭据模式：管理员密码直接 enrollment（走登录限流）→ 设备令牌。 */
    fun login(baseUrl: String, name: String, password: String, deviceName: String) {
        enroll(baseUrl) { container.authRepository.enrollWithCredentials(name, password, deviceName) }
    }

    /** 任意屏 401 后的统一出口：清凭据回登录页。 */
    fun forceLoggedOut() {
        loggedIn.value = false
    }

    /** 冷启动恢复成功：跳过登录页直接进入六屏。 */
    fun markRestored() {
        loggedIn.value = true
    }

    private fun enroll(baseUrl: String, call: suspend () -> SdkResult<mediaflow.client.core.auth.StoredCredentials>) {
        if (busy.value) return
        busy.value = true
        error.value = null
        viewModelScope.launch {
            container.configure(baseUrl)
            when (val result = call()) {
                is SdkResult.Success -> {
                    container.rememberBaseUrl(baseUrl)
                    loggedIn.value = true
                }
                is SdkResult.Failure -> error.value = PageVisual.readableMessage(result.error)
            }
            busy.value = false
        }
    }
}

/** 媒体墙：分页 + SSE 权威刷新 + 缓存预热（离线首屏）。 */
class WallViewModel(private val container: SdkContainer) : ViewModel() {
    private var pager: CursorPager<MediaItemSummary>? = null
    private var refresher: Job? = null

    val state: MutableStateFlow<PageState> = MutableStateFlow(PageState.Loading(false))
    val items: MutableStateFlow<List<MediaItemSummary>> = MutableStateFlow(emptyList())
    val connection = MutableStateFlow("idle")

    fun start() {
        if (pager != null) return
        val p = container.mediaRepository.mediaWall()
        pager = p
        viewModelScope.launch {
            // 先给离线快照（可能为空），再权威刷新；刷新成功后写缓存。
            items.value = container.mediaCache.cachedWall()
            p.reload()
        }
        viewModelScope.launch {
            p.state.collect { state.value = it }
        }
        viewModelScope.launch {
            p.items.collect { list ->
                items.value = list
                if (list.isNotEmpty()) container.mediaCache.replaceWall(list)
            }
        }
        container.eventClientOrNull()?.let { client ->
            viewModelScope.launch { client.connection.collect { connection.value = it.name.lowercase() } }
            refresher =
                ProjectionRefresher(client.events())
                    .start(
                        viewModelScope,
                        ProjectionRefresher.Route("wall", setOf(TaskEventEnvelope.TYPE_CATALOG_MEDIA_CHANGED)) {
                            p.reload()
                        },
                    )
        }
    }

    fun loadNext() {
        viewModelScope.launch { pager?.loadNext() }
    }

    fun reload() {
        viewModelScope.launch { pager?.reload() }
    }
}

/** 详情：单条快照 + 外部播放入口。 */
class DetailViewModel(private val container: SdkContainer) : ViewModel() {
    data class Ui(
        val loading: Boolean = true,
        val detail: MediaItemDetail? = null,
        val playbackUrl: String? = null,
        val playbackAvailable: Boolean = false,
        val error: String? = null,
        val offline: Boolean = false,
    )

    val ui = MutableStateFlow(Ui())

    fun load(mediaItemId: String) {
        ui.value = Ui()
        viewModelScope.launch {
            val cached = container.mediaCache.cachedDetail(mediaItemId)
            if (cached != null) ui.value = ui.value.copy(loading = false, detail = cached)
            when (val result = container.mediaRepository.detail(mediaItemId)) {
                is SdkResult.Success -> {
                    val d = result.value
                    container.mediaCache.putDetail(d)
                    val url = container.playbackRepository.externalPlaybackUrl().getOrNull()
                    ui.value =
                        Ui(
                            loading = false,
                            detail = d,
                            playbackUrl = url,
                            playbackAvailable = url != null,
                        )
                }
                is SdkResult.Failure -> {
                    container.handleAuthFailure(result)
                    ui.value =
                        ui.value.copy(
                            loading = false,
                            error = PageVisual.readableMessage(result.error),
                            offline = result.error.isOffline(),
                        )
                }
            }
        }
    }
}

/** 人工确认：案件列表 → 单案确认（候选搜索 + 提交决定）。 */
class ReviewQueueViewModel(private val container: SdkContainer) : ViewModel() {
    private var pager: CursorPager<ReviewCase>? = null

    val state = MutableStateFlow<PageState>(PageState.Loading(false))
    val cases = MutableStateFlow<List<ReviewCase>>(emptyList())

    fun start() {
        if (pager != null) return
        val p = container.reviewRepository.reviewQueue()
        pager = p
        viewModelScope.launch { p.reload() }
        viewModelScope.launch { p.state.collect { state.value = it } }
        viewModelScope.launch { p.items.collect { cases.value = it } }
        container.eventClientOrNull()?.let { client ->
            ProjectionRefresher(client.events())
                .start(
                    viewModelScope,
                    ProjectionRefresher.Route(
                        "review",
                        setOf(TaskEventEnvelope.TYPE_REVIEW_CASE_UPDATED, TaskEventEnvelope.TYPE_PROCESSING_TASK_IDENTIFICATION_DECIDED),
                    ) {
                        p.reload()
                    },
                )
        }
    }

    fun reload() {
        viewModelScope.launch { pager?.reload() }
    }
}

class ConfirmViewModel(private val container: SdkContainer) : ViewModel() {
    data class Ui(
        val case: ReviewCase? = null,
        val loading: Boolean = true,
        val candidates: List<mediaflow.api.models.ReviewCandidate> = emptyList(),
        val searching: Boolean = false,
        val submitting: Boolean = false,
        val submitted: mediaflow.api.models.TaskDecisionReceipt? = null,
        val error: String? = null,
    )

    val ui = MutableStateFlow(Ui())

    /** 候选搜索的媒体类型（案件不带该字段，由用户在确认页切换；默认电影）。 */
    val mediaType = MutableStateFlow("movie")

    fun load(reviewCaseId: String) {
        if (ui.value.case?.id == reviewCaseId) return
        viewModelScope.launch {
            when (val result = container.reviewRepository.reviewCase(reviewCaseId)) {
                is SdkResult.Success -> ui.value = Ui(loading = false, case = result.value)
                is SdkResult.Failure -> {
                    container.handleAuthFailure(result)
                    ui.value = Ui(loading = false, error = PageVisual.readableMessage(result.error))
                }
            }
        }
    }

    /** 候选搜索（locale 固定 zh-CN：第一批中文家庭场景）。 */
    fun search(reviewCaseId: String, query: String) {
        if (ui.value.case == null || query.isBlank()) return
        viewModelScope.launch {
            ui.value = ui.value.copy(searching = true, candidates = emptyList())
            val result = container.reviewRepository.searchCandidates(reviewCaseId, query, mediaType.value, "zh-CN")
            ui.value =
                when (result) {
                    is SdkResult.Success -> ui.value.copy(searching = false, candidates = result.value)
                    is SdkResult.Failure -> {
                        container.handleAuthFailure(result)
                        ui.value.copy(searching = false, error = PageVisual.readableMessage(result.error))
                    }
                }
        }
    }

    /** 提交决定：幂等键一次性生成（重试语义由服务端重放收敛）。 */
    fun submitSelectCandidate(candidateIndex: Int) {
        val case = ui.value.case ?: return
        val candidate = ui.value.candidates.getOrNull(candidateIndex) ?: return
        submit("select-provider-candidate", case) {
            mediaflow.client.core.review.ReviewRepository.ReviewDecisionKind.SelectProviderCandidate(
                mediaflow.api.models.SelectProviderCandidateCommand(
                    kind = "select-provider-candidate",
                    provider = candidate.provider.toString(),
                    mediaType =
                        if (candidate.mediaType.toString() == "tv") {
                            mediaflow.api.models.SelectProviderCandidateCommand.MediaType.tv
                        } else {
                            mediaflow.api.models.SelectProviderCandidateCommand.MediaType.movie
                        },
                    providerId = candidate.providerId,
                    saveFeedback = false,
                ),
            )
        }
    }

    fun submitGenericVideo() {
        val case = ui.value.case ?: return
        submit("select-generic-video", case) {
            mediaflow.client.core.review.ReviewRepository.ReviewDecisionKind.SelectGenericVideo(
                mediaflow.api.models.SelectGenericVideoCommand(
                    kind = "select-generic-video",
                    displayTitle = case.titleHint ?: case.relativePath,
                    groupHint = null,
                    saveGroupingFeedback = false,
                ),
            )
        }
    }

    private fun submit(
        kindLabel: String,
        case: ReviewCase,
        command: () -> mediaflow.client.core.review.ReviewRepository.ReviewDecisionKind,
    ) {
        if (ui.value.submitting || ui.value.submitted != null) return
        ui.value = ui.value.copy(submitting = true, error = null)
        viewModelScope.launch {
            val result =
                container.reviewRepository.submitDecision(
                    reviewCaseId = case.id,
                    caseVersion = case.version,
                    idempotencyKey = "android-${kindLabel}-${UUID.randomUUID()}",
                    command = command(),
                )
            ui.value =
                when (result) {
                    is SdkResult.Success -> ui.value.copy(submitting = false, submitted = result.value)
                    is SdkResult.Failure -> {
                        container.handleAuthFailure(result)
                        ui.value.copy(submitting = false, error = PageVisual.readableMessage(result.error))
                    }
                }
        }
    }
}

/** 任务中心：处理任务列表 + 取消。 */
class TasksViewModel(private val container: SdkContainer) : ViewModel() {
    private var pager: CursorPager<ProcessingTask>? = null

    val state = MutableStateFlow<PageState>(PageState.Loading(false))
    val tasks = MutableStateFlow<List<ProcessingTask>>(emptyList())
    val cancelling = MutableStateFlow<Set<String>>(emptySet())
    val error = MutableStateFlow<String?>(null)

    fun start() {
        if (pager != null) return
        val p = container.taskRepository.tasks()
        pager = p
        viewModelScope.launch { p.reload() }
        viewModelScope.launch { p.state.collect { state.value = it } }
        viewModelScope.launch { p.items.collect { tasks.value = it } }
        container.eventClientOrNull()?.let { client ->
            ProjectionRefresher(client.events())
                .start(
                    viewModelScope,
                    ProjectionRefresher.Route(
                        "tasks",
                        setOf(
                            TaskEventEnvelope.TYPE_PROCESSING_TASK_STATE_CHANGED,
                            TaskEventEnvelope.TYPE_TASK_STATE_CHANGED,
                        ),
                    ) {
                        p.reload()
                    },
                )
        }
    }

    fun cancel(taskId: String) {
        if (taskId in cancelling.value) return
        cancelling.value = cancelling.value + taskId
        viewModelScope.launch {
            when (val result = container.taskRepository.cancel(taskId, "android-cancel-${UUID.randomUUID()}")) {
                is SdkResult.Success -> pager?.reload()
                is SdkResult.Failure -> {
                    container.handleAuthFailure(result)
                    error.value = PageVisual.readableMessage(result.error)
                }
            }
            cancelling.value = cancelling.value - taskId
        }
    }

    fun dismissError() {
        error.value = null
    }
}

/** 简单工厂（无依赖注入框架；参数即依赖）。 */
class VmFactory(private val container: SdkContainer) : ViewModelProvider.Factory {
    override fun <T : ViewModel> create(modelClass: Class<T>): T =
        when (modelClass.simpleName) {
            "LoginViewModel" -> LoginViewModel(container)
            "WallViewModel" -> WallViewModel(container)
            "DetailViewModel" -> DetailViewModel(container)
            "ReviewQueueViewModel" -> ReviewQueueViewModel(container)
            "ConfirmViewModel" -> ConfirmViewModel(container)
            "TasksViewModel" -> TasksViewModel(container)
            else -> throw IllegalArgumentException("unknown view model: ${modelClass.name}")
        } as T
}
