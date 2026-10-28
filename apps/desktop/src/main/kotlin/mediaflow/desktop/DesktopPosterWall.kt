package mediaflow.desktop

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.toComposeImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import mediaflow.api.models.MediaItemSummary
import mediaflow.client.core.media.Artwork
import org.jetbrains.skia.Bitmap
import org.jetbrains.skia.Codec
import org.jetbrains.skia.Data
import org.jetbrains.skia.Image
import org.jetbrains.skia.SamplingMode

/** Session-owned, memory-only images; visible tiles own their requests and cancellation. */
internal class DesktopPosterCache(
    private val budget: Int = 16 * 1024 * 1024,
) {
    private val images = LinkedHashMap<String, ImageBitmap>(16, 0.75f, true)
    private val permits = Semaphore(4)
    private var bytes = 0L
    private var closed = false

    suspend fun load(
        id: String,
        fetch: suspend (String) -> Artwork?,
    ): ImageBitmap? =
        permits.withPermit {
            synchronized(this) {
                if (closed) return@withPermit null
                images[id]?.let { return@withPermit it }
            }
            val artwork = fetch(id) ?: return@withPermit null
            val image = withContext(Dispatchers.Default) { decodeDesktopPoster(artwork.bytes) } ?: return@withPermit null
            currentCoroutineContext().ensureActive()
            synchronized(this) {
                if (closed) return@withPermit null
                val size = image.width.toLong() * image.height * 4
                if (size <= budget) {
                    images.remove(id)?.let { bytes -= it.width.toLong() * it.height * 4 }
                    while (bytes + size > budget && images.isNotEmpty()) {
                        val iterator = images.entries.iterator()
                        val oldest = iterator.next().value
                        bytes -= oldest.width.toLong() * oldest.height * 4
                        iterator.remove()
                    }
                    images[id] = image
                    bytes += size
                }
            }
            image
        }

    @Synchronized
    fun close() {
        closed = true
        images.clear()
        bytes = 0
    }
}

/** Read encoded dimensions before pixel allocation; Skia also supports Core's WebP response. */
internal fun decodeDesktopPoster(bytes: ByteArray): ImageBitmap? {
    if (bytes.isEmpty() || bytes.size > 2 * 1024 * 1024) return null
    return try {
        Data.makeFromBytes(bytes).use { data ->
            Codec.makeFromData(data).use { codec ->
                val width = codec.width
                val height = codec.height
                if (width !in 1..4096 || height !in 1..4096 || width.toLong() * height > 8_000_000) return null
                val ratio = maxOf(width, height).toDouble().div(1024).coerceAtLeast(1.0)
                Bitmap().use { target ->
                    check(target.allocN32Pixels((width / ratio).toInt().coerceAtLeast(1), (height / ratio).toInt().coerceAtLeast(1)))
                    Image.makeFromEncoded(bytes).use { image ->
                        requireNotNull(target.peekPixels()).use { pixels ->
                            check(image.scalePixels(pixels, SamplingMode.LINEAR, false))
                        }
                    }
                    Image.makeFromBitmap(target).toComposeImageBitmap()
                }
            }
        }
    } catch (_: IllegalArgumentException) {
        null
    } catch (_: IllegalStateException) {
        null
    }
}

@Composable
internal fun DesktopPosterWall(
    items: List<MediaItemSummary>,
    hasMore: Boolean,
    loading: Boolean,
    loadPoster: suspend (String) -> Artwork?,
    onSelect: (String) -> Unit,
    onMore: () -> Unit,
    modifier: Modifier = Modifier,
    gridState: LazyGridState = rememberLazyGridState(),
    selectedId: String? = null,
    restoreFocus: Boolean = true,
) {
    val focus = remember { FocusRequester() }
    val anchor =
        items.firstOrNull { it.id == selectedId }
            ?: items.getOrNull(gridState.firstVisibleItemIndex.coerceAtMost((items.size - 1).coerceAtLeast(0)))
    LaunchedEffect(anchor?.id, selectedId, restoreFocus, gridState) {
        if (selectedId != null && restoreFocus && anchor != null) {
            val index = items.indexOfFirst { it.id == anchor.id }
            if (gridState.layoutInfo.visibleItemsInfo.none { it.key == anchor.id }) gridState.scrollToItem(index)
            snapshotFlow { gridState.layoutInfo.visibleItemsInfo.any { it.key == anchor.id } }.first { it }
            withFrameNanos { }
            focus.requestFocus()
        }
    }
    val cache = remember { DesktopPosterCache() }
    val fetch by rememberUpdatedState(loadPoster)
    DisposableEffect(cache) { onDispose { cache.close() } }
    LazyVerticalGrid(
        columns = GridCells.Adaptive(160.dp),
        state = gridState,
        modifier = modifier,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        items(items, key = { it.id }) { item ->
            var retry by remember(item.id) { mutableStateOf(0) }
            var finished by remember(item.id, retry) { mutableStateOf(false) }
            val poster by produceState<ImageBitmap?>(null, item.id, retry) {
                try {
                    value = cache.load(item.id) { fetch(it) }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    value = null
                } finally {
                    finished = true
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Card(onClick = { onSelect(item.id) }, modifier = if (item.id == anchor?.id) Modifier.focusRequester(focus) else Modifier) {
                    Box(Modifier.fillMaxWidth().aspectRatio(2f / 3f), contentAlignment = Alignment.Center) {
                        val image = poster
                        if (image != null) {
                            Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                        } else {
                            Text(if (finished) "海报暂不可用" else "正在加载海报", Modifier.padding(8.dp))
                        }
                    }
                    Text(item.title, Modifier.padding(8.dp), maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text("${item.year ?: ""} · ${item.localStatus}", Modifier.padding(8.dp), style = MaterialTheme.typography.bodySmall)
                }
                if (finished && poster == null) TextButton(onClick = { retry++ }) { Text("重试海报：${item.title}") }
            }
        }
        if (hasMore) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                TextButton(enabled = !loading, onClick = onMore) { Text("加载更多") }
            }
        }
    }
}
