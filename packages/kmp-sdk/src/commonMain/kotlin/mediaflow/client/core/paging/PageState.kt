package mediaflow.client.core.paging

import mediaflow.client.core.error.MediaFlowError

/**
 * 页面状态语义（对齐 ui-contract / Web `TaskCenterState` 模型）。
 *
 * kind ∈ {Loading, Content, Empty, Error, Offline}；`stale=true` 表示已有内容
 * 但正在刷新或刷新失败（UI 保留旧内容并提示）。Offline 是网络连接类错误的
 * 专属状态（[MediaFlowError.isOffline]）。
 */
sealed interface PageState {
    /** 加载中；[stale]=true 表示旧内容仍在屏。 */
    data class Loading(
        val stale: Boolean,
    ) : PageState

    /** 有内容（内容本体由列表容器持有，状态只表达语义）。 */
    data object Content : PageState

    /** 加载完成且无内容。 */
    data object Empty : PageState

    /** 错误（非离线类）；[error] 供 UI 展示可读信息与重试入口。 */
    data class Error(
        val error: MediaFlowError,
        val stale: Boolean,
    ) : PageState

    /** 离线（连接建立/DNS 失败）；重试由网络恢复或用户触发。 */
    data class Offline(
        val stale: Boolean,
    ) : PageState

    companion object {
        /** 从错误推导页面状态（stale 由调用方传入）。 */
        fun fromError(
            error: MediaFlowError,
            stale: Boolean,
        ): PageState = if (error.isOffline()) Offline(stale) else Error(error, stale)
    }
}
