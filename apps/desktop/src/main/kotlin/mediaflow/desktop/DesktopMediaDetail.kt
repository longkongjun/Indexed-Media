package mediaflow.desktop

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.first
import mediaflow.api.models.MediaChild
import mediaflow.api.models.MediaItemDetail
import java.util.Locale

/** 子节点只从当前根投影读取；不发子ID请求，也不打开NAS文件。 */
@Composable
internal fun DesktopMediaDetail(
    detail: MediaItemDetail,
    modifier: Modifier = Modifier,
    onBack: () -> Unit,
) {
    var seasonId by rememberSaveable(detail.item.id) { mutableStateOf<String?>(null) }
    var childId by rememberSaveable(detail.item.id) { mutableStateOf<String?>(null) }
    val selected = DesktopDetailPath(seasonId, childId)
    val path = reconcileDesktopDetailPath(detail, selected)
    LaunchedEffect(detail, selected) {
        seasonId = path.seasonId
        childId = path.childId
    }
    val back = {
        when {
            path.childId != null -> childId = null
            path.seasonId != null -> seasonId = null
            else -> onBack()
        }
    }
    val backRequester = remember { FocusRequester() }
    val panels = key(detail.item.id) { rememberSaveableStateHolder() }
    Column(
        modifier.fillMaxSize().onPreviewKeyEvent {
            if (it.key == Key.Escape && it.type == KeyEventType.KeyDown) {
                back()
                true
            } else {
                false
            }
        },
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        TextButton(onClick = back, modifier = Modifier.focusRequester(backRequester)) {
            Text(
                when {
                    path.childId != null && path.seasonId != null -> "返回季详情"
                    path != DesktopDetailPath() -> "返回媒体详情"
                    else -> "返回媒体列表"
                },
            )
        }
        // Root identity also scopes every saved scroll/focus record.
        key(detail.item.id, path) {
            panels.SaveableStateProvider("${detail.item.id}:${path.seasonId}:${path.childId}") {
                DesktopDetailPanel(detail, path, backRequester) { child ->
                    if (child.type == MediaChild.Type.season) seasonId = child.id else childId = child.id
                }
            }
        }
    }
}

@Composable
private fun DesktopDetailPanel(
    detail: MediaItemDetail,
    path: DesktopDetailPath,
    backRequester: FocusRequester,
    onSelect: (MediaChild) -> Unit,
) {
    val nodes = desktopDetailNodes(detail, path)
    val versions = desktopDetailVersions(detail, path)
    val owner = detail.children.firstOrNull { it.id == (path.childId ?: path.seasonId) }
    val list = rememberLazyListState()
    var returnFocusId by rememberSaveable { mutableStateOf<String?>(null) }
    var returnFocusIndex by rememberSaveable { mutableStateOf(0) }
    val requesters = remember(nodes.map { it.id }) { nodes.associate { it.id to FocusRequester() } }
    LaunchedEffect(Unit) {
        val exact = nodes.indexOfFirst { it.id == returnFocusId }
        val index =
            if (exact >=
                0
            ) {
                exact
            } else if (returnFocusId != null && nodes.isNotEmpty()) {
                returnFocusIndex.coerceAtMost(nodes.lastIndex)
            } else {
                -1
            }
        if (index >= 0) {
            // A focused parent may be outside the lazy viewport when returning from a child.
            if (list.layoutInfo.visibleItemsInfo.none { it.key == "node:${nodes[index].id}" }) list.scrollToItem(index + 2)
            snapshotFlow { list.layoutInfo.visibleItemsInfo.any { it.key == "node:${nodes[index].id}" } }.first { it }
            withFrameNanos { }
            requesters.getValue(nodes[index].id).requestFocus()
        } else {
            withFrameNanos { }
            backRequester.requestFocus()
        }
    }
    LazyColumn(Modifier.fillMaxSize(), state = list, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item(key = "title") { Text(owner?.title ?: detail.item.title, style = MaterialTheme.typography.titleLarge) }
        item(key = "context") {
            Text(if (owner == null) "状态：${detail.item.localStatus}" else detail.item.title, style = MaterialTheme.typography.bodyMedium)
        }
        items(nodes, key = { "node:${it.id}" }) { child ->
            Card(
                onClick = {
                    returnFocusId = child.id
                    returnFocusIndex = nodes.indexOf(child)
                    onSelect(child)
                },
                modifier = Modifier.fillMaxWidth().focusRequester(requesters.getValue(child.id)),
            ) {
                Text(child.title, Modifier.padding(12.dp))
            }
        }
        if (nodes.isEmpty() && versions.isEmpty()) item(key = "empty") { Text("暂无可用版本或子项") }
        versions.forEach { version ->
            item(key = "version:${version.id}") { Text(version.label ?: "媒体版本", style = MaterialTheme.typography.titleMedium) }
            items(version.files, key = { "file:${version.id}:${it.id}" }) { file ->
                Column(Modifier.fillMaxWidth().padding(8.dp)) {
                    Text(file.currentRelativePath)
                    Text(desktopFileSize(file.sizeBytes), style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        if (path == DesktopDetailPath()) {
            items(detail.metadata, key = { "metadata:${it.field}" }) { metadata -> Text("${metadata.field}：${metadata.value ?: "未提供"}") }
        }
    }
}

internal fun desktopFileSize(bytes: Long): String =
    when {
        bytes >= 1_073_741_824 -> String.format(Locale.ROOT, "%.1f GiB", bytes / 1_073_741_824.0)
        bytes >= 1_048_576 -> String.format(Locale.ROOT, "%.1f MiB", bytes / 1_048_576.0)
        bytes >= 1024 -> String.format(Locale.ROOT, "%.1f KiB", bytes / 1024.0)
        else -> "$bytes B"
    }
