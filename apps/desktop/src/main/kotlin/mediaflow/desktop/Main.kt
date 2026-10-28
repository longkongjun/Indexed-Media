package mediaflow.desktop

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyShortcut
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.MenuBar
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import dev.indexed.design.desktop.IndexedTheme
import dev.indexed.design.desktop.PageMode
import dev.indexed.design.desktop.RequestState
import kotlinx.coroutines.launch
import mediaflow.client.core.paging.PageState
import java.awt.event.WindowAdapter
import java.awt.event.WindowEvent

fun main() =
    application {
        val model = remember { DesktopModel(appDirectory()) }
        val scope = rememberCoroutineScope()
        var closing by remember { mutableStateOf(false) }
        Window(title = "MediaFlow", onCloseRequest = {
            if (!closing) {
                closing = true
                scope.launch {
                    model.close()
                    exitApplication()
                }
            }
        }) {
            RefreshOnWindowFocus(window, model::refresh)
            MenuBar {
                Menu("文件") {
                    val mac = System.getProperty("os.name").startsWith("Mac")
                    Item("刷新", onClick = model::refresh, shortcut = KeyShortcut(Key.R, meta = mac, ctrl = !mac))
                    Item("退出应用", onClick = {
                        if (!closing) {
                            closing = true
                            scope.launch {
                                model.close()
                                exitApplication()
                            }
                        }
                    })
                }
            }
            IndexedTheme(dark = isSystemInDarkTheme()) { Companion(model) }
        }
    }

@Composable
internal fun RefreshOnWindowFocus(
    window: java.awt.Window,
    onRefresh: () -> Unit,
) {
    val refresh by rememberUpdatedState(onRefresh)
    DisposableEffect(window) {
        val listener =
            object : WindowAdapter() {
                override fun windowGainedFocus(event: WindowEvent?) {
                    refresh()
                }
            }
        window.addWindowFocusListener(listener)
        onDispose { window.removeWindowFocusListener(listener) }
    }
}

@Composable
internal fun Companion(model: DesktopModel) {
    val ui by model.state.collectAsState()
    LaunchedEffect(model) {
        val server = model.rememberedServer()
        if (server.isNotBlank()) model.login(server, true, "", "", "", restore = true)
    }
    var section by remember { mutableStateOf("媒体") }
    var detailOpen by remember { mutableStateOf(false) }
    var localError by remember { mutableStateOf<String?>(null) }
    val cancelFocus = remember { FocusRequester() }
    var confirm by remember { mutableStateOf<Pair<String, () -> Unit>?>(null) }
    LaunchedEffect(ui.loggedIn) {
        if (!ui.loggedIn) {
            confirm = null
            detailOpen = false
        }
    }
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        (localError ?: ui.message)?.let { message ->
            Row(Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite }) {
                Text(message, Modifier.weight(1f))
                TextButton(onClick = {
                    localError = null
                    model.dismissMessage()
                }) { Text("关闭提示") }
            }
        }
        if (!ui.loggedIn) {
            Login(model, ui.busy)
            return@Column
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("MediaFlow", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.weight(1f))
            TextButton(onClick = model::refresh) { Text("刷新") }
            TextButton(enabled = !ui.writeBusy && !ui.writeUnknown, onClick = {
                detailOpen = false
                model.logout()
            }) { Text("退出登录") }
        }
        if (ui.writeUnknown) {
            when (ui.writeOperation) {
                DesktopWriteOperation.TaskCancel ->
                    if (section != "任务") Text("上次任务取消结果未知，请返回任务页核对。")
                DesktopWriteOperation.TaskRetry ->
                    if (section != "任务") Text("上次任务重试结果未知，请返回任务页核对。")
                DesktopWriteOperation.ReviewDecision ->
                    if (section != "人工确认") Text("上次人工确认提交结果未知，请返回人工确认页核对。")
                null -> Unit
            }
        }
        Row(Modifier.weight(1f).fillMaxWidth()) {
            NavigationRail {
                for (item in listOf("媒体", "任务", "人工确认")) {
                    NavigationRailItem(
                        selected = section == item,
                        onClick = {
                            if (section != item) {
                                section = item
                                detailOpen = false
                                model.refresh()
                            }
                        },
                        icon = { Text(item.take(1), Modifier.clearAndSetSemantics {}) },
                        label = { Text(item) },
                    )
                }
            }
            BoxWithConstraints(Modifier.weight(1f).fillMaxHeight()) {
                val wide = maxWidth >= 900.dp
                when (section) {
                    "媒体" ->
                        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                            if (wide || !detailOpen) {
                                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                    MediaFilters(ui, onApply = {
                                        detailOpen = false
                                        model.applyMediaFilter(it)
                                    }, onReloadLibraries = model::loadMediaLibraries)
                                    Page(
                                        ui.wallState,
                                        onRetry = { model.reloadWall() },
                                        emptyMessage =
                                            if (ui.mediaFilter ==
                                                mediaflow.client.core.media
                                                    .MediaFilter()
                                            ) {
                                                "媒体库暂无内容"
                                            } else {
                                                "没有符合筛选条件的媒体，请调整或清除筛选"
                                            },
                                    ) {
                                        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                            items(ui.wall, key = { it.id }) { item ->
                                                Card(onClick = {
                                                    detailOpen = true
                                                    model.detail(item.id)
                                                }, modifier = Modifier.fillMaxWidth()) {
                                                    Column(Modifier.padding(12.dp)) {
                                                        Text(item.title)
                                                        Text(
                                                            "${item.year ?: ""} · ${item.localStatus}",
                                                            style = MaterialTheme.typography.bodyMedium,
                                                        )
                                                    }
                                                }
                                            }
                                            if (ui.wallHasMore) {
                                                item {
                                                    TextButton(
                                                        enabled = ui.wallState !is PageState.Loading,
                                                        onClick = { model.reloadWall(true) },
                                                    ) { Text("加载更多") }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if (detailOpen) {
                                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                    TextButton(onClick = { detailOpen = false }) { Text("返回媒体列表") }
                                    if (ui.detailLoading) LinearProgressIndicator(Modifier.fillMaxWidth())
                                    ui.detail?.let { detail ->
                                        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                            item { Text(detail.item.title, style = MaterialTheme.typography.titleLarge) }
                                            item { Text("状态：${detail.item.localStatus}") }
                                            items(detail.metadata) { metadata -> Text("${metadata.field}：${metadata.value ?: "未提供"}") }
                                            items(
                                                detail.versions,
                                            ) { version -> Text("${version.label ?: "媒体版本"} · ${version.files.size} 个文件") }
                                            items(detail.children) { child -> Text(child.title) }
                                        }
                                    }
                                    ui.playbackUrl?.let { url ->
                                        Button(onClick = {
                                            runCatching { openExternal(url) }.onFailure { localError = "外部播放打开失败，请检查系统默认浏览器" }
                                        }) { Text("打开外部播放") }
                                    }
                                }
                            }
                        }
                    "任务" ->
                        Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (ui.writeUnknown &&
                                ui.writeOperation in setOf(DesktopWriteOperation.TaskCancel, DesktopWriteOperation.TaskRetry)
                            ) {
                                val retrying = ui.writeOperation == DesktopWriteOperation.TaskRetry
                                Text(if (retrying) "上次任务重试结果未知，请刷新核对。再次发送将沿用原幂等键。" else "上次任务取消结果未知，请刷新核对。再次发送将沿用原幂等键。")
                                TextButton(
                                    enabled = !ui.writeBusy,
                                    onClick = if (retrying) model::retryTaskRetry else model::retryTaskCancel,
                                ) {
                                    Text(if (retrying) "重发同一次任务重试" else "重试同一次取消")
                                }
                            }
                            Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                                if (wide || ui.selectedTaskId == null) {
                                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                        TaskFilters(ui, model::applyTaskFilter)
                                        Page(
                                            ui.taskState,
                                            onRetry = { model.reloadTasks() },
                                            emptyMessage = taskEmptyMessage(ui.taskFilter),
                                        ) {
                                            LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                                items(ui.tasks, key = { it.id }) { task ->
                                                    Card(onClick = { model.selectTask(task.id) }, modifier = Modifier.fillMaxWidth()) {
                                                        Column(Modifier.padding(12.dp)) {
                                                            Text(task.relativePath)
                                                            Text(
                                                                "${taskStatus(
                                                                    task.status.toString(),
                                                                )} · ${taskStage(task.stage.toString())}",
                                                            )
                                                            TaskActions(task, ui, onRetry = {
                                                                confirm = "重试任务：${task.relativePath}" to { model.retryTask(task) }
                                                            }, onCancel = {
                                                                confirm = "取消任务：${task.relativePath}" to { model.cancelTask(task) }
                                                            })
                                                        }
                                                    }
                                                }
                                                if (ui.taskHasMore) {
                                                    item {
                                                        TextButton(
                                                            enabled = ui.taskState !is PageState.Loading,
                                                            onClick = { model.reloadTasks(true) },
                                                        ) { Text("加载更多") }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                ui.selectedTaskId?.let { taskId ->
                                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                        TextButton(
                                            enabled = !ui.writeBusy && !ui.writeUnknown,
                                            onClick = model::clearTask,
                                        ) { Text("返回任务列表") }
                                        if (ui.taskDetailLoading) LinearProgressIndicator(Modifier.fillMaxWidth())
                                        ui.taskDetailMessage?.let {
                                            Text(it)
                                            TextButton(onClick = { model.selectTask(taskId) }) { Text("重试任务详情") }
                                        }
                                        ui.selectedTask?.let { task ->
                                            LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                                item { Text(task.relativePath, style = MaterialTheme.typography.titleLarge) }
                                                item { Text("任务状态：${taskStatus(task.status.toString())}") }
                                                item { Text("处理阶段：${taskStage(task.stage.toString())}") }
                                                item { Text("当前检查点：${taskCheckpoint(task.checkpoint.toString())}") }
                                                item { Text("原因：${taskReason(task.reason?.toString())}") }
                                                item { Text("已尝试：${task.attemptCount}次") }
                                                item { Text("下次自动重试：${task.nextRetryAt?.toString() ?: "未安排"}") }
                                                if (task.recovering) item { Text("服务器正在恢复此任务") }
                                                item {
                                                    TaskActions(task, ui, onRetry = {
                                                        confirm = "重试任务：${task.relativePath}" to { model.retryTask(task) }
                                                    }, onCancel = {
                                                        confirm = "取消任务：${task.relativePath}" to { model.cancelTask(task) }
                                                    })
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    else ->
                        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                            if (wide || ui.selectedCase == null) {
                                Column(Modifier.weight(1f)) {
                                    Page(ui.caseState, onRetry = { model.reloadCases() }) {
                                        LazyColumn(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                            items(ui.cases, key = { it.id }) { case ->
                                                OutlinedButton(
                                                    onClick = { model.selectCase(case.id) },
                                                    modifier = Modifier.fillMaxWidth(),
                                                ) {
                                                    Text(
                                                        case.titleHint ?: case.relativePath,
                                                    )
                                                }
                                            }
                                            item { TextButton(onClick = { model.reloadCases(true) }) { Text("加载更多") } }
                                        }
                                    }
                                }
                            }
                            ui.selectedCase?.let { case ->
                                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                    var query by remember(case.id) { mutableStateOf(case.titleHint ?: "") }
                                    var mediaType by remember(case.id) { mutableStateOf("movie") }
                                    Text(case.titleHint ?: case.relativePath, style = MaterialTheme.typography.titleLarge)
                                    Text("案件版本：${case.version}")
                                    OutlinedTextField(query, { query = it }, label = { Text("候选搜索") }, modifier = Modifier.fillMaxWidth())
                                    Row {
                                        FilterChip(mediaType == "movie", { mediaType = "movie" }, label = { Text("电影") })
                                        FilterChip(mediaType == "tv", { mediaType = "tv" }, label = { Text("剧集") })
                                        TextButton(
                                            enabled = query.isNotBlank() && !ui.writeBusy,
                                            onClick = { model.search(query, mediaType) },
                                        ) { Text("搜索") }
                                    }
                                    if (ui.writeUnknown && ui.writeOperation == DesktopWriteOperation.ReviewDecision) {
                                        Text("上次提交结果未知。先刷新核对；重试将沿用原幂等键。")
                                        Button(enabled = !ui.writeBusy, onClick = model::retryDecision) { Text("重试同一次提交") }
                                    }
                                    ui.receipt?.let { Text("提交回执：${it.state}") }
                                    LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                        items(ui.candidates.indices.toList()) { index ->
                                            val candidate = ui.candidates[index]
                                            OutlinedButton(
                                                enabled =
                                                    !ui.writeBusy &&
                                                        !ui.writeUnknown &&
                                                        ui.receipt == null &&
                                                        case.allowedActions.any { it.toString() == "select-provider-candidate" },
                                                onClick = {
                                                    confirm =
                                                        "确认选择：${candidate.title}" to
                                                        { model.chooseCandidate(candidate, case.id, case.version) }
                                                },
                                            ) { Text("${candidate.title} · ${candidate.providerId}") }
                                        }
                                    }
                                    Button(
                                        enabled =
                                            !ui.writeBusy &&
                                                !ui.writeUnknown &&
                                                ui.receipt == null &&
                                                case.allowedActions.any { it.toString() == "select-generic-video" },
                                        onClick = {
                                            confirm = "将此案件标记为通用视频" to { model.chooseGeneric(case.id, case.version) }
                                        },
                                    ) { Text("标记为通用视频") }
                                    TextButton(
                                        enabled = !ui.writeBusy && !ui.writeUnknown,
                                        onClick = { model.clearCase() },
                                    ) { Text("返回审核队列") }
                                }
                            }
                        }
                }
            }
        }
    }
    confirm?.let { action ->
        AlertDialog(
            onDismissRequest = { confirm = null },
            title = { Text(action.first) },
            text = { Text("请核对当前对象，提交后将由服务器执行。") },
            confirmButton = {
                TextButton(onClick = {
                    confirm = null
                    action.second()
                }) { Text("确认提交") }
            },
            dismissButton = {
                TextButton(onClick = { confirm = null }, modifier = Modifier.focusRequester(cancelFocus)) { Text("取消") }
                LaunchedEffect(Unit) { cancelFocus.requestFocus() }
            },
        )
    }
}

@Composable
private fun TaskActions(
    task: mediaflow.api.models.ProcessingTask,
    ui: DesktopUi,
    onRetry: () -> Unit,
    onCancel: () -> Unit,
) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (task.allowedActions.any { it.toString() == "retry" }) {
            TextButton(enabled = !ui.writeBusy && !ui.writeUnknown, onClick = onRetry) { Text("重试任务") }
        }
        if (task.allowedActions.any { it.toString() == "cancel" }) {
            TextButton(enabled = !ui.writeBusy && !ui.writeUnknown, onClick = onCancel) { Text("取消任务") }
        }
    }
}

@Composable
private fun Login(
    model: DesktopModel,
    busy: Boolean,
) {
    var server by remember { mutableStateOf("") }
    var pairing by remember { mutableStateOf(true) }
    var code by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("admin") }
    var password by remember { mutableStateOf("") }
    LaunchedEffect(model) {
        server = model.rememberedServer()
    }
    Column(Modifier.widthIn(max = 560.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("连接 MediaFlow", style = MaterialTheme.typography.titleLarge)
        OutlinedTextField(server, { server = it }, label = { Text("服务器地址") }, singleLine = true, enabled = !busy)
        Row {
            Switch(pairing, { pairing = it }, enabled = !busy)
            Text(if (pairing) "配对码登录" else "管理员凭据登录")
        }
        if (pairing) {
            OutlinedTextField(code, { code = it }, label = { Text("一次性配对码") }, singleLine = true, enabled = !busy)
        } else {
            OutlinedTextField(name, { name = it }, label = { Text("用户名") }, enabled = !busy)
            OutlinedTextField(password, {
                password = it
            }, label = { Text("密码") }, visualTransformation = PasswordVisualTransformation(), singleLine = true, enabled = !busy)
        }
        val credentialsReady = if (pairing) code.isNotBlank() else name.isNotBlank() && password.isNotEmpty()
        Button(enabled = !busy && server.isNotBlank() && credentialsReady, onClick = {
            model.login(server, pairing, code, name, password)
            password = ""
            code = ""
        }) { Text(if (busy) "正在连接" else "连接") }
    }
}

@Composable
private fun Page(
    state: PageState,
    onRetry: () -> Unit,
    emptyMessage: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val mode =
        when (state) {
            is PageState.Loading -> PageMode.Loading
            PageState.Content -> PageMode.Content
            PageState.Empty -> PageMode.Empty
            is PageState.Error -> PageMode.Error
            is PageState.Offline -> PageMode.Offline
        }
    val stale =
        when (state) {
            is PageState.Loading -> state.stale
            is PageState.Error -> state.stale
            is PageState.Offline -> state.stale
            else -> false
        }
    RequestState(
        mode,
        stale,
        if (state == PageState.Empty) emptyMessage else (state as? PageState.Error)?.let { readable(it.error) },
        onRetry,
    ) { Column(Modifier.fillMaxSize(), content = content) }
}
