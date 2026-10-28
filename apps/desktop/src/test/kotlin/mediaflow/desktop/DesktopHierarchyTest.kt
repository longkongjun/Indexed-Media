package mediaflow.desktop

import mediaflow.api.models.MediaChild
import mediaflow.api.models.MediaFileAsset
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.MediaVersion
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.time.Instant

class DesktopHierarchyTest {
    @Test
    fun sparseSeasonsAndEpisodesStayWithTheirParents() {
        val detail = desktopSeriesFixture()
        assertEquals(listOf("s2", "s7"), desktopDetailNodes(detail, DesktopDetailPath()).map { it.id })
        assertEquals(listOf("e2", "e10"), desktopDetailNodes(detail, DesktopDetailPath("s2")).map { it.id })
        assertEquals(
            "episode.mkv",
            desktopDetailVersions(detail, DesktopDetailPath("s2", "e2"))
                .single()
                .files
                .single()
                .currentRelativePath,
        )
        assertTrue(desktopDetailVersions(detail, DesktopDetailPath("s7", "e2")).isEmpty())
    }

    @Test
    fun deletionFallsBackOnlyToAnExistingAncestor() {
        val detail = desktopSeriesFixture()
        val selected = DesktopDetailPath("s2", "e2")
        assertEquals(selected, reconcileDesktopDetailPath(detail, selected))
        assertEquals(
            DesktopDetailPath("s2"),
            reconcileDesktopDetailPath(
                detail.copy(
                    children =
                        detail.children.filterNot {
                            it.id == "e2"
                        },
                ),
                selected,
            ),
        )
        assertEquals(
            DesktopDetailPath(),
            reconcileDesktopDetailPath(detail.copy(children = detail.children.filterNot { it.id == "s2" }), selected),
        )
    }

    @Test
    fun byteSizesAreStableWithoutDependingOnTheDesktopLocale() {
        assertEquals("0 B", desktopFileSize(0))
        assertEquals("1023 B", desktopFileSize(1023))
        assertEquals("1.0 KiB", desktopFileSize(1024))
        assertEquals("1.0 GiB", desktopFileSize(1_073_741_824))
    }

    @Test
    fun movieAndGenericVersionsKeepTheirOwners() {
        val version = desktopSeriesFixture().children.first { it.id == "e2" }.versions
        val movie =
            desktopSeriesFixture().copy(
                item = desktopSeriesFixture().item.copy(type = MediaItemSummary.Type.movie),
                children = emptyList(),
                versions = version,
            )
        assertEquals(version, desktopDetailVersions(movie, DesktopDetailPath()))
        val generic =
            movie.copy(
                item = movie.item.copy(type = MediaItemSummary.Type.genericMinusVideo),
                versions = emptyList(),
                children = listOf(MediaChild("member", null, MediaChild.Type.genericMinusVideoMinusItem, "Member", 0, version)),
            )
        assertEquals(listOf("member"), desktopDetailNodes(generic, DesktopDetailPath()).map { it.id })
        assertEquals(version, desktopDetailVersions(generic, DesktopDetailPath(childId = "member")))
        assertEquals(version, desktopDetailVersions(desktopSeriesFixture(), DesktopDetailPath("s7")))
    }
}

internal fun desktopSeriesFixture(): MediaItemDetail {
    val versions = listOf(MediaVersion("v", "1080p", listOf(MediaFileAsset("f", "source.mkv", "episode.mkv", 1_048_576))))
    return MediaItemDetail(
        MediaItemSummary(
            "root",
            MediaItemSummary.Type.series,
            "library",
            "Series fixture",
            2026,
            MediaItemSummary.LocalStatus.complete,
            null,
            Instant.parse("2026-10-28T00:00:00Z"),
        ),
        emptyList(),
        emptyList(),
        listOf(
            MediaChild("e10", "s2", MediaChild.Type.episode, "Episode 10", 10, versions),
            MediaChild("s7", null, MediaChild.Type.season, "Season 7", 7, versions),
            MediaChild("e2", "s2", MediaChild.Type.episode, "Episode 2", 2, versions),
            MediaChild("s2", null, MediaChild.Type.season, "Season 2", 2, emptyList()),
        ),
        MediaItemDetail.NfoStatus.complete,
        emptySet(),
    )
}
