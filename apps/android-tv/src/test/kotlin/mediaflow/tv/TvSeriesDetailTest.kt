package mediaflow.tv

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.MockEngineConfig
import io.ktor.client.engine.mock.respond
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.client.plugins.defaultRequest
import io.ktor.http.HttpStatusCode
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import mediaflow.api.models.MediaChild
import mediaflow.api.models.MediaFileAsset
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.MediaVersion
import mediaflow.client.core.auth.InMemoryDeviceAuthStore
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Instant

/** 受控HTTP使用真实SDK；子节点只是当前根详情的只读投影，不构造子ID请求。 */
@OptIn(ExperimentalCoroutinesApi::class)
class TvSeriesDetailTest {
    @Test
    fun refreshingAnExistingDetailKeepsItsProjectionUntilTheReplyArrives() =
        runTest {
            val gate = CompletableDeferred<Unit>()
            var reads = 0
            val original = detail(ROOT, MediaItemSummary.Type.series, fixtureChildren())
            val fixture =
                Fixture(this) { id ->
                    assertEquals(ROOT, id)
                    if (++reads > 1) gate.await()
                    original
                }
            try {
                fixture.connect()
                fixture.model.detail(ROOT)
                advanceUntilIdle()
                assertNotNull(fixture.model.state.value.detail)
                fixture.model.refresh()
                runCurrent()
                assertTrue(fixture.model.state.value.detailLoading)
                assertNotNull(fixture.model.state.value.detail, "refresh must retain the current projection and its navigable context")
                gate.complete(Unit)
                advanceUntilIdle()
                assertEquals(original, fixture.model.state.value.detail)
            } finally {
                gate.complete(Unit)
                fixture.close()
            }
        }

    @Test
    fun hierarchySelectionSortsSparseSeasonsAndNeverFetchesAChild() =
        runTest {
            val original = detail(ROOT, MediaItemSummary.Type.series, fixtureChildren())
            val fixture = Fixture(this) { original }
            try {
                fixture.connect()
                fixture.model.detail(ROOT)
                advanceUntilIdle()
                assertEquals(listOf(SEASON2, SEASON7), tvDetailNodes(original, TvDetailPath()).map { it.id })
                val tied =
                    original.copy(
                        children =
                            original.children + MediaChild(uuid(206), null, MediaChild.Type.season, "Tied season", 2, emptyList()),
                    )
                assertEquals(listOf(SEASON2, uuid(206), SEASON7), tvDetailNodes(tied, TvDetailPath()).map { it.id })
                assertTrue(fixture.model.selectDetailNode(SEASON2))
                assertEquals(TvDetailPath(seasonId = SEASON2), fixture.model.state.value.detailPath)
                assertEquals(listOf(EPISODE2, EPISODE10), tvDetailNodes(original, fixture.model.state.value.detailPath).map { it.id })
                assertFalse(fixture.model.selectDetailNode(EPISODE7))
                assertFalse(fixture.model.selectDetailNode("missing"))
                assertTrue(fixture.model.selectDetailNode(EPISODE10))
                assertEquals(versions(10), tvDetailVersions(original, fixture.model.state.value.detailPath))
                assertTrue(tvDetailNodes(original, fixture.model.state.value.detailPath).isEmpty())
                fixture.model.backInDetail()
                assertEquals(TvDetailPath(seasonId = SEASON2), fixture.model.state.value.detailPath)
                fixture.model.backInDetail()
                assertEquals(TvDetailPath(), fixture.model.state.value.detailPath)
                fixture.model.backInDetail()
                assertFalse(fixture.model.state.value.detailOpen)
                assertEquals(listOf(ROOT), fixture.mediaRequests)
            } finally {
                fixture.close()
            }
        }

    @Test
    fun refreshReconcilesTheCurrentNavigationAfterBackAndRemoval() =
        runTest {
            val gate = CompletableDeferred<Unit>()
            var reads = 0
            var children = fixtureChildren()
            val fixture =
                Fixture(this) {
                    if (++reads == 2) gate.await()
                    detail(ROOT, MediaItemSummary.Type.series, children)
                }
            try {
                fixture.connect()
                fixture.model.detail(ROOT)
                advanceUntilIdle()
                fixture.model.selectDetailNode(SEASON2)
                fixture.model.selectDetailNode(EPISODE2)
                fixture.model.refresh()
                runCurrent()
                assertEquals(TvDetailPath(SEASON2, EPISODE2), fixture.model.state.value.detailPath)
                assertNotNull(fixture.model.state.value.detail)
                fixture.model.backInDetail()
                gate.complete(Unit)
                advanceUntilIdle()
                assertEquals(
                    TvDetailPath(seasonId = SEASON2),
                    fixture.model.state.value.detailPath,
                    "a pending reply cannot undo the user's back navigation",
                )
                fixture.model.selectDetailNode(EPISODE2)
                children = children.filterNot { it.id == EPISODE2 }
                fixture.model.refresh()
                advanceUntilIdle()
                assertEquals(TvDetailPath(seasonId = SEASON2), fixture.model.state.value.detailPath)
                children = children.filterNot { it.id == SEASON2 || it.parentId == SEASON2 }
                fixture.model.refresh()
                advanceUntilIdle()
                assertEquals(TvDetailPath(), fixture.model.state.value.detailPath)
                assertFalse(fixture.model.selectDetailNode(EPISODE2))
                assertEquals(listOf(ROOT, ROOT, ROOT, ROOT), fixture.mediaRequests)
            } finally {
                gate.complete(Unit)
                fixture.close()
            }
        }

    @Test
    fun failedRefreshKeepsReadableSelectionAndRetriesTheSameRoot() =
        runTest {
            var fail = false
            val original = detail(ROOT, MediaItemSummary.Type.series, fixtureChildren())
            val fixture = Fixture(this) { if (fail) throw java.io.IOException("controlled offline read") else original }
            try {
                fixture.connect()
                fixture.model.detail(ROOT)
                advanceUntilIdle()
                fixture.model.selectDetailNode(SEASON2)
                fixture.model.selectDetailNode(EPISODE2)
                fail = true
                fixture.model.refresh()
                advanceUntilIdle()
                assertFalse(fixture.model.state.value.detailLoading)
                assertEquals(original, fixture.model.state.value.detail)
                assertEquals(TvDetailPath(SEASON2, EPISODE2), fixture.model.state.value.detailPath)
                assertNotNull(fixture.model.state.value.message)
                fail = false
                fixture.model.refresh()
                advanceUntilIdle()
                assertEquals(TvDetailPath(SEASON2, EPISODE2), fixture.model.state.value.detailPath)
                assertEquals(listOf(ROOT, ROOT, ROOT), fixture.mediaRequests)
                fixture.model.back()
                assertFalse(fixture.model.state.value.detailOpen, "global wall navigation must close the entire nested detail")
                assertEquals(TvDetailPath(), fixture.model.state.value.detailPath)
            } finally {
                fixture.close()
            }
        }

    @Test
    fun newMediaAndSessionClearSelectionAndRejectLateRootReplies() =
        runTest {
            val gate = CompletableDeferred<Unit>()
            var reads = 0
            var lateProduced = false
            val fixture =
                Fixture(this) { id ->
                    if (id == ROOT) {
                        if (++reads == 2) {
                            withContext(NonCancellable) {
                                gate.await()
                                lateProduced = true
                            }
                        }
                        detail(ROOT, MediaItemSummary.Type.series, fixtureChildren())
                    } else {
                        detail(SECOND, MediaItemSummary.Type.movie, emptyList(), versions(99))
                    }
                }
            try {
                fixture.connect()
                fixture.model.detail(ROOT)
                advanceUntilIdle()
                fixture.model.selectDetailNode(SEASON2)
                fixture.model.selectDetailNode(EPISODE2)
                fixture.model.refresh()
                runCurrent()
                fixture.model.detail(SECOND)
                assertEquals(TvDetailPath(), fixture.model.state.value.detailPath)
                advanceUntilIdle()
                assertEquals(
                    SECOND,
                    fixture.model.state.value.detail
                        ?.item
                        ?.id,
                )
                gate.complete(Unit)
                advanceUntilIdle()
                assertTrue(lateProduced)
                assertEquals(
                    SECOND,
                    fixture.model.state.value.detail
                        ?.item
                        ?.id,
                )
                fixture.model.detail(ROOT)
                advanceUntilIdle()
                fixture.model.selectDetailNode(SEASON7)
                fixture.model.selectDetailNode(EPISODE7)
                val oldIdentity = fixture.model.state.value.identity
                fixture.connect("https://second.local")
                assertTrue(fixture.model.state.value.identity != oldIdentity)
                assertEquals(TvDetailPath(), fixture.model.state.value.detailPath)
                assertNull(fixture.model.state.value.detail)
                assertFalse(fixture.model.state.value.detailOpen)
                assertTrue(fixture.mediaRequests.all { it == ROOT || it == SECOND })
            } finally {
                gate.complete(Unit)
                fixture.close()
            }
        }

    @Test
    fun movieGenericAndSeasonVersionsStayWithTheirOwningNodes() =
        runTest {
            val movie = detail(SECOND, MediaItemSummary.Type.movie, emptyList(), versions(99))
            val generic =
                detail(
                    ROOT,
                    MediaItemSummary.Type.genericMinusVideo,
                    listOf(
                        MediaChild(MEMBER, null, MediaChild.Type.genericMinusVideoMinusItem, "Extra", 0, versions(40)),
                    ),
                )
            val fixture = Fixture(this) { if (it == SECOND) movie else generic }
            try {
                fixture.connect()
                fixture.model.detail(SECOND)
                advanceUntilIdle()
                assertEquals(versions(99), tvDetailVersions(movie, TvDetailPath()))
                assertTrue(tvDetailNodes(movie, TvDetailPath()).isEmpty())
                fixture.model.detail(ROOT)
                advanceUntilIdle()
                assertTrue(fixture.model.selectDetailNode(MEMBER))
                assertEquals(TvDetailPath(childId = MEMBER), fixture.model.state.value.detailPath)
                assertEquals(versions(40), tvDetailVersions(generic, fixture.model.state.value.detailPath))
                fixture.model.backInDetail()
                assertTrue(fixture.model.state.value.detailOpen)
                assertEquals(TvDetailPath(), fixture.model.state.value.detailPath)
                val series = detail(ROOT, MediaItemSummary.Type.series, fixtureChildren())
                assertEquals(versions(70, 7), tvDetailVersions(series, TvDetailPath(seasonId = SEASON7)))
                assertTrue(
                    tvDetailVersions(series, TvDetailPath(SEASON2, EPISODE7)).isEmpty(),
                    "a sibling season's versions must not be borrowed",
                )
                val sharedFile = versions(2).first().files.single()
                val multiEpisodeFile =
                    series.copy(
                        children =
                            series.children.map { node ->
                                if (node.type == MediaChild.Type.episode) {
                                    node.copy(
                                        versions =
                                            node.versions.mapIndexed { index, version ->
                                                if (index == 0) version.copy(files = listOf(sharedFile)) else version
                                            },
                                    )
                                } else {
                                    node
                                }
                            },
                    )
                assertEquals(
                    sharedFile.id,
                    tvDetailVersions(multiEpisodeFile, TvDetailPath(SEASON2, EPISODE2))
                        .first()
                        .files
                        .single()
                        .id,
                )
                assertEquals(
                    sharedFile.id,
                    tvDetailVersions(multiEpisodeFile, TvDetailPath(SEASON2, EPISODE10))
                        .first()
                        .files
                        .single()
                        .id,
                )
                assertEquals(listOf(SECOND, ROOT), fixture.mediaRequests)
            } finally {
                fixture.close()
            }
        }

    private class Fixture(
        private val scope: TestScope,
        private val read: suspend (String) -> MediaItemDetail,
    ) : AutoCloseable {
        private val owner = CoroutineScope(scope.coroutineContext + SupervisorJob(scope.coroutineContext[Job]))
        val mediaRequests = mutableListOf<String>()
        val model =
            TvModel(owner, { server ->
                val engine =
                    MockEngine(
                        MockEngineConfig().apply {
                            dispatcher = StandardTestDispatcher(scope.testScheduler)
                            addHandler { request ->
                                val path = request.url.encodedPath
                                val (body, status) =
                                    when {
                                        path.endsWith("/enrollment") -> enrollment to HttpStatusCode.Created
                                        path.startsWith("/api/v1/media-items/") -> {
                                            val id = path.substringAfterLast('/')
                                            mediaRequests.add(id)
                                            json.encodeToString(MediaItemDetail.serializer(), read(id)) to HttpStatusCode.OK
                                        }
                                        path == "/api/v1/integrations/jellyfin" -> "{}" to HttpStatusCode.NotFound
                                        else -> "{\"items\":[]}" to HttpStatusCode.OK
                                    }
                                respond(body, status, headersOf("Content-Type", "application/json"))
                            }
                        },
                    )
                val client =
                    HttpClient(engine) {
                        defaultRequest { url(server) }
                        install(ContentNegotiation) { json(TvSeriesDetailTest.json) }
                    }
                TvSession(server, InMemoryDeviceAuthStore(), SessionAuthCache(), client, json)
            })

        suspend fun connect(server: String = "https://nas.local") {
            model.connect(server, false, "fixture-code")
            scope.advanceUntilIdle()
            assertTrue(model.state.value.connected)
        }

        override fun close() {
            model.close()
            owner.cancel()
        }
    }

    private companion object {
        val json = MediaFlowApiFactory.defaultJson()
        const val ROOT = "019f0000-0000-7000-8000-000000000100"
        const val SECOND = "019f0000-0000-7000-8000-000000000101"
        const val SEASON2 = "019f0000-0000-7000-8000-000000000202"
        const val SEASON7 = "019f0000-0000-7000-8000-000000000207"
        const val EPISODE2 = "019f0000-0000-7000-8000-000000000302"
        const val EPISODE10 = "019f0000-0000-7000-8000-000000000310"
        const val EPISODE7 = "019f0000-0000-7000-8000-000000000307"
        const val MEMBER = "019f0000-0000-7000-8000-000000000400"
        const val DEVICE = "019f0000-0000-7000-8000-000000000051"
        val enrollment =
            """
            {"token":"series-fixture-only","device":{
              "id":"$DEVICE","display_name":"TV","created_at":"2026-10-28T00:00:00Z",
              "expires_at":"2027-10-28T00:00:00Z","access_mode":"media-read-only"
            }}
            """.trimIndent()

        fun versions(
            seed: Int = 1,
            season: Int = 2,
        ) = listOf(
            MediaVersion(
                uuid(5000 + seed * 10 + 1),
                "1080p",
                listOf(
                    MediaFileAsset(
                        uuid(6000 + seed * 10 + 1),
                        "incoming/episode-$seed.mkv",
                        "Series/Season $season/episode-$seed.mkv",
                        1_048_576,
                    ),
                ),
            ),
            MediaVersion(
                uuid(5000 + seed * 10 + 2),
                "4K",
                listOf(
                    MediaFileAsset(
                        uuid(6000 + seed * 10 + 2),
                        "incoming/episode-$seed-4k.mkv",
                        "Series/Season $season/episode-$seed-4k.mkv",
                        4_194_304,
                    ),
                ),
            ),
        )

        fun uuid(number: Int) = "019f0000-0000-7000-8000-${number.toString().padStart(12, '0')}"

        fun fixtureChildren() =
            listOf(
                MediaChild(EPISODE10, SEASON2, MediaChild.Type.episode, "Episode 10", 10, versions(10)),
                MediaChild(SEASON7, null, MediaChild.Type.season, "Season 7", 7, versions(70, 7)),
                MediaChild(EPISODE2, SEASON2, MediaChild.Type.episode, "Episode 2", 2, versions(2)),
                MediaChild(SEASON2, null, MediaChild.Type.season, "Season 2", 2, emptyList()),
                MediaChild(EPISODE7, SEASON7, MediaChild.Type.episode, "Episode 7", 1, versions(7, 7)),
            )

        fun detail(
            id: String,
            type: MediaItemSummary.Type,
            children: List<MediaChild>,
            rootVersions: List<MediaVersion> = emptyList(),
        ) = MediaItemDetail(
            MediaItemSummary(
                id,
                type,
                DEVICE,
                "Series fixture",
                2026,
                MediaItemSummary.LocalStatus.complete,
                null,
                Instant.parse("2026-10-28T00:00:00Z"),
            ),
            emptyList(),
            rootVersions,
            children,
            MediaItemDetail.NfoStatus.complete,
            emptySet(),
        )
    }
}
