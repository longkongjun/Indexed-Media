package mediaflow.tv

import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.tv.material3.MaterialTheme
import dev.indexed.design.tv.IndexedTvButton
import dev.indexed.design.tv.IndexedTvContentCard
import kotlinx.coroutines.flow.first
import mediaflow.api.models.MediaChild

@Composable
internal fun TvDetail(
    ui: TvUi,
    model: TvModel,
    images: TvArtworkCache,
    open: (String) -> Unit,
) {
    Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(32.dp)) {
        ui.detail?.item?.let { item -> TvPoster(ui, model, images, item.id, Modifier.width(240.dp)) }
        TvDetailPanel(ui, model::backInDetail, { model.selectDetailNode(it) }, model::refresh, open, Modifier.weight(1f))
    }
}

/** 层级页面只消费当前根投影；父级保存节点锚点，子层关闭后恢复原卡和可见滚动位置。 */
@Composable
internal fun TvDetailPanel(
    ui: TvUi,
    onBack: () -> Unit,
    onSelect: (String) -> Unit,
    onRefresh: () -> Unit,
    open: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    key(ui.identity, ui.detailMediaId) {
        val panels = rememberSaveableStateHolder()
        panels.SaveableStateProvider("${ui.detailPath.seasonId}/${ui.detailPath.childId}") {
            val context = LocalContext.current
            val detail = ui.detail
            val path = ui.detailPath
            val nodes = detail?.let { tvDetailNodes(it, path) }.orEmpty()
            val metadata =
                detail
                    ?.metadata
                    .orEmpty()
                    .takeIf { path == TvDetailPath() }
                    .orEmpty()
            val versions = detail?.let { tvDetailVersions(it, path) }.orEmpty()
            val selected = (path.childId ?: path.seasonId)?.let { id -> detail?.children?.firstOrNull { it.id == id } }
            val season = path.seasonId?.let { id -> detail?.children?.firstOrNull { it.id == id } }
            val nodeKeys = nodes.map { "node:${it.id}" }
            val emptyVersions = versions.isEmpty() && nodes.isEmpty() && detail != null
            val keys =
                buildList {
                    addAll(listOf("back", "title", "context"))
                    addAll(metadata.map { "metadata:${it.field}" })
                    addAll(nodeKeys)
                    if (emptyVersions) add("empty-versions")
                    versions.forEach { version ->
                        add("version:${version.id}")
                        addAll(version.files.map { "file:${version.id}:${it.id}" })
                    }
                    addAll(listOf("external", "refresh"))
                }
            val currentKeys by rememberUpdatedState(keys)
            val currentNodeKeys by rememberUpdatedState(nodeKeys)
            val list = rememberLazyListState()
            // 存活item保留同一requester；删去过时引用，不随刷新累积。
            val requesterCache = remember { mutableMapOf<String, FocusRequester>() }
            val requesters =
                remember(keys) {
                    keys
                        .associateWith { requesterCache.getOrPut(it) { FocusRequester() } }
                        .also { requesterCache.keys.retainAll(it.keys) }
                }
            var anchor by rememberSaveable { mutableStateOf("back") }
            var anchorIndex by rememberSaveable { mutableStateOf(0) }
            var nodeIndex by rememberSaveable { mutableStateOf(0) }
            var entering by remember { mutableStateOf(true) }
            var activeKey by remember { mutableStateOf<String?>(null) }

            fun rememberFocus(itemKey: String) {
                anchor = itemKey
                anchorIndex = currentKeys.indexOf(itemKey).coerceAtLeast(0)
                if (itemKey in currentNodeKeys) nodeIndex = currentNodeKeys.indexOf(itemKey)
                activeKey = itemKey
            }

            fun focus(itemKey: String): Modifier =
                Modifier.focusRequester(requesters.getValue(itemKey)).onFocusChanged {
                    if (it.isFocused) {
                        rememberFocus(itemKey)
                    } else if (activeKey == itemKey && itemKey in currentKeys) {
                        activeKey = null
                    }
                }
            LaunchedEffect(keys, ui.detailLoading) {
                val removedFocusedItem = activeKey != null && activeKey !in keys
                if ((entering || removedFocusedItem) && (!ui.detailLoading || detail != null)) {
                    val target =
                        if (anchor.startsWith("node:") && anchor !in keys) {
                            restoredMediaId(nodeKeys, null, nodeIndex) ?: "back"
                        } else {
                            requireNotNull(restoredMediaId(keys, anchor, anchorIndex))
                        }
                    snapshotFlow { list.layoutInfo.visibleItemsInfo }.first { it.isNotEmpty() }
                    if (list.layoutInfo.visibleItemsInfo.none { it.key == target }) list.scrollToItem(keys.indexOf(target))
                    snapshotFlow { list.layoutInfo.visibleItemsInfo.any { it.key == target } }.first { it }
                    requesters.getValue(target).requestFocus()
                    activeKey = target
                    entering = false
                }
            }
            LazyColumn(modifier, state = list, verticalArrangement = Arrangement.spacedBy(16.dp)) {
                item(key = "back") {
                    val label =
                        when {
                            path.childId != null -> if (season != null) "返回季详情" else "返回媒体详情"
                            path.seasonId != null -> "返回剧集详情"
                            else -> "返回影视墙"
                        }
                    IndexedTvButton(label, onClick = onBack, modifier = focus("back"))
                    // 首次读取尚未结束时也有返回焦点，已有文件阅读锚点则等待原内容恢复。
                    LaunchedEffect(Unit) {
                        if (entering && anchor == "back" && ui.detailLoading && detail == null) {
                            requesters.getValue("back").requestFocus()
                            entering = false
                        }
                    }
                }
                item(
                    key = "title",
                ) {
                    TvReadableText(
                        selected?.title ?: detail?.item?.title ?: "影片详情",
                        focus("title"),
                        style = MaterialTheme.typography.headlineMedium,
                    )
                }
                item(key = "context") {
                    val status = if (detail?.item?.localStatus?.toString() == "complete") "完整入库" else "部分入库"
                    TvReadableText(
                        when {
                            ui.detailLoading -> "正在刷新详情，保留当前内容"
                            detail == null -> "详情暂不可用，请重新加载"
                            path == TvDetailPath() -> "${detail.item.year ?: ""} · $status"
                            season != null -> "所属媒体：${detail.item.title}\n当前季：${season.title}"
                            else -> "所属媒体：${detail.item.title}"
                        },
                        modifier = focus("context"),
                    )
                }
                items(metadata, key = { "metadata:${it.field}" }) { value ->
                    TvReadableText("${metadataLabel(value.field)}：${value.value ?: "未提供"}", modifier = focus("metadata:${value.field}"))
                }
                items(nodes, key = { "node:${it.id}" }) { node ->
                    val subtitle =
                        if (ui.detailLoading) {
                            "正在刷新，稍后选择"
                        } else if (node.type == MediaChild.Type.season) {
                            "${detail?.children?.count {
                                it.parentId == node.id && it.type == MediaChild.Type.episode
                            } ?: 0} 集 · ${node.versions.size} 个版本"
                        } else {
                            "${node.versions.size} 个版本 · 查看文件信息"
                        }
                    IndexedTvContentCard(
                        node.title,
                        subtitle,
                        onClick = {
                            if (!ui.detailLoading) {
                                rememberFocus("node:${node.id}")
                                onSelect(node.id)
                            }
                        },
                        modifier =
                            focus("node:${node.id}").semantics {
                                if (ui.detailLoading) {
                                    disabled()
                                    stateDescription = "正在刷新，暂不可选择"
                                }
                            },
                    )
                }
                if (emptyVersions) {
                    item(key = "empty-versions") { TvReadableText("当前内容暂无版本信息", modifier = focus("empty-versions")) }
                }
                versions.forEachIndexed { index, version ->
                    item(
                        key = "version:${version.id}",
                    ) {
                        TvReadableText(
                            "版本：${version.label ?: "未命名版本 ${index + 1}"} · ${version.files.size} 个文件",
                            modifier = focus("version:${version.id}"),
                        )
                    }
                    items(version.files, key = { "file:${version.id}:${it.id}" }) { file ->
                        TvReadableText(
                            "文件：${file.currentRelativePath}\n大小：${Formatter.formatShortFileSize(context, file.sizeBytes)}",
                            modifier = focus("file:${version.id}:${file.id}"),
                        )
                    }
                }
                item(key = "external") {
                    val url = ui.playbackUrl
                    if (url == null) {
                        TvReadableText("外部播放尚未配置或暂不可用，请在Web管理端检查连接。", modifier = focus("external"))
                    } else {
                        IndexedTvButton("打开外部播放服务", onClick = { open(url) }, modifier = focus("external"))
                    }
                }
                item(key = "refresh") { IndexedTvButton("重新加载详情", onClick = onRefresh, modifier = focus("refresh")) }
            }
        }
    }
}

private fun metadataLabel(field: String): String =
    when (field) {
        "title" -> "片名"
        "original_title" -> "原名"
        "overview" -> "简介"
        "year" -> "年份"
        else -> "资料"
    }
