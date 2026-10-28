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
import mediaflow.api.models.MediaLibrary
import mediaflow.api.models.ProcessingTask
import mediaflow.api.models.ReviewCandidate
import mediaflow.api.models.ReviewCase
import mediaflow.api.models.SelectGenericVideoCommand
import mediaflow.api.models.SelectProviderCandidateCommand
import mediaflow.api.models.TaskDecisionReceipt
import mediaflow.client.core.SdkResult
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.events.TaskEventEnvelope
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.paging.CachedPager
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.PageState
import mediaflow.client.core.review.ReviewRepository
import mediaflow.client.core.tasks.TaskFilter
import java.nio.file.Files
import java.nio.file.Path
import java.util.UUID

internal enum class DesktopWriteOperation { ReviewDecision, TaskCancel, TaskRetry }

private data class PendingTaskWrite(
    val id: String,
    val key: String,
    val operation: DesktopWriteOperation,
)

internal data class DesktopUi(
    val loggedIn: Boolean = false,
    val sessionEpoch: Long = 0,
    val busy: Boolean = false,
    val message: String? = null,
    val wall: List<MediaItemSummary> = emptyList(),
    val wallState: PageState = PageState.Loading(false),
    val wallHasMore: Boolean = false,
    val mediaFilter: MediaFilter = MediaFilter(),
    val mediaLibraries: List<MediaLibrary> = emptyList(),
    val librariesLoading: Boolean = false,
    val libraryMessage: String? = null,
    val detail: MediaItemDetail? = null,
    val detailLoading: Boolean = false,
    val playbackUrl: String? = null,
    val tasks: List<ProcessingTask> = emptyList(),
    val taskState: PageState = PageState.Loading(false),
    val taskHasMore: Boolean = false,
    val taskFilter: TaskFilter = TaskFilter(),
    val selectedTaskId: String? = null,
    val selectedTask: ProcessingTask? = null,
    val taskDetailLoading: Boolean = false,
    val taskDetailMessage: String? = null,
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
    private var wallGeneration = 0L
    private var taskGeneration = 0L
    private var taskDetailGeneration = 0L
    private var sessionWork = SupervisorJob(scope.coroutineContext[Job])
    private var wallWork: Job? = null
    private var taskWork: Job? = null
    private var wall: CachedPager<MediaItemSummary>? = null
    private var tasks: CursorPager<ProcessingTask>? = null
    private var cases: CursorPager<ReviewCase>? = null
    private var pendingDecision: Triple<String, Long, ReviewRepository.ReviewDecisionKind>? = null
    private var pendingKey: String? = null
    private var pendingTaskWrite: PendingTaskWrite? = null

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
                mutable.value = DesktopUi(loggedIn = true, sessionEpoch = token)
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
        val owner = sessionWork
        bindWall(current, token, state.value.mediaFilter, ++wallGeneration)
        if (session !== current || generation != token) return
        bindTasks(current, token, state.value.taskFilter, ++taskGeneration)
        if (session !== current || generation != token) return
        val c = withContext(current.io) { current.review.reviewQueue() }
        if (session !== current || generation != token) return
        cases = c

        fun observeSession(block: suspend () -> Unit) {
            observe(owner, block)
        }
        observeSession { c.items.collect { if (generation == token) update { ui -> ui.copy(cases = it) } } }
        observeSession {
            c.state.collect {
                if (generation == token) {
                    update { ui -> ui.copy(caseState = it) }
                    checkPageAuth(it)
                }
            }
        }
        observeSession {
            current.authExpired.collect { expired ->
                if (expired && generation == token && session === current) logout(expired = true)
            }
        }
        val events = withContext(current.io) { current.events() }
        if (session !== current || generation != token) return
        observeSession {
            events.collect { event ->
                if (generation == token) {
                    when (event.type) {
                        TaskEventEnvelope.TYPE_CATALOG_MEDIA_CHANGED -> {
                            reloadWall()
                            loadMediaLibraries()
                        }
                        TaskEventEnvelope.TYPE_PROCESSING_TASK_STATE_CHANGED, TaskEventEnvelope.TYPE_TASK_STATE_CHANGED -> {
                            reloadTasks()
                            reloadSelectedTask()
                        }
                        TaskEventEnvelope.TYPE_TASK_DECISION_ACCEPTED,
                        TaskEventEnvelope.TYPE_PROCESSING_TASK_IDENTIFICATION_DECIDED,
                        -> {
                            reloadCases()
                            reloadTasks()
                            reloadSelectedTask()
                        }
                        TaskEventEnvelope.TYPE_REVIEW_CASE_UPDATED -> reloadCases()
                        TaskEventEnvelope.TYPE_STREAM_GAP -> refresh()
                    }
                }
            }
        }
        refresh()
    }

    private fun observe(
        work: Job,
        block: suspend () -> Unit,
    ) {
        CoroutineScope(scope.coroutineContext + work).launch { block() }
    }

    private suspend fun bindWall(
        current: DesktopSession,
        token: Long,
        filter: MediaFilter,
        selected: Long,
    ) {
        val previous = wallWork
        wallWork = null
        wall = null
        previous?.cancelAndJoin()
        if (session !== current || generation != token || wallGeneration != selected) return
        val pager =
            withContext(current.io) {
                if (filter == MediaFilter()) {
                    CachedPager(current.media.mediaWall(filter), current.cache::cachedWall, current.cache::replaceWall)
                } else {
                    // Searches retain stale results only in their own pager; the default offline wall remains separate.
                    CachedPager(current.media.mediaWall(filter), { emptyList() }, {})
                }
            }
        if (session !== current || generation != token || wallGeneration != selected) return
        val work = SupervisorJob(sessionWork)
        wall = pager
        wallWork = work
        observe(work) {
            pager.items.collect {
                if (generation == token && wallGeneration == selected) update { ui -> ui.copy(wall = it, wallHasMore = pager.hasMore()) }
            }
        }
        observe(work) {
            pager.state.collect {
                if (generation == token && wallGeneration == selected) {
                    update { ui -> ui.copy(wallState = it, wallHasMore = pager.hasMore()) }
                    checkPageAuth(it)
                }
            }
        }
    }

    private suspend fun bindTasks(
        current: DesktopSession,
        token: Long,
        filter: TaskFilter,
        selected: Long,
    ) {
        val previous = taskWork
        taskWork = null
        tasks = null
        previous?.cancelAndJoin()
        if (session !== current || generation != token || taskGeneration != selected) return
        val pager = withContext(current.io) { current.tasks.tasks(filter = filter) }
        if (session !== current || generation != token || taskGeneration != selected) return
        val work = SupervisorJob(sessionWork)
        tasks = pager
        taskWork = work
        observe(work) {
            pager.items.collect {
                if (generation == token && taskGeneration == selected) update { ui -> ui.copy(tasks = it, taskHasMore = pager.hasMore()) }
            }
        }
        observe(work) {
            pager.state.collect {
                if (generation == token && taskGeneration == selected) {
                    update { ui -> ui.copy(taskState = it, taskHasMore = pager.hasMore()) }
                    checkPageAuth(it)
                }
            }
        }
    }

    fun applyMediaFilter(filter: MediaFilter) {
        if (!state.value.loggedIn) return
        val normalized = filter.copy(query = filter.query?.trim()?.takeIf { it.isNotEmpty() })
        if (normalized == state.value.mediaFilter) return
        val selected = ++wallGeneration
        ++detailGeneration
        update {
            it.copy(
                mediaFilter = normalized,
                wall = emptyList(),
                wallState = PageState.Loading(false),
                wallHasMore = false,
                detail = null,
                detailLoading = false,
                playbackUrl = null,
            )
        }
        inSession(stillCurrent = { wallGeneration == selected }) { current, token ->
            bindWall(current, token, normalized, selected)
            if (wallGeneration == selected) reloadWall()
        }
    }

    fun applyTaskFilter(filter: TaskFilter) {
        if (!state.value.loggedIn || state.value.writeBusy || state.value.writeUnknown) return
        val normalized = filter.copy(query = filter.query?.trim()?.takeIf { it.isNotEmpty() })
        if (normalized == state.value.taskFilter) return
        val selected = ++taskGeneration
        clearTask()
        update { it.copy(taskFilter = normalized, tasks = emptyList(), taskState = PageState.Loading(false), taskHasMore = false) }
        inSession(stillCurrent = { taskGeneration == selected }) { current, token ->
            bindTasks(current, token, normalized, selected)
            if (taskGeneration == selected) reloadTasks()
        }
    }

    fun loadMediaLibraries() {
        if (!state.value.loggedIn) return
        update { it.copy(librariesLoading = true, libraryMessage = null) }
        inSession { current, token ->
            when (val loaded = current.media.libraries()) {
                is SdkResult.Success ->
                    if (session === current && token == generation) {
                        update { it.copy(mediaLibraries = loaded.value, librariesLoading = false) }
                    }
                is SdkResult.Failure ->
                    if (session === current && token == generation) {
                        update { it.copy(librariesLoading = false, libraryMessage = "媒体库筛选暂不可用：${readable(loaded.error)}") }
                        if (loaded.error.isCredentialExpired()) logout(expired = true)
                    }
            }
        }
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
        work: Job = sessionWork,
        stillCurrent: () -> Boolean = { true },
        block: suspend (DesktopSession, Long) -> Unit,
    ) {
        val current = session ?: return
        val token = generation
        CoroutineScope(scope.coroutineContext + work).launch {
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
                                it.writeBusy && (pendingDecision != null || pendingTaskWrite != null) || it.writeUnknown,
                            detailLoading = false,
                            message = "操作未完成，请检查连接或存储权限后重试",
                        )
                    }
                }
            }
        }
    }

    fun refresh() {
        loadMediaLibraries()
        reloadWall()
        reloadTasks()
        reloadCases()
        state.value.detail
            ?.item
            ?.id
            ?.let(::detail)
        reloadSelectedCase()
        reloadSelectedTask()
    }

    private fun reloadSelectedTask() {
        if (!state.value.writeBusy) state.value.selectedTaskId?.let(::selectTask)
    }

    fun selectTask(id: String) {
        if (state.value.writeBusy || (state.value.writeUnknown && state.value.selectedTaskId != id)) return
        val selected = ++taskDetailGeneration
        update {
            it.copy(
                selectedTaskId = id,
                selectedTask = it.selectedTask?.takeIf { task -> task.id == id },
                taskDetailLoading = true,
                taskDetailMessage = null,
            )
        }
        inSession(stillCurrent = { taskDetailGeneration == selected }) { current, token ->
            val loaded = current.tasks.detail(id)
            if (selected != taskDetailGeneration || current !== session || token != generation) return@inSession
            when (loaded) {
                is SdkResult.Success -> update { it.copy(selectedTask = loaded.value, taskDetailLoading = false) }
                is SdkResult.Failure -> {
                    update { it.copy(taskDetailLoading = false, taskDetailMessage = readable(loaded.error)) }
                    if (loaded.error.isCredentialExpired()) logout(expired = true)
                }
            }
        }
    }

    fun clearTask() {
        if (state.value.writeBusy || state.value.writeUnknown) return
        ++taskDetailGeneration
        update { it.copy(selectedTaskId = null, selectedTask = null, taskDetailLoading = false, taskDetailMessage = null) }
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
        val work = wallWork ?: return
        val selected = wallGeneration
        inSession(work = work, stillCurrent = { selected == wallGeneration && pager === wall }) { s, token ->
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
        val work = taskWork ?: return
        val selected = taskGeneration
        inSession(work = work, stillCurrent = { selected == taskGeneration && pager === tasks }) { s, token ->
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

    suspend fun artwork(id: String): mediaflow.client.core.media.Artwork? {
        val (current, token) = withContext(dispatcher) { session to generation }
        val active = current ?: return null
        val response = active.artwork.poster(id)
        return withContext(dispatcher) {
            if (generation != token || session !== active) return@withContext null
            when (response) {
                is SdkResult.Success -> response.value
                is SdkResult.Failure -> {
                    if (response.error.isCredentialExpired()) logout(expired = true)
                    null
                }
            }
        }
    }

    fun detail(id: String) {
        val selected = ++detailGeneration
        update { it.copy(detail = it.detail?.takeIf { detail -> detail.item.id == id }, detailLoading = true, playbackUrl = null) }
        inSession(stillCurrent = { selected == detailGeneration }) { s, token ->
            val cache = withContext(s.io) { s.cache.cachedDetail(id) }
            if (token == generation && selected == detailGeneration) update { it.copy(detail = it.detail ?: cache) }
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
                    reloadSelectedTask()
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

    fun cancelTask(task: ProcessingTask) = startTaskWrite(task, DesktopWriteOperation.TaskCancel)

    fun retryTask(task: ProcessingTask) = startTaskWrite(task, DesktopWriteOperation.TaskRetry)

    private fun startTaskWrite(
        task: ProcessingTask,
        operation: DesktopWriteOperation,
    ) {
        if (state.value.writeBusy || state.value.writeUnknown) return
        val current =
            state.value.selectedTask?.takeIf { it.id == task.id }
                ?: state.value.tasks.firstOrNull { it.id == task.id } ?: return
        val action = if (operation == DesktopWriteOperation.TaskRetry) "retry" else "cancel"
        if (current.allowedActions.none { it.toString() == action }) return
        ++taskDetailGeneration
        pendingTaskWrite = PendingTaskWrite(task.id, "desktop-$action-${UUID.randomUUID()}", operation)
        update { it.copy(selectedTaskId = current.id, selectedTask = current, taskDetailMessage = null) }
        executeTaskWrite()
    }

    fun retryTaskCancel() = retryTaskWrite(DesktopWriteOperation.TaskCancel)

    fun retryTaskRetry() = retryTaskWrite(DesktopWriteOperation.TaskRetry)

    private fun retryTaskWrite(operation: DesktopWriteOperation) {
        if (state.value.writeUnknown && !state.value.writeBusy && state.value.writeOperation == operation) executeTaskWrite()
    }

    private fun executeTaskWrite() {
        val pending = pendingTaskWrite ?: return
        update { it.copy(writeBusy = true, writeOperation = pending.operation, taskDetailLoading = false) }
        inSession { s, token ->
            val response =
                if (pending.operation == DesktopWriteOperation.TaskRetry) {
                    s.tasks.retry(pending.id, pending.key)
                } else {
                    s.tasks.cancel(pending.id, pending.key)
                }
            if (generation != token || s !== session || pendingTaskWrite != pending) return@inSession
            when (response) {
                is SdkResult.Success -> {
                    pendingTaskWrite = null
                    ++taskDetailGeneration
                    update {
                        it.copy(
                            writeBusy = false,
                            writeUnknown = false,
                            writeOperation = null,
                            selectedTask = if (it.selectedTaskId == response.value.id) response.value else it.selectedTask,
                            taskDetailLoading = false,
                            message = null,
                        )
                    }
                }
                is SdkResult.Failure -> {
                    val error = response.error
                    val unknown = error !is MediaFlowError.Http || error.status >= 500 || error.status == 408
                    if (!unknown) pendingTaskWrite = null
                    update {
                        it.copy(
                            writeBusy = false,
                            writeUnknown = unknown,
                            writeOperation = if (unknown) pending.operation else null,
                            message = readable(error),
                        )
                    }
                    if (error.isCredentialExpired()) logout(expired = true)
                }
            }
            reloadTasks()
            reloadSelectedTask()
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
            pendingTaskWrite = null
            wallWork = null
            taskWork = null
            ++wallGeneration
            ++taskGeneration
            ++taskDetailGeneration
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
