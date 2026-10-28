package mediaflow.tv

import android.app.AlertDialog
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import dev.indexed.design.tv.IndexedTvButton
import dev.indexed.design.tv.IndexedTvPosterCard
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.paging.PageState

@Composable
internal fun TvMediaWall(
    ui: TvUi,
    model: TvModel,
    images: TvArtworkCache,
) {
    val context = LocalContext.current
    val grid = rememberLazyGridState()
    val ids = ui.wall.map { it.id }
    val requesters = remember(ids) { ids.associateWith { FocusRequester() } }
    val toolbar = remember { FocusRequester() }
    val filterFocus = remember { FocusRequester() }
    val recovery = remember { FocusRequester() }
    var lastId by rememberSaveable { mutableStateOf<String?>(null) }
    var lastIndex by rememberSaveable { mutableStateOf(0) }
    var restore by rememberSaveable { mutableStateOf(true) }
    var filtersOpen by remember { mutableStateOf(false) }
    var dialog by remember { mutableStateOf<AlertDialog?>(null) }

    fun applyFilter(next: MediaFilter) {
        if (next != ui.filter) {
            lastId = null
            lastIndex = 0
        }
        restore = true
        model.filter(next)
    }
    val returnToMedia =
        Modifier.onPreviewKeyEvent {
            if (it.type == KeyEventType.KeyDown && it.key == Key.DirectionDown) {
                restore = true
                true
            } else {
                false
            }
        }
    DisposableEffect(Unit) { onDispose { dialog?.dismiss() } }
    LaunchedEffect(Unit) { restore = true }
    LaunchedEffect(ids, ui.page !is PageState.Loading, restore) {
        if (ui.page !is PageState.Loading && (restore || lastId !in ids)) {
            val target = restoredMediaId(ids, lastId, lastIndex)
            if (target == null) {
                recovery.requestFocus()
            } else {
                val index = ids.indexOf(target)
                snapshotFlow { grid.layoutInfo.visibleItemsInfo }.first { it.isNotEmpty() }
                if (grid.layoutInfo.visibleItemsInfo.none { it.key == target }) grid.scrollToItem(index)
                snapshotFlow { grid.layoutInfo.visibleItemsInfo.any { it.key == target } }.first { it }
                requesters[target]?.requestFocus()
            }
            restore = false
        }
    }
    Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            IndexedTvButton(
                "搜索：${ui.filter.query ?: "输入片名"}",
                onClick = {
                    dialog =
                        tvTextInput(context, "搜索影视标题", ui.filter.query.orEmpty(), onSubmit = {
                            applyFilter(ui.filter.copy(query = it.trim().takeIf(String::isNotEmpty)))
                        }, onDismiss = {
                            dialog = null
                            runCatching { toolbar.requestFocus() }
                        })
                },
                modifier = Modifier.focusRequester(toolbar).then(returnToMedia),
            )
            IndexedTvButton("筛选", onClick = { filtersOpen = true }, modifier = Modifier.focusRequester(filterFocus).then(returnToMedia))
            IndexedTvButton("清除筛选", onClick = { applyFilter(MediaFilter()) }, modifier = returnToMedia)
            Text("${ui.wall.size} 部", style = MaterialTheme.typography.bodyLarge)
        }
        when (val page = ui.page) {
            is PageState.Loading -> Text(if (page.stale) "正在刷新，保留现有影片" else "正在加载影片")
            is PageState.Offline -> Text("连接中断，保留已加载影片")
            is PageState.Error -> Text("影片暂时无法加载，请重试")
            PageState.Empty ->
                Text(
                    if (ui.filter ==
                        MediaFilter()
                    ) {
                        "媒体库还没有影片"
                    } else {
                        "没有符合筛选条件的影片"
                    },
                )
            PageState.Content -> Unit
        }
        if (ui.wall.isEmpty() && ui.page !is PageState.Loading) {
            IndexedTvButton(
                "${if (ui.filter ==
                    MediaFilter()
                ) {
                    "重新加载"
                } else {
                    "清除筛选"
                }}",
                onClick = {
                    if (ui.filter ==
                        MediaFilter()
                    ) {
                        model.refresh()
                    } else {
                        model.filter(
                            MediaFilter(),
                        )
                    }
                },
                modifier = Modifier.focusRequester(recovery),
            )
        }
        BoxWithConstraints(Modifier.weight(1f)) {
            // 同时以宽和高计算列数，保证20sp两行标题与整张海报均落在可见画布。
            val columns = tvPosterColumns(maxWidth.value, maxHeight.value, LocalDensity.current.fontScale)
            LazyVerticalGrid(
                columns = GridCells.Fixed(columns),
                state = grid,
                horizontalArrangement = Arrangement.spacedBy(24.dp),
                verticalArrangement = Arrangement.spacedBy(32.dp),
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(8.dp),
            ) {
                itemsIndexed(ui.wall, key = { _, item -> item.id }) { index, item ->
                    IndexedTvPosterCard(
                        item.title,
                        "${item.year ?: ""} · ${mediaTypeName(item.type.toString())}",
                        onClick = {
                            lastId = item.id
                            lastIndex = index
                            restore = true
                            model.detail(item.id)
                        },
                        modifier =
                            Modifier
                                .focusRequester(requesters.getValue(item.id))
                                .onFocusChanged {
                                    if (it.isFocused) {
                                        lastId = item.id
                                        lastIndex = index
                                    }
                                }.onPreviewKeyEvent {
                                    when {
                                        it.type != KeyEventType.KeyDown -> false
                                        it.key == Key.DirectionLeft && index % columns == 0 -> true
                                        it.key == Key.DirectionRight &&
                                            (index % columns == columns - 1 || index == ui.wall.lastIndex) -> true
                                        it.key == Key.DirectionUp && index < columns -> {
                                            toolbar.requestFocus()
                                            true
                                        }
                                        else -> false
                                    }
                                },
                        artwork = { TvPoster(ui, model, images, item.id) },
                    )
                }
                item(key = "paging", span = { GridItemSpan(maxLineSpan) }) {
                    IndexedTvButton(if (ui.hasMore) "加载更多影片" else "已显示全部影片", onClick = {
                        if (ui.hasMore) model.loadMore()
                    }, busy = ui.page is PageState.Loading)
                }
            }
        }
    }
    if (filtersOpen) {
        TvFilters(ui, onApply = {
            applyFilter(it)
            filtersOpen = false
        }, onClose = {
            filtersOpen = false
            runCatching { filterFocus.requestFocus() }
        })
    }
}

@Composable
internal fun TvPoster(
    ui: TvUi,
    model: TvModel,
    images: TvArtworkCache,
    id: String,
    modifier: Modifier = Modifier,
) {
    val image by produceState(PosterImage(null, true), id, ui.identity, ui.artworkEpoch) {
        val bitmap =
            try {
                images.load(model, id, ui.artworkEpoch)
            } catch (
                cancelled: CancellationException,
            ) {
                throw cancelled
            } catch (_: Exception) {
                null
            }
        value = PosterImage(bitmap, false)
    }
    Box(
        modifier.fillMaxWidth().aspectRatio(2f / 3f).background(MaterialTheme.colorScheme.surfaceVariant),
        contentAlignment = Alignment.Center,
    ) {
        val bitmap = image.bitmap
        if (bitmap == null) {
            Text(if (image.loading) "正在加载封面" else "封面暂不可用", style = MaterialTheme.typography.bodyLarge)
        } else {
            Image(bitmap.asImageBitmap(), contentDescription = null, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        }
    }
}

@Composable
internal fun TvDetail(
    ui: TvUi,
    model: TvModel,
    images: TvArtworkCache,
    open: (String) -> Unit,
) {
    val back = remember { FocusRequester() }
    var initialFocus by remember { mutableStateOf(true) }
    Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(32.dp)) {
        ui.detail?.item?.let { item -> TvPoster(ui, model, images, item.id, Modifier.width(240.dp)) }
        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            item {
                IndexedTvButton("返回影视墙", onClick = model::back, modifier = Modifier.focusRequester(back))
                // 等待Lazy子项真正装配，刷新或重新滚入时不抢夺阅读焦点。
                LaunchedEffect(Unit) {
                    if (initialFocus) {
                        back.requestFocus()
                        initialFocus = false
                    }
                }
            }
            item { if (ui.detailLoading) Text("正在加载影片详情") }
            ui.detail?.let { detail ->
                item { TvReadableText(detail.item.title, MaterialTheme.typography.headlineMedium) }
                item {
                    val status = if (detail.item.localStatus.toString() == "complete") "完整入库" else "部分入库"
                    TvReadableText("${detail.item.year ?: ""} · $status")
                }
                items(
                    detail.metadata,
                ) { metadata -> TvReadableText("${metadataLabel(metadata.field)}：${metadata.value ?: "未提供"}") }
                items(detail.versions) { version -> TvReadableText("${version.label ?: "媒体版本"} · ${version.files.size} 个文件") }
                items(detail.children) { child -> TvReadableText(child.title) }
            }
            item {
                val url = ui.playbackUrl
                if (url == null) {
                    TvReadableText("外部播放尚未配置或暂不可用，请在Web管理端检查连接。")
                } else {
                    IndexedTvButton("打开外部播放服务", onClick = { open(url) })
                }
            }
            item { IndexedTvButton("重新加载详情", onClick = model::refresh) }
        }
    }
}

private fun mediaTypeName(type: String): String =
    when (type) {
        "movie" -> "电影"
        "series" -> "剧集"
        else -> "通用视频"
    }

private fun metadataLabel(field: String): String =
    when (field) {
        "title" -> "片名"
        "original_title" -> "原名"
        "overview" -> "简介"
        "year" -> "年份"
        else -> "资料"
    }

private data class PosterImage(
    val bitmap: android.graphics.Bitmap?,
    val loading: Boolean,
)

@Composable
internal fun TvReadableText(
    text: String,
    style: TextStyle = MaterialTheme.typography.bodyLarge,
) {
    var focused by remember { mutableStateOf(false) }
    val scroll = rememberScrollState()
    val scrolling = rememberCoroutineScope()
    val step = with(LocalDensity.current) { 60.dp.toPx() }
    Text(
        text,
        Modifier
            .fillMaxWidth()
            .heightIn(max = 180.dp)
            .border(2.dp, if (focused) MaterialTheme.colorScheme.primary else Color.Transparent)
            .padding(8.dp)
            .verticalScroll(scroll)
            .onPreviewKeyEvent {
                val delta =
                    when {
                        it.type != KeyEventType.KeyDown -> 0f
                        it.key == Key.DirectionDown && scroll.canScrollForward -> step
                        it.key == Key.DirectionUp && scroll.canScrollBackward -> -step
                        else -> 0f
                    }
                if (delta != 0f) scrolling.launch { scroll.scrollBy(delta) }
                delta != 0f
            }.onFocusChanged { focused = it.isFocused }
            .focusable(),
        style = style,
    )
}
