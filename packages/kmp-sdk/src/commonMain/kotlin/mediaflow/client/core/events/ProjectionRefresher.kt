package mediaflow.client.core.events

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import mediaflow.client.core.events.TaskEventEnvelope.Companion.TYPE_STREAM_GAP

/**
 * SSE → 权威刷新桥（对齐 Web useProjectionEvents 的合并刷新语义）：
 * 事件按路由分组标记脏，去抖窗口（默认 300ms）内的多次变更合并为一次刷新；
 * `stream.gap` 将全部路由标记脏（缺口期间的任何投影都可能过期）。
 *
 * 刷新动作通常是对各分页器 `reload()` 的调用——分页器自身的 epoch 守卫保证
 * 在途刷新不被旧响应污染。
 */
class ProjectionRefresher(
    private val events: kotlinx.coroutines.flow.Flow<TaskEventEnvelope>,
    private val debounceMs: Long = DEFAULT_DEBOUNCE_MS,
) {
    /** 一条路由：关注的契约事件类型集合 + 对应的权威刷新动作。 */
    class Route(
        val id: String,
        val types: Set<String>,
        val refresh: suspend () -> Unit,
    )

    /** 启动桥接协程；取消返回的 Job 即停止。 */
    fun start(
        scope: CoroutineScope,
        vararg routes: Route,
    ): Job =
        scope.launch {
            val dirty = mutableSetOf<String>()
            var flushJob: Job? = null
            events.collect { event ->
                routes.forEach { route ->
                    if (event.type in route.types || event.type == TYPE_STREAM_GAP) dirty += route.id
                }
                if (dirty.isEmpty()) return@collect
                flushJob?.cancel()
                flushJob =
                    launch {
                        delay(debounceMs)
                        val pending = dirty.toList()
                        dirty.removeAll(pending.toSet())
                        pending.forEach { id ->
                            routes.firstOrNull { it.id == id }?.refresh()
                        }
                    }
            }
        }

    companion object {
        const val DEFAULT_DEBOUNCE_MS = 300L
    }
}
