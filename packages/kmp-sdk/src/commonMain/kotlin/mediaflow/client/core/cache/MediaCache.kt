package mediaflow.client.core.cache

import app.cash.sqldelight.db.SqlDriver
import kotlinx.serialization.json.Json
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.client.cache.MediaFlowCache
import mediaflow.client.core.net.MediaFlowApiFactory

/**
 * 本地读缓存门面：媒体墙（字段级行存）+ 详情（JSON 快照，schema 演进免迁移）+ 键值。
 *
 * 驱动由宿主构造（Android 需 Context、iOS 需沙盒路径——平台能力边界），SDK 只
 * 依赖 SQLDelight 生成的 [MediaFlowCache]。写入语义为整体替换（权威刷新后
 * `replaceWall`），缓存不作为事实源。
 */
class MediaCache(
    driver: SqlDriver,
    private val json: Json = MediaFlowApiFactory.defaultJson(),
) {
    private val database = MediaFlowCache(driver)

    /** 权威刷新后整体替换媒体墙快照。 */
    fun replaceWall(items: List<MediaItemSummary>) {
        database.transaction {
            database.mediaFlowCacheQueries.clearWall()
            val now = nowEpochMs()
            items.forEach { item ->
                database.mediaFlowCacheQueries.insertWallItem(
                    id = item.id,
                    type = item.type.toString(),
                    library_id = item.libraryId,
                    title = item.title,
                    year = item.year,
                    local_status = item.localStatus.toString(),
                    artwork_state = item.artworkRef?.state?.toString(),
                    updated_at = item.updatedAt.toString(),
                    cached_at = now,
                )
            }
        }
    }

    /** 离线读：按标题序返回缓存快照（可能为空或过期，由调用方展示 stale 语义）。 */
    fun cachedWall(): List<MediaItemSummary> =
        database.mediaFlowCacheQueries.selectWall().executeAsList().map { row ->
            MediaItemSummary(
                id = row.id,
                type =
                    mediaflow.api.models.MediaItemSummary.Type.entries
                        .firstOrNull { it.value == row.type }
                        ?: mediaflow.api.models.MediaItemSummary.Type.genericMinusVideo,
                libraryId = row.library_id,
                title = row.title,
                year = row.year,
                localStatus =
                    mediaflow.api.models.MediaItemSummary.LocalStatus.entries
                        .firstOrNull { it.value == row.local_status }
                        ?: mediaflow.api.models.MediaItemSummary.LocalStatus.partial,
                artworkRef = null,
                updatedAt = kotlinx.datetime.Instant.parse(row.updated_at),
            )
        }

    /** 详情 JSON 快照（含 children/versions/metadata 全量）。 */
    fun putDetail(detail: MediaItemDetail) {
        database.mediaFlowCacheQueries.putDetail(
            media_item_id = detail.item.id,
            payload = json.encodeToString(MediaItemDetail.serializer(), detail),
            cached_at = nowEpochMs(),
        )
    }

    fun cachedDetail(mediaItemId: String): MediaItemDetail? {
        val payload =
            database.mediaFlowCacheQueries.selectDetail(mediaItemId).executeAsOneOrNull() ?: return null
        return runCatching { json.decodeFromString(MediaItemDetail.serializer(), payload) }.getOrNull()
    }

    /** 事件游标持久化（SSE 冷启动续传）。 */
    fun storeEventCursor(cursor: Long) {
        database.mediaFlowCacheQueries.putValue(EVENT_CURSOR_KEY, cursor.toString())
    }

    fun eventCursor(): Long =
        database.mediaFlowCacheQueries
            .selectValue(EVENT_CURSOR_KEY)
            .executeAsOneOrNull()
            ?.toLongOrNull() ?: 0L

    fun close() {
        // SQLDelight 2.x 驱动关闭由宿主持有；数据库对象无需显式释放（驱动持有时）。
    }

    companion object {
        private const val EVENT_CURSOR_KEY = "sse.event-cursor"

        /** 单调时钟（缓存的 cached_at 只用于排障，不做展示语义）。 */
        internal fun nowEpochMs(): Long =
            kotlin.time.Clock.System
                .now()
                .toEpochMilliseconds()
    }
}
