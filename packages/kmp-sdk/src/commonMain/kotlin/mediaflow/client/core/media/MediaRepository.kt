package mediaflow.client.core.media

import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.MediaLibrary
import mediaflow.client.core.SdkResult
import mediaflow.client.core.apiCall
import mediaflow.client.core.errorOrNull
import mediaflow.client.core.getOrNull
import mediaflow.client.core.paging.CursorPager
import mediaflow.client.core.paging.MediaFlowErrorTunnel
import mediaflow.client.core.paging.PageResult

/** 媒体墙的服务端筛选上下文（契约 listMediaItems 查询参数的有界镜像）。 */
data class MediaFilter(
    val type: String? = null,
    val libraryId: String? = null,
    val localStatus: String? = null,
    val query: String? = null,
)

/** 正式 Catalog 读模型：媒体墙分页 + 详情。 */
class MediaRepository(
    private val api: DefaultApi,
    private val json: Json,
) {
    /** 安全媒体库筛选项；只含公开ID与名称，不返回服务端文件系统路径。 */
    suspend fun libraries(): SdkResult<List<MediaLibrary>> =
        when (val result = apiCall(json) { api.listMediaLibraries() }) {
            is SdkResult.Success -> SdkResult.success(result.value.items)
            is SdkResult.Failure -> result
        }

    /** 建立媒体墙分页器；筛选上下文经闭包固定，换筛选 = 新建分页器。 */
    fun mediaWall(
        filter: MediaFilter = MediaFilter(),
        limit: Int = CursorPager.DEFAULT_LIMIT,
    ): CursorPager<MediaItemSummary> =
        CursorPager(
            fetch = { cursor, pageLimit ->
                val result =
                    apiCall(json) {
                        api.listMediaItems(
                            cursor = cursor,
                            limit = pageLimit.toLong(),
                            type = filter.type?.let { DefaultApi.TypeListMediaItems.entries.firstOrNull { e -> e.value == it } },
                            libraryId = filter.libraryId,
                            localStatus =
                                filter.localStatus?.let {
                                    DefaultApi.LocalStatusListMediaItems.entries.firstOrNull { e ->
                                        e.value ==
                                            it
                                    }
                                },
                            q = filter.query,
                        )
                    }
                result.getOrNull()?.let { page -> PageResult(page.items, page.nextCursor) }
                    ?: throw MediaFlowErrorTunnel(result.errorOrNull!!)
            },
            limit = limit,
        )

    /** 单条详情（404 等语义经统一错误模型表达）。 */
    suspend fun detail(mediaItemId: String): SdkResult<MediaItemDetail> = apiCall(json) { api.getMediaItem(mediaItemId) }

    /** iOS 桥用的显式游标单页获取（分页状态推进留在 Swift 侧）。 */
    internal suspend fun apiPage(cursor: String?): mediaflow.api.infrastructure.HttpResponse<mediaflow.api.models.MediaItemPage> =
        api.listMediaItems(cursor = cursor)
}
