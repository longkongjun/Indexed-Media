package mediaflow.androidapp

import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.paging.PageState

/** 六屏路由（无导航框架依赖；返回栈由 AppRoot 持有）。 */
sealed interface Screen {
    data object Login : Screen

    data object Wall : Screen

    data object ReviewQueue : Screen

    data object Tasks : Screen

    data class Detail(
        val mediaItemId: String,
        val title: String,
    ) : Screen

    data class Confirm(
        val reviewCaseId: String,
    ) : Screen
}

/**
 * 页面状态 → 展示语义的纯映射（ui-contract 语义，UI 层零业务判断）。
 * 返回值：主体渲染模式 + stale 标记 + 可展示的错误文案。
 */
data class PageVisual(
    val mode: Mode,
    val stale: Boolean,
    val message: String?,
) {
    enum class Mode { Loading, Content, Empty, Error, Offline }

    companion object {
        fun from(state: PageState): PageVisual =
            when (state) {
                is PageState.Loading -> PageVisual(Mode.Loading, state.stale, null)
                PageState.Content -> PageVisual(Mode.Content, stale = false, null)
                PageState.Empty -> PageVisual(Mode.Empty, stale = false, "暂无内容")
                is PageState.Error -> PageVisual(Mode.Error, state.stale, readableMessage(state.error))
                is PageState.Offline -> PageVisual(Mode.Offline, state.stale, "离线：无法连接服务器")
            }

        fun readableMessage(error: MediaFlowError): String =
            when (error) {
                is MediaFlowError.Http ->
                    when (error.status) {
                        401 -> "登录已过期，请重新登录"
                        403 -> "没有执行该操作的权限"
                        404 -> "内容不存在或已移除"
                        409 -> "内容已被他人更新，请刷新后重试"
                        429 -> "操作过于频繁，请稍后重试"
                        else -> error.message.ifBlank { "服务错误（${error.status}）" }
                    }
                is MediaFlowError.Network -> "网络异常，请检查连接"
                is MediaFlowError.Serialization -> "数据格式异常"
                is MediaFlowError.Unexpected -> "发生未预期错误"
            }
    }
}
