package mediaflow.tv

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import dev.indexed.design.tv.IndexedTvButton
import dev.indexed.design.tv.IndexedTvTheme

/** 正式TV入口；只读业务、平台会话和无业务Design组件在此装配。 */
class MainActivity : ComponentActivity() {
    private val holder: TvViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { TvApplication(holder.model) }
    }
}

@Composable
internal fun TvApplication(
    model: TvModel,
    version: String = "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
) {
    val ui by model.state.collectAsStateWithLifecycle()
    val theme by model.theme.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val images = remember(ui.identity) { TvArtworkCache() }
    val savedScreens = rememberSaveableStateHolder()
    var settings by remember(ui.identity) { mutableStateOf(false) }
    DisposableEffect(images, ui.identity) {
        val wallKey = "wall-${ui.identity}"
        onDispose {
            images.clear()
            savedScreens.removeState(wallKey)
        }
    }
    DisposableEffect(owner, model) {
        val observer =
            LifecycleEventObserver { _, event ->
                if (event == Lifecycle.Event.ON_RESUME && model.state.value.connected) model.refresh()
            }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    BackHandler(enabled = ui.connected && (ui.detailOpen || settings)) {
        if (ui.detailOpen) model.back() else settings = false
    }
    IndexedTvTheme(dark = theme.isDark(isSystemInDarkTheme())) {
        Column(
            Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(horizontal = 48.dp, vertical = 27.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            if (!ui.connected) {
                TvConnect(ui, model)
            } else {
                Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                    Text("MediaFlow", style = MaterialTheme.typography.headlineMedium)
                    IndexedTvButton("影视墙", onClick = {
                        settings = false
                        model.back()
                    })
                    IndexedTvButton("刷新", onClick = model::refresh)
                    IndexedTvButton("设置", onClick = {
                        model.back()
                        settings = true
                    })
                }
                ui.message?.let { message ->
                    Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                        Text(message, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                        IndexedTvButton("关闭提示", onClick = { model.message(null) })
                    }
                }
                when {
                    settings -> TvSettings(ui, theme, version, model::setTheme) { model.confirmLogout(it) }
                    ui.detailOpen ->
                        TvDetail(ui, model, images) { url ->
                            runCatching { openTvPlayback(context, url) }.onFailure { model.message("无法打开外部播放，请检查电视上是否有可处理该地址的应用") }
                        }
                    else -> savedScreens.SaveableStateProvider("wall-${ui.identity}") { TvMediaWall(ui, model, images) }
                }
            }
        }
    }
}
