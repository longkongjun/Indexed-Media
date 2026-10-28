package mediaflow.desktop

import mediaflow.api.models.MediaChild
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemSummary
import mediaflow.api.models.MediaVersion

/** 只记录当前根详情内的选择；子节点ID不参与HTTP路由。 */
internal data class DesktopDetailPath(
    val seasonId: String? = null,
    val childId: String? = null,
)

/** 保留Core的节点角色与父级，ordinal相同时按稳定ID排序。 */
internal fun desktopDetailNodes(
    detail: MediaItemDetail,
    path: DesktopDetailPath,
): List<MediaChild> {
    if (path.childId != null) return emptyList()
    val type =
        when {
            path.seasonId != null && detail.item.type == MediaItemSummary.Type.series -> MediaChild.Type.episode
            path.seasonId == null && detail.item.type == MediaItemSummary.Type.series -> MediaChild.Type.season
            path.seasonId == null && detail.item.type == MediaItemSummary.Type.genericMinusVideo ->
                MediaChild.Type.genericMinusVideoMinusItem
            else -> return emptyList()
        }
    return detail.children
        .filter { it.type == type && it.parentId == path.seasonId }
        .sortedWith(compareBy({ it.ordinal }, { it.id }))
}

/** 刷新只向有效祖先回退，不自动跳到兄弟节点或借用别的版本。 */
internal fun reconcileDesktopDetailPath(
    detail: MediaItemDetail,
    path: DesktopDetailPath,
): DesktopDetailPath {
    val season =
        path.seasonId?.let { id ->
            desktopDetailNodes(detail, DesktopDetailPath()).firstOrNull {
                it.id == id &&
                    it.type == MediaChild.Type.season
            }
        }
    if (path.seasonId != null && season == null) return DesktopDetailPath()
    val parent = DesktopDetailPath(seasonId = season?.id)
    val child =
        path.childId?.let { id ->
            desktopDetailNodes(detail, parent).firstOrNull { it.id == id && it.type != MediaChild.Type.season }
        }
    return parent.copy(childId = child?.id)
}

internal fun desktopDetailVersions(
    detail: MediaItemDetail,
    path: DesktopDetailPath,
): List<MediaVersion> {
    if (reconcileDesktopDetailPath(detail, path) != path) return emptyList()
    val id = path.childId ?: path.seasonId
    return if (id == null) detail.versions else detail.children.first { it.id == id }.versions
}
