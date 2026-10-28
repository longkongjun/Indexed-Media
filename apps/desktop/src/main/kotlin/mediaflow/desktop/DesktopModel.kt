package mediaflow.desktop

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.ProcessingTask
import mediaflow.api.models.ReviewCandidate
import mediaflow.api.models.ReviewCase
import mediaflow.api.models.SelectGenericVideoCommand
import mediaflow.api.models.SelectProviderCandidateCommand
import mediaflow.api.models.TaskDecisionReceipt
import mediaflow.client.core.SdkResult
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.events.TaskEventEnvelope
import mediaflow.client.core.paging.CachedPager
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.PageState
import mediaflow.client.core.review.ReviewRepository
import java.nio.file.Files
import java.nio.file.Path
import java.util.UUID

internal enum class DesktopWriteOperation { ReviewDecision, TaskCancel }

internal data class DesktopUi(
    val loggedIn: Boolean = false,
    val busy: Boolean = false,
    val message: String? = null,
    val wall: List<MediaItemSummary> = emptyList(),
    val wallState: PageState = PageState.Loading(false),
    val detail: MediaItemDetail? = null,
    val detailLoading: Boolean = false,
    val playbackUrl: String? = null,
    val tasks: List<ProcessingTask> = emptyList(),
    val taskState: PageState = PageState.Loading(false),
    val cases: List<ReviewCase> = emptyList(),
    val caseState: PageState = PageState.Loading(false),
    val selectedCase: ReviewCase? = null,
    val candidates: List<ReviewCandidate> = emptyList(),
    val receipt: TaskDecisionReceipt? = null,
    val writeBusy: Boolean = false,
    val writeUnknown: Boolean = false,
    val writeOperation: DesktopWriteOperation? = null,
)

/** Desktop owns page state; SDK repositories and system credential adapters remain separate. */
internal class DesktopModel(
    private val directory: Path,
    private val dispatcher: CoroutineDispatcher = Dispatchers.Main,
    private val storeFactory: (String) -> mediaflow.client.core.auth.DeviceAuthStore = ::secureStore,
    private val sessionFactory: (String, mediaflow.client.core.auth.DeviceAuthStore) -> DesktopSession = ::DesktopSession,
) {
    private val scope = CoroutineScope(SupervisorJob() + dispatcher)
    private val mutable = MutableStateFlow(DesktopUi())
    val state: StateFlow<DesktopUi> = mutable.asStateFlow()

    @Volatile private var session: DesktopSession? = null

    @Volatile private var generation = 0L
    private var detailGeneration = 0L
    private var caseGeneration = 0L
    private var sessionWork = SupervisorJob(scope.coroutineContext[Job])
    private var wall: CachedPager<MediaItemSummary>? = null
    private var tasks: CursorPager<ProcessingTask>? = null
    private var cases: CursorPager<ReviewCase>? = null
    private var pendingDecision: Triple<String, Long, ReviewRepository.ReviewDecisionKind>? = null
    private var pendingKey: String? = null
    private var pendingCancel: Pair<String, String>? = null

    private fun update(change: (DesktopUi) -> DesktopUi) {
        mutable.value = change(mutable.value)
    }

    fun dismissMessage() {
        update { it.copy(message = null) }
    }

    private fun action(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (
                cancelled: CancellationException,
            ) {
                throw cancelled
            } catch (
                _: Exception,
            ) {
                update { it.copy(busy = false, writeBusy = false, detailLoading = false, message = "操作未完成，请检查连接或系统存储权限后重试") }
            }
        }
    }

    suspend fun rememberedServer(): String =
        withContext(Dispatchers.IO) {
            val path = directory.resolve("server.txt")
            if (Files.exists(path)) runCatching { normalizeServer(Files.readString(path)) }.getOrDefault("") else ""
        }

    fun login(
        serverInput: String,
        pairing: Boolean,
        code: String,
        name: String,
        password: String,
        restore: Boolean = false,
    ) {
        if (state.value.busy || !scope.isActive) return
        update { it.copy(busy = true, message = null) }
        action {
            val server = normalizeServer(serverInput)
            val token = ++generation
            releaseSession()
            val store = storeFactory(server)
            val candidate = sessionFactory(server, store)
            try {
                val success =
                    if (restore) {
                        candidate.restore(store)
                    } else {
                        when (
                            val result =
                                if (pairing) {
                                    candidate.auth.enrollWithPairingCode(
                                        code,
                                        "MediaFlow Desktop",
                                    )
                                } else {
                                    candidate.auth.enrollWithCredentials(name, password, "MediaFlow Desktop")
                                }
                        ) {
                            is SdkResult.Success -> true
                            is SdkResult.Failure -> {
                                update {
                                    it.copy(
                                        message =
                                            if (candidate.authCache.current() !=
                                                null
                                            ) {
                                                "凭据未能安全保存，本次未进入会话；请检查系统安全存储后重试"
                                            } else {
                                                readable(result.error)
                                            },
                                    )
                                }
                                false
                            }
                        }
                    }
                if (!success || generation != token) {
                    candidate.close()
                    update { it.copy(busy = false) }
                    return@action
                }
                candidate.activate(directory.resolve("cache"))
                withContext(Dispatchers.IO) {
                    Files.createDirectories(directory)
                    Files.writeString(directory.resolve("server.txt"), server)
                }
                if (generation != token) {
                    candidate.close()
                    return@action
                }
                session = candidate
                mutable.value = DesktopUi(loggedIn = true)
                attach(candidate, token)
            } catch (failure: Exception) {
                if (session === candidate) {
                    ++generation
                    releaseSession()
                    mutable.value = DesktopUi()
                } else {
                    candidate.close()
                }
                throw failure
            }
        }
    }

    private suspend fun attach(
        current: DesktopSession,
        token: Long,
    ) {
        withContext(current.io) {
            wall = CachedPager(current.media.mediaWall(), current.cache::cachedWall, current.cache::replaceWall)
            tasks = current.tasks.tasks()
            cases = current.review.reviewQueue()
        }
        val w = wall!!
        val t = tasks!!
        val c = cases!!

        fun observe(block: suspend () -> Unit) {
            CoroutineScope(scope.coroutineContext + sessionWork).launch { block() }
        }
        observe { w.items.collect { if (generation == token) update { ui -> ui.copy(wall = it) } } }
        observe {
            w.state.collect {
                if (generation == token) {
                    update { ui -> ui.copy(wallState = it) }
                    checkPageAuth(it)
                }
            }
        }
        observe { t.items.collect { if (generation == token) update { ui -> ui.copy(tasks = it) } } }
        observe {
            t.state.collect {
                if (generation == token) {
                    update { ui -> ui.copy(taskState = it) }
                    checkPageAuth(it)
                }
            }
        }
        observe { c.items.collect { if (generation == token) update { ui -> ui.copy(cases = it) } } }
        observe {
            c.state.collect {
                if (generation == token) {
                    update { ui -> ui.copy(caseState = it) }
                    checkPageAuth(it)
                }
            }
        }
        observe {
            current.authExpired.collect { expired ->
                if (expired && generation == token && session === current) logout(expired = true)
            }
        }
        val events = withContext(current.io) { current.events() }
        observe {
            events.collect { event ->
                if (generation == token) {
                    when (event.type) {
                        TaskEventEnvelope.TYPE_CATALOG_MEDIA_CHANGED -> reloadWall()
                        TaskEventEnvelope.TYPE_PROCESSING_TASK_STATE_CHANGED, TaskEventEnvelope.TYPE_TASK_STATE_CHANGED -> reloadTasks()
                        TaskEventEnvelope.TYPE_REVIEW_CASE_UPDATED,
                        TaskEventEnvelope.TYPE_PROCESSING_TASK_IDENTIFICATION_DECIDED,
                        -> reloadCases()
                        TaskEventEnvelope.TYPE_STREAM_GAP -> refresh()
                    }
                }
            }
        }
        refresh()
    }

    private fun checkPageAuth(page: PageState) {
        if (page is PageState.Error && page.error.isCredentialExpired()) logout(expired = true)
    }

    private suspend fun <T> result(
        value: SdkResult<T>,
        current: DesktopSession,
        token: Long,
        success: (T) -> Unit,
    ) {
        if (session !== current || generation != token) return
        when (value) {
            is SdkResult.Success -> success(value.value)
            is SdkResult.Failure -> {
                if (value.error.isCredentialExpired()) {
                    logout(expired = true)
                } else {
                    update { it.copy(message = readable(value.error)) }
                }
            }
        }
    }

    private fun inSession(
        stillCurrent: () -> Boolean = { true },
        block: suspend (DesktopSession, Long) -> Unit,
    ) {
        val current = session ?: return
        val token = generation
        CoroutineScope(scope.coroutineContext + sessionWork).launch {
            try {
                if (current === session && token == generation && stillCurrent()) block(current, token)
            } catch (
                cancelled: CancellationException,
            ) {
                throw cancelled
            } catch (_: Exception) {
                if (current === session && token == generation && stillCurrent()) {
                    update {
                        it.copy(
                            writeBusy = false,
                            writeUnknown =
                                it.writeBusy && (pendingDecision != null || pendingCancel != null) || it.writeUnknown,
                            detailLoading = false,
                            message = "操作未完成，请检查连接或存储权限后重试",
                        )
                    }
                }
            }
        }
    }

    fun refresh() {
        reloadWall()
        reloadTasks()
        reloadCases()
        state.value.detail
            ?.item
            ?.id
            ?.let(::detail)
        reloadSelectedCase()
    }

    private fun reloadSelectedCase() {
        if (state.value.writeBusy || state.value.writeUnknown) return
        val previous = state.value.selectedCase ?: return
        val selected = ++caseGeneration
        inSession(stillCurrent = { selected == caseGeneration }) { s, token ->
            val loaded = s.review.reviewCase(previous.id)
            if (selected == caseGeneration && !state.value.writeBusy && !state.value.writeUnknown) {
                result(loaded, s, token) { current ->
                    update {
                        it.copy(
                            selectedCase = current,
                            candidates = if (previous.version == current.version) it.candidates else emptyList(),
                            receipt = if (previous.version == current.version) it.receipt else null,
                        )
                    }
                }
            }
        }
    }

    fun reloadWall(next: Boolean = false) {
        val pager = wall ?: return
        inSession { s, token ->
            withContext(s.io) {
                if (token == generation &&
                    s === session
                ) {
                    if (next) pager.loadNext() else pager.reload()
                }
            }
        }
    }

    fun reloadTasks(next: Boolean = false) {
        val pager = tasks ?: return
        inSession { s, token ->
            withContext(s.io) {
                if (token == generation &&
                    s === session
                ) {
                    if (next) pager.loadNext() else pager.reload()
                }
            }
        }
    }

    fun reloadCases(next: Boolean = false) {
        val pager = cases ?: return
        inSession { s, token ->
            withContext(s.io) {
                if (token == generation &&
                    s === session
                ) {
                    if (next) pager.loadNext() else pager.reload()
                }
            }
        }
    }

    fun detail(id: String) {
        val selected = ++detailGeneration
        update { it.copy(detail = null, detailLoading = true, playbackUrl = null) }
        inSession(stillCurrent = { selected == detailGeneration }) { s, token ->
            val cache = withContext(s.io) { s.cache.cachedDetail(id) }
            if (token == generation && selected == detailGeneration) update { it.copy(detail = cache) }
            val loaded = s.media.detail(id)
            if (selected != detailGeneration) return@inSession
            result(loaded, s, token) { detail -> update { it.copy(detail = detail, detailLoading = false) } }
            if (loaded is SdkResult.Success && token == generation) withContext(s.io) { s.cache.putDetail(loaded.value) }
            val playback = s.playback.externalPlaybackUrl()
            if (selected == detailGeneration) result(playback, s, token) { url -> update { it.copy(playbackUrl = url) } }
            if (selected == detailGeneration && token == generation) update { it.copy(detailLoading = false) }
        }
    }

    fun clearCase() {
        if (state.value.writeBusy || state.value.writeUnknown) return
        ++caseGeneration
        pendingDecision = null
        pendingKey = null
        update { it.copy(selectedCase = null, candidates = emptyList(), receipt = null, writeUnknown = false) }
    }

    fun selectCase(id: String) {
        if (state.value.writeBusy || state.value.writeUnknown) return
        val selected = ++caseGeneration
        pendingDecision = null
        pendingKey = null
        update { it.copy(selectedCase = null, candidates = emptyList(), receipt = null, writeUnknown = false) }
        inSession(stillCurrent = { selected == caseGeneration }) { s, token ->
            val loaded = s.review.reviewCase(id)
            if (selected == caseGeneration) result(loaded, s, token) { case -> update { it.copy(selectedCase = case) } }
        }
    }

    fun search(
        query: String,
        type: String,
    ) {
        if (state.value.writeBusy || state.value.writeUnknown) return
        val case = state.value.selectedCase ?: return
        val selected = ++caseGeneration
        update { it.copy(candidates = emptyList()) }
        inSession(stillCurrent = { selected == caseGeneration }) { s, token ->
            val loaded = s.review.searchCandidates(case.id, query, type, "zh-CN")
            if (selected == caseGeneration) result(loaded, s, token) { candidates -> update { it.copy(candidates = candidates) } }
        }
    }

    fun chooseCandidate(
        candidate: ReviewCandidate,
        expectedId: String? = null,
        expectedVersion: Long? = null,
    ) {
        val current = state.value.selectedCase ?: return
        if ((expectedId != null && current.id != expectedId) || (expectedVersion != null && current.version != expectedVersion)) return
        if (current.allowedActions.none { it.toString() == "select-provider-candidate" }) return
        if (candidate !in state.value.candidates) return
        submit(
            ReviewRepository.ReviewDecisionKind.SelectProviderCandidate(
                SelectProviderCandidateCommand(
                    kind = "select-provider-candidate",
                    provider = candidate.provider.toString(),
                    mediaType =
                        if (candidate.mediaType.toString() ==
                            "tv"
                        ) {
                            SelectProviderCandidateCommand.MediaType.tv
                        } else {
                            SelectProviderCandidateCommand.MediaType.movie
                        },
                    providerId = candidate.providerId,
                    saveFeedback = false,
                ),
            ),
        )
    }

    fun chooseGeneric(
        expectedId: String? = null,
        expectedVersion: Long? = null,
    ) {
        val case = state.value.selectedCase ?: return
        if ((expectedId != null && case.id != expectedId) || (expectedVersion != null && case.version != expectedVersion)) return
        if (case.allowedActions.none { it.toString() == "select-generic-video" }) return
        submit(
            ReviewRepository.ReviewDecisionKind.SelectGenericVideo(
                SelectGenericVideoCommand(
                    "select-generic-video",
                    case.titleHint ?: case.relativePath,
                    null,
                    false,
                ),
            ),
        )
    }

    private fun submit(command: ReviewRepository.ReviewDecisionKind) {
        val case = state.value.selectedCase ?: return
        if (state.value.writeBusy || state.value.receipt != null || state.value.writeUnknown) return
        ++caseGeneration
        pendingDecision = Triple(case.id, case.version, command)
        pendingKey = "desktop-${UUID.randomUUID()}"
        executeDecision()
    }

    fun retryDecision() {
        if (state.value.writeUnknown &&
            !state.value.writeBusy &&
            state.value.writeOperation == DesktopWriteOperation.ReviewDecision
        ) {
            executeDecision()
        }
    }

    private fun executeDecision() {
        val pending = pendingDecision ?: return
        val key = pendingKey ?: return
        val selected = caseGeneration
        update { it.copy(writeBusy = true, writeOperation = DesktopWriteOperation.ReviewDecision) }
        inSession { s, token ->
            val response = s.review.submitDecision(pending.first, pending.second, key, pending.third)
            if (token != generation || selected != caseGeneration) return@inSession
            update { it.copy(writeBusy = false) }
            when (response) {
                is SdkResult.Success -> {
                    pendingDecision = null
                    pendingKey = null
                    update { it.copy(receipt = response.value, writeUnknown = false, writeOperation = null) }
                    reloadCases()
                    reloadTasks()
                }
                is SdkResult.Failure -> {
                    val error = response.error
                    val unknown = error !is MediaFlowError.Http || error.status >= 500 || error.status == 408
                    if (!unknown) {
                        pendingDecision = null
                        pendingKey = null
                    }
                    update {
                        it.copy(
                            writeUnknown = unknown,
                            writeOperation = if (unknown) DesktopWriteOperation.ReviewDecision else null,
                            message = readable(error),
                        )
                    }
                    if ((response.error as? MediaFlowError.Http)?.status == 409) selectCase(pending.first)
                    if (response.error.isCredentialExpired()) logout(expired = true)
                }
            }
        }
    }

    fun cancelTask(task: ProcessingTask) {
        if (state.value.writeBusy || state.value.writeUnknown || task.allowedActions.none { it.toString() == "cancel" }) return
        pendingCancel = task.id to "desktop-cancel-${UUID.randomUUID()}"
        executeTaskCancel()
    }

    fun retryTaskCancel() {
        if (state.value.writeUnknown &&
            !state.value.writeBusy &&
            state.value.writeOperation == DesktopWriteOperation.TaskCancel
        ) {
            executeTaskCancel()
        }
    }

    private fun executeTaskCancel() {
        val pending = pendingCancel ?: return
        update { it.copy(writeBusy = true, writeOperation = DesktopWriteOperation.TaskCancel) }
        inSession { s, token ->
            val response = s.tasks.cancel(pending.first, pending.second)
            if (generation != token || s !== session) return@inSession
            when (response) {
                is SdkResult.Success -> {
                    pendingCancel = null
                    update { it.copy(writeBusy = false, writeUnknown = false, writeOperation = null) }
                }
                is SdkResult.Failure -> {
                    val error = response.error
                    val unknown = error !is MediaFlowError.Http || error.status >= 500 || error.status == 408
                    if (!unknown) pendingCancel = null
                    update {
                        it.copy(
                            writeBusy = false,
                            writeUnknown = unknown,
                            writeOperation = if (unknown) DesktopWriteOperation.TaskCancel else null,
                            message = readable(error),
                        )
                    }
                    if (error.isCredentialExpired()) logout(expired = true)
                }
            }
            reloadTasks()
        }
    }

    fun logout(expired: Boolean = false) {
        if (!state.value.loggedIn || state.value.busy) return
        update { it.copy(busy = true) }
        action {
            ++generation
            val previous = session
            mutable.value = DesktopUi(busy = true, message = if (expired) "登录已失效，请重新登录" else null)
            try {
                if (expired) previous?.auth?.discardLocalCredentials() else previous?.auth?.logout()
            } catch (cancelled: CancellationException) {
                withContext(NonCancellable) { previous?.auth?.discardLocalCredentials() }
                throw cancelled
            } finally {
                withContext(NonCancellable) {
                    releaseSession()
                    update { it.copy(busy = false) }
                }
            }
        }
    }

    private suspend fun releaseSession() =
        withContext(NonCancellable) {
            val work = sessionWork
            sessionWork = SupervisorJob(scope.coroutineContext[Job])
            val previous = session
            session = null
            wall = null
            tasks = null
            cases = null
            pendingDecision = null
            pendingKey = null
            pendingCancel = null
            work.cancelAndJoin()
            previous?.close()
        }

    suspend fun close() =
        withContext(NonCancellable + dispatcher) {
            ++generation
            scope.cancel()
            releaseSession()
            scope.coroutineContext[Job]?.join()
            mutable.value = DesktopUi()
        }
}

internal fun readable(error: MediaFlowError): String =
    when (error) {
        is MediaFlowError.Http ->
            when (error.status) {
                401 -> "登录已失效，请重新登录"
                403 -> "没有执行此操作的权限"
                409 -> "内容已变化，请检查最新内容后重新确认"
                429 -> "操作过于频繁，请稍后重试"
                else -> "服务请求失败（${error.status}），请刷新核对当前结果"
            }
        is MediaFlowError.Network -> "网络请求未完成；写操作结果可能未知，请先核对或重试同一操作"
        is MediaFlowError.Serialization -> "服务器数据格式不兼容"
        is MediaFlowError.Unexpected -> "操作未完成，请检查服务器或系统凭据存储"
    }
