package mediaflow.client.core.cache

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import kotlinx.datetime.Instant
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.client.cache.MediaFlowCache
import mediaflow.client.core.net.MediaFlowApiFactory
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * 缓存语义（jvm 执行；iOS native 驱动路径由 C3 fixture server 链路覆盖）：
 * 媒体墙整体替换与回读、详情 JSON 快照、事件游标键值；Schema.version 钉住迁移基线。
 */
class MediaCacheTest {
    private fun newCache(): MediaCache {
        val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
        MediaFlowCache.Schema.create(driver)
        return MediaCache(driver, MediaFlowApiFactory.defaultJson())
    }

    private fun summary(
        id: String,
        title: String,
    ) = MediaItemSummary(
        id = id,
        type = MediaItemSummary.Type.movie,
        libraryId = "lib-1",
        title = title,
        year = 2021,
        localStatus = MediaItemSummary.LocalStatus.complete,
        artworkRef = null,
        updatedAt = Instant.parse("2026-07-17T00:00:00Z"),
    )

    @Test
    fun wallSnapshotIsReplacedWholesale() {
        val cache = newCache()
        cache.replaceWall(listOf(summary("a", "Dune"), summary("b", "Blade Runner")))
        assertEquals(listOf("Blade Runner", "Dune"), cache.cachedWall().map { it.title })
        cache.replaceWall(listOf(summary("c", "Contact")))
        assertEquals(listOf("Contact"), cache.cachedWall().map { it.title })
    }

    @Test
    fun wallRoundTripKeepsContractFields() {
        val cache = newCache()
        cache.replaceWall(listOf(summary("a", "Dune")))
        val item = cache.cachedWall().single()
        assertEquals("a", item.id)
        assertEquals("movie", item.type.toString())
        assertEquals("complete", item.localStatus.toString())
        assertEquals(2021L, item.year)
    }

    @Test
    fun detailSnapshotRoundTripsThroughJson() {
        val cache = newCache()
        val detail =
            MediaItemDetail(
                item = summary("a", "Dune"),
                metadata = emptyList(),
                versions = emptyList(),
                children = emptyList(),
                nfoStatus = MediaItemDetail.NfoStatus.notMinusRequested,
                relatedTaskIds = emptySet(),
            )
        cache.putDetail(detail)
        val restored = assertNotNull(cache.cachedDetail("a"))
        assertEquals("Dune", restored.item.title)
        assertNull(cache.cachedDetail("missing"))
    }

    @Test
    fun eventCursorPersistsAndDefaultsToZero() {
        val cache = newCache()
        assertEquals(0L, cache.eventCursor())
        cache.storeEventCursor(41)
        assertEquals(41L, cache.eventCursor())
        cache.storeEventCursor(42)
        assertEquals(42L, cache.eventCursor())
    }

    @Test
    fun schemaVersionIsPinnedAtBaseline() {
        // v1 基线：后续 schema 变更必须递增版本并补迁移脚本（迁移测试随版本引入）。
        assertEquals(1L, MediaFlowCache.Schema.version)
        assertTrue(MediaFlowCache.Schema.version > 0)
    }
}
