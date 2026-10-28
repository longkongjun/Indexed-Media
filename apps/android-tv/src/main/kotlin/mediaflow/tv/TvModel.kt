package mediaflow.tv

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.MediaLibrary
import mediaflow.client.core.SdkResult
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.media.Artwork
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.PageState
import java.net.URI

internal data class TvUi(
    val connected: Boolean = false,
    val connecting: Boolean = false,
    val server: String = "",
    val identity: Long = 0,
    val deviceName: String? = null,
    val artworkEpoch: Long = 0,
    val filter: MediaFilter = MediaFilter(),
    val wall: List<MediaItemSummary> = emptyList(),
    val page: PageState = PageState.Loading(false),
    val hasMore: Boolean = false,
    val libraries: List<MediaLibrary> = emptyList(),
    val detail: MediaItemDetail? = null,
    val detailMediaId: String? = null,
    val detailPath: TvDetailPath = TvDetailPath(),
    val detailLoading: Boolean = false,
    val detailOpen: Boolean = false,
    val playbackUrl: String? = null,
    val message: String? = null,
)

/** TV页面状态所有者；身份/筛选/详情代次分别拒绝晚到结果，图片不进入业务状态。 */
internal class TvModel(
    private val scope: CoroutineScope,
    private val sessionFactory: (String) -> TvSession,
    private val rememberServer: (String) -> Unit = {},
    initialTheme: TvTheme = TvTheme.System,
    private val rememberTheme: (TvTheme) -> Unit = {},
) : AutoCloseable {
    private val mutable = MutableStateFlow(TvUi())
    val state = mutable.asStateFlow()
    private val mutableTheme = MutableStateFlow(initialTheme)
    val theme = mutableTheme.asStateFlow()
    private var session: TvSession? = null
    private var generation = 0L
    private var pageGeneration = 0L
    private var detailGeneration = 0L
    private var connectionJob: Job? = null
    private var sessionWork = SupervisorJob(scope.coroutineContext[Job])
    private var pageWork: Job? = null
    private var detailWork: Job? = null
    private var pager: CursorPager<MediaItemSummary>? = null

    fun connect(
        serverInput: String,
        allowHttp: Boolean,
        code: String = "",
        restore: Boolean = false,
    ) {
        val token = ++generation
        val previous = connectionJob
        previous?.cancel()
        connectionJob =
            scope.launch {
                previous?.join()
                release()
                mutable.value = TvUi(connecting = true, server = serverInput, identity = token)
                var candidate: TvSession? = null
                try {
                    val server = normalizeTvServer(serverInput, allowHttp)
                    val connected = sessionFactory(server)
                    candidate = connected
                    val result =
                        if (restore) {
                            if (connected.restore()) {
                                connected.auth.validateReadOnlyDevice()
                            } else {
                                SdkResult.failure(MediaFlowError.Unexpected("请在Web生成配对码后连接"))
                            }
                        } else {
                            require(code.isNotBlank()) { "请输入Web生成的一次性配对码" }
                            connected.auth.enrollReadOnlyWithPairingCode(code.trim(), "MediaFlow Android TV")
                        }
                    if (generation != token) return@launch
                    when (result) {
                        is SdkResult.Failure -> mutable.value = TvUi(server = server, identity = token, message = describe(result.error))
                        is SdkResult.Success -> {
                            session = connected
                            sessionWork = SupervisorJob(scope.coroutineContext[Job])
                            mutable.value =
                                TvUi(
                                    connected = true,
                                    server = server,
                                    identity = token,
                                    deviceName = connected.credentials.current()?.displayName,
                                )
                            rememberServer(server)
                            replacePager(MediaFilter())
                            val current = connected
                            CoroutineScope(scope.coroutineContext + sessionWork).launch {
                                when (val libraries = current.media.libraries()) {
                                    is SdkResult.Success ->
                                        if (session === current &&
                                            generation == token
                                        ) {
                                            update { it.copy(libraries = libraries.value) }
                                        }
                                    is SdkResult.Failure -> {
                                        if (session === current && generation == token) update { it.copy(message = "媒体库选项暂不可用，请刷新重试") }
                                        authFailure(libraries.error, current, token)
                                    }
                                }
                            }
                        }
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    if (generation ==
                        token
                    ) {
                        mutable.value =
                            TvUi(
                                server = serverInput,
                                identity = token,
                                message = if (failure is IllegalArgumentException) failure.message else "连接未完成，请检查服务器地址或系统凭据存储",
                            )
                    }
                } finally {
                    if (session !== candidate) candidate?.close()
                }
            }
    }

    fun filter(value: MediaFilter) {
        if (value == state.value.filter || session == null) return
        ++detailGeneration
        detailWork?.cancel()
        update {
            it.copy(
                filter = value,
                wall = emptyList(),
                page = PageState.Loading(false),
                hasMore = false,
                detailOpen = false,
                detail = null,
                detailMediaId = null,
                detailPath = TvDetailPath(),
                playbackUrl = null,
            )
        }
        CoroutineScope(scope.coroutineContext + sessionWork).launch { replacePager(value) }
    }

    private suspend fun replacePager(filter: MediaFilter) {
        val current = session ?: return
        val token = generation
        val pageToken = ++pageGeneration
        pageWork?.cancelAndJoin()
        if (session !== current || generation != token || pageToken != pageGeneration) return
        val work = SupervisorJob(sessionWork)
        pageWork = work
        val pages = current.media.mediaWall(filter)
        pager = pages
        update {
            it.copy(
                filter = filter,
                wall = emptyList(),
                page = PageState.Loading(false),
                hasMore = false,
                detail = null,
                detailMediaId = null,
                detailPath = TvDetailPath(),
                detailOpen = false,
                playbackUrl = null,
            )
        }
        val observing = CoroutineScope(scope.coroutineContext + work)
        observing.launch {
            pages.items.collect { items ->
                if (session === current && generation == token && pageToken == pageGeneration) update { it.copy(wall = items) }
            }
        }
        observing.launch {
            pages.state.collect { page ->
                if (session === current && generation == token && pageToken == pageGeneration) {
                    update { it.copy(page = page, hasMore = pages.hasMore()) }
                    (page as? PageState.Error)?.let { authFailure(it.error, current, token) }
                }
            }
        }
        loadMore(next = false)
    }

    fun loadMore(next: Boolean = true) {
        val current = session ?: return
        val pages = pager ?: return
        val token = generation
        val pageToken = pageGeneration
        val work = pageWork ?: return
        CoroutineScope(scope.coroutineContext + work).launch {
            if (next) pages.loadNext() else pages.reload()
            if (session === current && generation == token && pageToken == pageGeneration) update { it.copy(hasMore = pages.hasMore()) }
        }
    }

    fun refresh() {
        update { it.copy(artworkEpoch = it.artworkEpoch + 1) }
        loadMore(next = false)
        val current = session
        val token = generation
        if (current != null) {
            CoroutineScope(scope.coroutineContext + sessionWork).launch {
                when (val response = current.media.libraries()) {
                    is SdkResult.Success -> if (session === current && token == generation) update { it.copy(libraries = response.value) }
                    is SdkResult.Failure -> {
                        if (session === current && token == generation) update { it.copy(message = "媒体库选项暂不可用，请刷新重试") }
                        authFailure(response.error, current, token)
                    }
                }
            }
        }
        if (state.value.detailOpen) state.value.detailMediaId?.let(::detail)
    }

    fun detail(id: String) {
        val current = session ?: return
        val refreshing = state.value.detailMediaId == id
        val token = generation
        val selected = ++detailGeneration
        detailWork?.cancel()
        update {
            it.copy(
                detailOpen = true,
                detailMediaId = id,
                detailPath = if (refreshing) it.detailPath else TvDetailPath(),
                detail = if (refreshing) it.detail else null,
                detailLoading = true,
                playbackUrl = if (refreshing) it.playbackUrl else null,
                message = null,
            )
        }
        detailWork =
            CoroutineScope(scope.coroutineContext + sessionWork).launch {
                when (val response = current.media.detail(id)) {
                    is SdkResult.Success ->
                        if (session === current && generation == token && detailGeneration == selected) {
                            update {
                                it.copy(
                                    detail = response.value,
                                    detailLoading = false,
                                    detailPath = reconcileDetailPath(response.value, it.detailPath),
                                )
                            }
                            when (val playback = current.playback.externalPlaybackUrl()) {
                                is SdkResult.Success ->
                                    if (session === current &&
                                        generation == token &&
                                        detailGeneration == selected
                                    ) {
                                        update { it.copy(playbackUrl = playback.value) }
                                    }
                                is SdkResult.Failure -> authFailure(playback.error, current, token)
                            }
                        }
                    is SdkResult.Failure ->
                        if (session === current && generation == token && detailGeneration == selected) {
                            update { it.copy(detailLoading = false, message = describe(response.error)) }
                            authFailure(response.error, current, token)
                        }
                }
            }
    }

    fun back() {
        ++detailGeneration
        detailWork?.cancel()
        update {
            it.copy(
                detailOpen = false,
                detail = null,
                detailMediaId = null,
                detailPath = TvDetailPath(),
                detailLoading = false,
                playbackUrl = null,
            )
        }
    }

    fun selectDetailNode(id: String): Boolean {
        val ui = state.value
        val detail = ui.detail ?: return false
        if (!ui.connected || !ui.detailOpen || ui.detailLoading) return false
        val node = tvDetailNodes(detail, ui.detailPath).firstOrNull { it.id == id } ?: return false
        val path =
            if (node.type ==
                mediaflow.api.models.MediaChild.Type.season
            ) {
                TvDetailPath(seasonId = id)
            } else {
                ui.detailPath.copy(childId = id)
            }
        update { it.copy(detailPath = path) }
        return true
    }

    /** 物理Back和详情返回逐级后退；全局导航继续使用back直接回影视墙。 */
    fun backInDetail() {
        val path = state.value.detailPath
        when {
            path.childId != null -> update { it.copy(detailPath = path.copy(childId = null)) }
            path.seasonId != null -> update { it.copy(detailPath = TvDetailPath()) }
            else -> back()
        }
    }

    suspend fun artwork(id: String): SdkResult<Artwork> {
        val current = session ?: return SdkResult.failure(MediaFlowError.Unexpected("会话已关闭"))
        val token = generation
        val result = current.artwork.poster(id)
        if (result is SdkResult.Failure) authFailure(result.error, current, token)
        if (session !== current || token != generation) throw CancellationException("identity changed")
        return result
    }

    fun logout(expired: Boolean = false) {
        val token = ++generation
        // 先关闭冷启动恢复入口，避免进程关闭或安全存储失败后重新显示旧身份。
        rememberServer("")
        val previous = connectionJob
        previous?.cancel()
        connectionJob =
            scope.launch {
                previous?.join()
                val current = session
                mutable.value = TvUi(server = state.value.server, identity = token, message = if (expired) "设备已失效，请重新配对" else null)
                try {
                    if (expired) {
                        current?.auth?.discardLocalCredentials()
                    } else if (current?.auth?.logout() is SdkResult.Failure) {
                        update { it.copy(message = "已退出本机；服务端吊销未确认，请在Web检查设备") }
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    update { it.copy(message = "已关闭本机会话；凭据清理未确认，请在Web吊销设备") }
                } finally {
                    withContext(NonCancellable) { release() }
                }
            }
    }

    /** 确认层只能退出其发起时的会话；重复点击和旧窗口回调不得借用新身份。 */
    fun confirmLogout(identity: Long): Boolean {
        if (!state.value.connected || generation != identity || state.value.identity != identity) return false
        logout()
        return true
    }

    fun setTheme(value: TvTheme) {
        if (value == mutableTheme.value) return
        try {
            rememberTheme(value)
            mutableTheme.value = value
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            message("主题设置未能保存，请重试")
        }
    }

    fun message(value: String?) = update { it.copy(message = value) }

    private fun authFailure(
        error: MediaFlowError,
        current: TvSession,
        token: Long,
    ) {
        if (session === current && generation == token && error.isCredentialExpired()) logout(expired = true)
    }

    private suspend fun release() {
        ++pageGeneration
        ++detailGeneration
        sessionWork.cancelAndJoin()
        session?.close()
        session = null
        pager = null
        pageWork = null
        detailWork = null
    }

    private fun update(change: (TvUi) -> TvUi) {
        mutable.value = change(mutable.value)
    }

    override fun close() {
        ++generation
        connectionJob?.cancel()
        sessionWork.cancel()
        session?.close()
        session = null
        pager = null
        mutable.value = TvUi(server = state.value.server, identity = generation)
    }
}

/** 只接受HTTP(S) origin；HTTP需要用户显式选择可信局域网模式。 */
internal fun normalizeTvServer(
    input: String,
    allowHttp: Boolean,
): String {
    val uri = URI(input.trim())
    val scheme = uri.scheme?.lowercase()
    require(scheme in setOf("https", "http") && uri.host != null) { "请输入有效的HTTP/HTTPS服务器地址" }
    require(scheme != "http" || allowHttp) { "HTTP仅用于可信局域网，请先明确允许" }
    require(uri.userInfo == null && uri.rawQuery == null && uri.rawFragment == null && uri.path.orEmpty() in setOf("", "/")) {
        "服务器地址不能带账号、路径、查询或片段"
    }
    val port = uri.port.takeUnless { it == -1 || (scheme == "https" && it == 443) || (scheme == "http" && it == 80) } ?: -1
    require(port == -1 || port in 1..65535) { "端口无效" }
    return URI(scheme, null, uri.host.lowercase(), port, null, null, null).toASCIIString()
}

private fun describe(error: MediaFlowError): String =
    when (error) {
        is MediaFlowError.Http ->
            when (error.status) {
                401 -> "设备已失效，请重新配对"
                403 -> "当前设备没有媒体只读权限，请重新配对"
                404 -> "内容已不存在，请返回影视墙"
                else -> "服务器暂时无法完成请求，请重试"
            }
        else -> if (error.isOffline()) "连接中断，已保留当前内容" else "请求未完成，请检查连接后重试"
    }

/** 媒体移除后恢复最近存活位置，空集交给宿主的清筛选或重试入口。 */
internal fun restoredMediaId(
    ids: List<String>,
    previous: String?,
    index: Int,
): String? = previous?.takeIf { it in ids } ?: ids.getOrNull(index.coerceIn(0, (ids.size - 1).coerceAtLeast(0)))
