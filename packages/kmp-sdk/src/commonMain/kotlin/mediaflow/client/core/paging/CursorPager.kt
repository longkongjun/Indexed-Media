package mediaflow.client.core.paging

import kotlinx.coroutines.flow.MutableStateFlow

/** 一页数据：条目 + 下一页游标（null = 到达末页，契约 next_cursor 语义）。 */
data class PageResult<T>(
    val items: List<T>,
    val nextCursor: String?,
)

/**
 * 有界游标分页状态机：对齐 Web 端 useMediaItems 的语义——
 *
 * - **epoch 乱序守卫**：每次 [reload]/[loadNext] 递增代次；旧代次的响应（成功或
 *   失败）一律丢弃，避免旧数据覆盖新上下文（并发触发时只有最新请求可写状态）；
 * - **加载更多**：[loadNext] 追加下一页；无更多页返回 false；
 * - **状态语义**：[state] 遵循 [PageState]（Loading/Content/Empty/Error/Offline
 *   + stale），翻页失败保留已有内容并标记 stale。
 */
class CursorPager<T>(
    private val fetch: suspend (cursor: String?, limit: Int) -> PageResult<T>,
    private val limit: Int = DEFAULT_LIMIT,
) {
    /** 当前页面状态（不含条目本体）。 */
    val state: MutableStateFlow<PageState> = MutableStateFlow(PageState.Loading(stale = false))

    /** 已加载条目（按服务端顺序）；翻页追加，reload 整体替换。 */
    val items: MutableStateFlow<List<T>> = MutableStateFlow(emptyList())

    private var nextCursor: String? = null
    private var epoch: Long = 0

    /** 还有下一页可加载。 */
    fun hasMore(): Boolean = nextCursor != null

    /**
     * 重新加载第一页（筛选上下文由 fetch 闭包持有）；旧代次在途响应作废。
     * 返回 true 表示本次调用最终写入了状态（即调用时是最新代次）。
     */
    suspend fun reload(): Boolean {
        val myEpoch = ++epoch
        val stale = items.value.isNotEmpty()
        state.value = PageState.Loading(stale)
        return execute(myEpoch, cursor = null, append = false, stale)
    }

    /**
     * 追加加载下一页；无更多页返回 false，被并发的新一代次取代返回 false。
     * 翻页失败不回退已加载内容（stale 标记提示）。
     */
    suspend fun loadNext(): Boolean {
        val cursor = nextCursor ?: return false
        val myEpoch = ++epoch
        val stale = items.value.isNotEmpty()
        state.value = PageState.Loading(stale)
        return execute(myEpoch, cursor, append = true, stale)
    }

    /**
     * 执行一次取数并按代次判定写入权。
     *
     * Suppress 说明：fetch 的 catch(Exception) 是边界映射点（异常经 ErrorMapping
     * 归类为统一错误模型，不外抛）；多出口来自代次守卫的早退语义。
     */
    @Suppress("TooGenericExceptionCaught", "SwallowedException", "ReturnCount")
    private suspend fun execute(
        myEpoch: Long,
        cursor: String?,
        append: Boolean,
        stale: Boolean,
    ): Boolean {
        val result =
            try {
                fetch(cursor, limit)
            } catch (failure: Exception) {
                if (myEpoch != epoch) return false
                val error =
                    mediaflow.client.core.error.ErrorMapping
                        .fromThrowable(failure)
                state.value = PageState.fromError(error, stale)
                return true
            }
        if (myEpoch != epoch) return false
        nextCursor = result.nextCursor
        items.value =
            if (append) items.value + result.items else result.items
        state.value =
            if (items.value.isEmpty()) PageState.Empty else PageState.Content
        return true
    }

    companion object {
        /** 契约 limit：1..200，默认 50。 */
        const val DEFAULT_LIMIT = 50
        const val MAX_LIMIT = 200
    }
}
