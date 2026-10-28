package mediaflow.tv

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
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
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Surface
import androidx.tv.material3.Text
import dev.indexed.design.tv.IndexedTvButton

/** 仅设备本地偏好与当前连接信息；Dialog关闭后恢复原按钮，确认绑定发起身份。 */
@Composable
internal fun TvSettings(
    ui: TvUi,
    theme: TvTheme,
    version: String,
    onTheme: (TvTheme) -> Unit,
    onLogout: (Long) -> Unit,
) {
    var confirming by remember(ui.identity) { mutableStateOf(false) }
    var restoreExit by remember(ui.identity) { mutableStateOf(false) }
    val exit = remember { FocusRequester() }
    LaunchedEffect(confirming, restoreExit) {
        if (!confirming && restoreExit) {
            exit.requestFocus()
            restoreExit = false
        }
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(24.dp)) {
        Text("连接与设备", style = MaterialTheme.typography.headlineMedium)
        TvReadableText("设备：${ui.deviceName ?: "Android TV设备"}")
        TvReadableText("服务器：${ui.server}")
        Text("权限：媒体只读")
        Text("应用版本：$version")
        Text("外观")
        for ((choice, label) in listOf(TvTheme.System to "跟随系统", TvTheme.Light to "浅色", TvTheme.Dark to "深色")) {
            IndexedTvButton(
                "${if (theme == choice) "✓ " else ""}$label",
                onClick = { onTheme(choice) },
                modifier = Modifier.semantics { selected = theme == choice },
            )
        }
        IndexedTvButton("断开并退出设备", onClick = { confirming = true }, modifier = Modifier.focusRequester(exit))
    }
    if (confirming) {
        TvLogoutDialog(ui, onCancel = {
            confirming = false
            restoreExit = true
        }, onConfirm = {
            if (confirming) {
                confirming = false
                onLogout(ui.identity)
            }
        })
    }
}

@Composable
private fun TvLogoutDialog(
    ui: TvUi,
    onCancel: () -> Unit,
    onConfirm: () -> Unit,
) {
    val cancel = remember { FocusRequester() }
    Dialog(onDismissRequest = onCancel) {
        Surface(shape = MaterialTheme.shapes.large) {
            Column(
                Modifier.widthIn(max = 760.dp).verticalScroll(rememberScrollState()).padding(32.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                Text("退出此设备？", style = MaterialTheme.typography.headlineMedium)
                TvReadableText("设备：${ui.deviceName ?: "Android TV设备"}")
                TvReadableText("服务器：${ui.server}")
                Text("退出后需重新配对。服务器上的媒体和任务会保留。")
                Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    IndexedTvButton("取消", onClick = onCancel, modifier = Modifier.focusRequester(cancel))
                    IndexedTvButton("退出此设备", onClick = onConfirm)
                }
                LaunchedEffect(Unit) { cancel.requestFocus() }
            }
        }
    }
}
