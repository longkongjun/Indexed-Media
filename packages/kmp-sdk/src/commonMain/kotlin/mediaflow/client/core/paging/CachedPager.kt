package mediaflow.client.core.paging

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * 缓存合成分页器：把 [CursorPager] 的权威结果与本地读缓存按一致语义合成。
 *
 * - **首屏快照**：构造时同步读缓存作为初始条目，状态为 `Loading(stale = 有缓存)`；
 * - **权威门控**：只有分页器本代次拥有状态（[reload]/[loadNext] 返回 true）且结果为
 *   `Content`/`Empty` 时才替换条目并整体写缓存——权威空页同样写入，从而清除旧缓存；
 *   写缓存失败直接向调用者抛出；
 * - **非权威不改条目**：`Error`/`Offline` 保留当前条目，`stale` 按"屏上是否有条目"重算；
 *   分页器自身以空列表为初值的 `items` 流不会泄漏到本类；
 * - **取消/晚到**：被取消的调用抛出取消且不写入；被更新代次取代的调用返回 false 且不写入。
 *
 * 依赖 [CursorPager] 的不变量：`Content`/`Empty` 只在当前代次成功路径于写入 `items` 之后
 * 设置，失败与取消均不发布这两个状态。
 */
class CachedPager<T>(
    private val pager: CursorPager<T>,
    private val readCache: () -> List<T>,
    private val writeCache: (List<T>) -> Unit,
) {
    private val mutableItems = MutableStateFlow(readCache())
    private val mutableState = MutableStateFlow<PageState>(PageState.Loading(stale = mutableItems.value.isNotEmpty()))

    /** 当前可展示条目：缓存快照或最近一次权威结果。 */
    val items: StateFlow<List<T>> = mutableItems.asStateFlow()

    /** 页面状态；`stale=true` 表示屏上仍有条目。 */
    val state: StateFlow<PageState> = mutableState.asStateFlow()

    fun hasMore(): Boolean = pager.hasMore()

    /** 重新加载第一页；只有本次调用拥有状态时才应用结果并写缓存。 */
    suspend fun reload(): Boolean {
        mutableState.value = PageState.Loading(stale = mutableItems.value.isNotEmpty())
        val owned = pager.reload()
        if (owned) applyPublished()
        return owned
    }

    /** 追加下一页；无更多页直接返回 false 且不改状态。 */
    suspend fun loadNext(): Boolean {
        if (!pager.hasMore()) return false
        mutableState.value = PageState.Loading(stale = mutableItems.value.isNotEmpty())
        val owned = pager.loadNext()
        if (owned) applyPublished()
        return owned
    }

    private fun applyPublished() {
        val onScreen = mutableItems.value.isNotEmpty()
        when (val published = pager.state.value) {
            PageState.Content, PageState.Empty -> {
                val authoritative = pager.items.value
                mutableItems.value = authoritative
                writeCache(authoritative)
                mutableState.value = published
            }
            is PageState.Error -> mutableState.value = published.copy(stale = onScreen)
            is PageState.Offline -> mutableState.value = published.copy(stale = onScreen)
            // 本代次拥有状态时分页器不会停在 Loading；此分支只为 when 穷尽，等值重写无副作用。
            is PageState.Loading -> mutableState.value = published.copy(stale = onScreen)
        }
    }
}
