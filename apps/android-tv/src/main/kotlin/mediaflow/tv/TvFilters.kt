package mediaflow.tv

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Surface
import androidx.tv.material3.Text
import dev.indexed.design.tv.IndexedTvButton
import mediaflow.client.core.media.MediaFilter

@Composable
internal fun TvFilters(
    ui: TvUi,
    onApply: (MediaFilter) -> Unit,
    onClose: () -> Unit,
) {
    var draft by remember { mutableStateOf(ui.filter) }
    val cancel = remember { FocusRequester() }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(modifier = Modifier.fillMaxWidth(0.9f).widthIn(max = 864.dp), shape = MaterialTheme.shapes.large) {
            Column(
                Modifier.widthIn(max = 760.dp).verticalScroll(rememberScrollState()).padding(32.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                Text("筛选影视库", style = MaterialTheme.typography.headlineMedium)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    for ((value, title) in listOf(null to "全部类型", "movie" to "电影", "series" to "剧集", "generic-video" to "通用视频")) {
                        IndexedTvButton("${if (draft.type == value) "✓ " else ""}$title", onClick = { draft = draft.copy(type = value) })
                    }
                }
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    for ((value, title) in listOf(null to "全部状态", "complete" to "完整入库", "partial" to "部分入库")) {
                        IndexedTvButton("${if (draft.localStatus == value) "✓ " else ""}$title", onClick = {
                            draft =
                                draft.copy(localStatus = value)
                        })
                    }
                }
                Text("媒体库")
                IndexedTvButton("${if (draft.libraryId == null) "✓ " else ""}全部媒体库", onClick = { draft = draft.copy(libraryId = null) })
                for (library in ui.libraries) {
                    IndexedTvButton("${if (draft.libraryId == library.id) "✓ " else ""}${library.name}", onClick = {
                        draft =
                            draft.copy(libraryId = library.id)
                    })
                }
                Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                    IndexedTvButton("取消", onClick = onClose, modifier = Modifier.focusRequester(cancel))
                    IndexedTvButton("应用筛选", onClick = { onApply(draft) })
                }
                LaunchedEffect(Unit) { cancel.requestFocus() }
            }
        }
    }
}
