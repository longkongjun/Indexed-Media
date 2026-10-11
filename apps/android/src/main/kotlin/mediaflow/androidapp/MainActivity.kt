package mediaflow.androidapp

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.launch
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val container = (application as MediaFlowApp).container
        setContent { AppRoot(container) }
    }
}

/** 六屏根路由：登录 →（媒体墙 / 审核 / 任务 三入口）→ 详情 / 确认。 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AppRoot(container: SdkContainer) {
    val factory = remember { VmFactory(container) }
    val scope = rememberCoroutineScope()
    val login: LoginViewModel = viewModel(factory = factory)
    val loggedIn by login.loggedIn.collectAsState()
    var backStack by remember { mutableStateOf(listOf<Screen>()) }

    LaunchedEffect(Unit) {
        if (container.restoreSession()) login.markRestored()
    }
    // 任意屏 401：凭据已被容器清空，统一回登录页。
    LaunchedEffect(Unit) {
        container.sessionExpired.collect {
            login.forceLoggedOut()
            backStack = listOf()
        }
    }

    MaterialTheme {
        if (!loggedIn) {
            LoginScreen(login)
            return@MaterialTheme
        }
        val screen = backStack.lastOrNull() ?: Screen.Wall
        BackHandler(enabled = backStack.isNotEmpty()) { backStack = backStack.dropLast(1) }
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(titleOf(screen)) },
                    navigationIcon = {
                        if (backStack.isNotEmpty()) {
                            OutlinedButton(onClick = { backStack = backStack.dropLast(1) }) { Text("返回") }
                        }
                    },
                    actions = {
                        if (backStack.isEmpty()) {
                            OutlinedButton(onClick = { scope.launch { container.authRepository.logout() } ; login.forceLoggedOut() }) { Text("退出") }
                        }
                    },
                )
            },
        ) { padding ->
            Column(
                Modifier
                    .fillMaxSize()
                    .padding(padding),
            ) {
                when (val s = screen) {
                    Screen.Wall -> {
                        TabBar { backStack = listOf(it) }
                        WallScreen(container, factory) { backStack = backStack + Screen.Detail(it.id, it.title) }
                    }
                    Screen.ReviewQueue -> {
                        TabBar { backStack = listOf(it) }
                        ReviewQueueScreen(container, factory) { backStack = backStack + Screen.Confirm(it) }
                    }
                    Screen.Tasks -> {
                        TabBar { backStack = listOf(it) }
                        TasksScreen(container, factory)
                    }
                    is Screen.Detail -> DetailScreen(container, factory, s)
                    is Screen.Confirm -> ConfirmScreen(container, factory, s)
                    Screen.Login -> LoginScreen(login)
                }
            }
        }
    }
}

private fun titleOf(screen: Screen): String =
    when (screen) {
        Screen.Login -> "登录"
        Screen.Wall -> "媒体库"
        Screen.ReviewQueue -> "人工确认"
        Screen.Tasks -> "任务中心"
        is Screen.Detail -> screen.title
        is Screen.Confirm -> "确认识别"
    }

@Composable
private fun TabBar(select: (Screen) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        FilterChip(selected = false, onClick = { select(Screen.Wall) }, label = { Text("媒体库") })
        FilterChip(selected = false, onClick = { select(Screen.ReviewQueue) }, label = { Text("确认") })
        FilterChip(selected = false, onClick = { select(Screen.Tasks) }, label = { Text("任务") })
    }
}

@Composable
fun LoginScreen(vm: LoginViewModel) {
    var baseUrl by remember { mutableStateOf("http://10.0.2.2:8642") }
    var pairing by remember { mutableStateOf(true) }
    var code by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("admin") }
    var password by remember { mutableStateOf("") }
    val busy by vm.busy.collectAsState()
    val error by vm.error.collectAsState()
    val deviceName = remember { android.os.Build.MODEL }
    Column(
        Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
    ) {
        Text("MediaFlow", style = MaterialTheme.typography.headlineMedium)
        OutlinedTextField(value = baseUrl, onValueChange = { baseUrl = it }, label = { Text("服务器地址") }, modifier = Modifier.fillMaxWidth())
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = pairing, onClick = { pairing = true }, label = { Text("配对码") })
            FilterChip(selected = !pairing, onClick = { pairing = false }, label = { Text("管理员密码") })
        }
        Text("设备名：$deviceName", style = MaterialTheme.typography.bodySmall)
        if (pairing) {
            OutlinedTextField(value = code, onValueChange = { code = it }, label = { Text("8 位配对码（Web 管理页签发）") }, modifier = Modifier.fillMaxWidth())
        } else {
            OutlinedTextField(value = name, onValueChange = { name = it }, label = { Text("管理员") }, modifier = Modifier.fillMaxWidth())
            OutlinedTextField(value = password, onValueChange = { password = it }, label = { Text("密码") }, modifier = Modifier.fillMaxWidth())
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(
            onClick = {
                if (pairing) {
                    vm.pair(baseUrl, code.trim(), deviceName)
                } else {
                    vm.login(baseUrl, name, password, deviceName)
                }
            },
            enabled =
                !busy && baseUrl.isNotBlank() &&
                    if (pairing) code.isNotBlank() else (name.isNotBlank() && password.isNotBlank()),
            modifier = Modifier.fillMaxWidth(),
        ) {
            if (busy) CircularProgressIndicator(Modifier.height(20.dp)) else Text(if (pairing) "配对并登录" else "登录")
        }
    }
}

@Composable
fun WallScreen(
    container: SdkContainer,
    factory: VmFactory,
    openDetail: (mediaflow.api.models.MediaItemSummary) -> Unit,
) {
    val vm: WallViewModel = viewModel(factory = factory)
    LaunchedEffect(Unit) { vm.start() }
    val items by vm.items.collectAsState()
    val state by vm.state.collectAsState()
    val visual = PageVisual.from(state)
    Column(Modifier.fillMaxSize()) {
        if (visual.stale && visual.mode != PageVisual.Mode.Loading) {
            Text(
                visual.message ?: "内容可能已过期",
                Modifier.padding(8.dp),
                color = MaterialTheme.colorScheme.tertiary,
            )
        }
        when (visual.mode) {
            PageVisual.Mode.Loading -> if (items.isEmpty()) LoadingBox() else WallList(vm, items, openDetail)
            PageVisual.Mode.Content -> WallList(vm, items, openDetail)
            PageVisual.Mode.Empty -> EmptyBox("媒体库为空")
            PageVisual.Mode.Error, PageVisual.Mode.Offline ->
                MessageAndRetry(visual.message ?: "", items.isNotEmpty()) { vm.reload() }
        }
    }
}

@Composable
private fun WallList(
    vm: WallViewModel,
    items: List<mediaflow.api.models.MediaItemSummary>,
    openDetail: (mediaflow.api.models.MediaItemSummary) -> Unit,
) {
    val listState = rememberLazyListState()
    LaunchedEffect(items.size) {
        val last = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
        if (items.isNotEmpty() && last >= items.size - 4) vm.loadNext()
    }
    LazyColumn(state = listState, modifier = Modifier.fillMaxSize()) {
        items(items, key = { it.id }) { item ->
            Card(
                onClick = { openDetail(item) },
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 4.dp),
            ) {
                Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        if (item.artworkRef?.state.toString() == "available") "▣" else "□",
                        style = MaterialTheme.typography.headlineSmall,
                    )
                    Column(Modifier.padding(start = 12.dp)) {
                        Text(item.title, style = MaterialTheme.typography.titleMedium)
                        Text(
                            "${typeLabel(item.type.toString())} · ${item.year ?: "—"} · ${statusLabel(item.localStatus.toString())}",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
        }
        item {
            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(12.dp),
                horizontalArrangement = Arrangement.Center,
            ) {
                OutlinedButton(onClick = { vm.loadNext() }) { Text("加载更多") }
            }
        }
    }
}

@Composable
fun DetailScreen(
    container: SdkContainer,
    factory: VmFactory,
    screen: Screen.Detail,
) {
    val vm: DetailViewModel = viewModel(factory = factory)
    val context = LocalContext.current
    LaunchedEffect(screen.mediaItemId) { vm.load(screen.mediaItemId) }
    val ui by vm.ui.collectAsState()
    when {
        ui.loading -> LoadingBox()
        ui.error != null -> MessageAndRetry(ui.error!!, false) { vm.load(screen.mediaItemId) }
        ui.detail != null -> {
            val detail = ui.detail!!
            LazyColumn(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                item {
                    Column(Modifier.padding(16.dp)) {
                        Text(detail.item.title, style = MaterialTheme.typography.headlineSmall)
                        Text("${typeLabel(detail.item.type.toString())} · ${detail.item.year ?: "—"}")
                        Spacer(Modifier.height(8.dp))
                        val playbackUrl = ui.playbackUrl
                        if (ui.playbackAvailable && playbackUrl != null) {
                            Button(onClick = {
                                context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(playbackUrl)))
                            }) { Text("外部播放（Jellyfin）") }
                        } else {
                            AssistChip(onClick = {}, label = { Text("外部播放不可用（Jellyfin 未配置或不健康）") })
                        }
                    }
                }
                if (detail.metadata.isNotEmpty()) {
                    item { SectionTitle("元数据") }
                    items(detail.metadata.size) { i ->
                        val meta = detail.metadata[i]
                        Text("${meta.field}: ${meta.value ?: "—"}", Modifier.padding(horizontal = 16.dp))
                    }
                }
                if (detail.versions.isNotEmpty()) {
                    item { SectionTitle("版本") }
                    items(detail.versions.size) { i ->
                        val version = detail.versions[i]
                        Text("· ${version.label ?: "主版本"}（${version.files.size} 个文件）", Modifier.padding(horizontal = 16.dp))
                    }
                }
                if (detail.children.isNotEmpty()) {
                    item { SectionTitle("分集/子项") }
                    items(detail.children.size) { i ->
                        Text("· ${detail.children[i].title}", Modifier.padding(horizontal = 16.dp))
                    }
                }
            }
        }
    }
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text,
        Modifier.padding(horizontal = 16.dp),
        style = MaterialTheme.typography.titleMedium,
    )
}

@Composable
fun ReviewQueueScreen(
    container: SdkContainer,
    factory: VmFactory,
    openConfirm: (String) -> Unit,
) {
    val vm: ReviewQueueViewModel = viewModel(factory = factory)
    LaunchedEffect(Unit) { vm.start() }
    val cases by vm.cases.collectAsState()
    val state by vm.state.collectAsState()
    val visual = PageVisual.from(state)
    when (visual.mode) {
        PageVisual.Mode.Loading -> if (cases.isEmpty()) LoadingBox() else CaseList(cases, openConfirm)
        PageVisual.Mode.Content -> CaseList(cases, openConfirm)
        PageVisual.Mode.Empty -> EmptyBox("没有待确认的案件")
        PageVisual.Mode.Error, PageVisual.Mode.Offline -> MessageAndRetry(visual.message ?: "", false) { vm.reload() }
    }
}

@Composable
private fun CaseList(
    cases: List<mediaflow.api.models.ReviewCase>,
    openConfirm: (String) -> Unit,
) {
    LazyColumn(Modifier.fillMaxSize()) {
        items(cases, key = { it.id }) { case ->
            Card(
                onClick = { openConfirm(case.id) },
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 4.dp),
            ) {
                Column(Modifier.padding(12.dp)) {
                    Text(case.titleHint ?: case.relativePath, style = MaterialTheme.typography.titleMedium)
                    Text(
                        "级别 ${case.level} · 版本 ${case.version}",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
    }
}

@Composable
fun ConfirmScreen(
    container: SdkContainer,
    factory: VmFactory,
    screen: Screen.Confirm,
) {
    val vm: ConfirmViewModel = viewModel(factory = factory)
    LaunchedEffect(screen.reviewCaseId) { vm.load(screen.reviewCaseId) }
    val ui by vm.ui.collectAsState()
    var query by remember { mutableStateOf("") }
    val mediaType by vm.mediaType.collectAsState()
    when {
        ui.loading -> LoadingBox()
        ui.error != null && ui.case == null -> MessageAndRetry(ui.error!!, false) { vm.load(screen.reviewCaseId) }
        ui.case != null -> {
            val case = ui.case!!
            Column(
                Modifier
                    .fillMaxSize()
                    .padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(case.titleHint ?: case.relativePath, style = MaterialTheme.typography.titleLarge)
                Text("原因 ${case.reason}", style = MaterialTheme.typography.bodySmall)
                val receipt = ui.submitted
                if (receipt != null) {
                    Text(
                        "已提交（${receipt.state}）· 案件版本 ${receipt.caseVersion}",
                        color = MaterialTheme.colorScheme.primary,
                    )
                } else {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(selected = mediaType == "movie", onClick = { vm.mediaType.value = "movie" }, label = { Text("电影") })
                        FilterChip(selected = mediaType == "tv", onClick = { vm.mediaType.value = "tv" }, label = { Text("剧集") })
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedTextField(
                            value = query,
                            onValueChange = { query = it },
                            label = { Text("搜索标题") },
                            modifier = Modifier.weight(1f),
                        )
                        Button(
                            onClick = { vm.search(screen.reviewCaseId, query) },
                            enabled = query.isNotBlank() && !ui.searching,
                        ) { Text("搜索") }
                    }
                    if (ui.searching) LoadingBox()
                    LazyColumn(Modifier.weight(1f)) {
                        items(ui.candidates.size) { index ->
                            val candidate = ui.candidates[index]
                            Card(
                                onClick = { vm.submitSelectCandidate(index) },
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 4.dp),
                            ) {
                                Column(Modifier.padding(12.dp)) {
                                    Text(candidate.title, style = MaterialTheme.typography.titleMedium)
                                    Text(
                                        "${candidate.provider} · ${candidate.mediaType} · ${candidate.year ?: "—"}",
                                        style = MaterialTheme.typography.bodySmall,
                                    )
                                }
                            }
                        }
                    }
                    OutlinedButton(
                        onClick = { vm.submitGenericVideo() },
                        enabled = !ui.submitting,
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text("标记为通用视频（不做影片识别）") }
                    ui.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            }
        }
    }
}

@Composable
fun TasksScreen(
    container: SdkContainer,
    factory: VmFactory,
) {
    val vm: TasksViewModel = viewModel(factory = factory)
    LaunchedEffect(Unit) { vm.start() }
    val tasks by vm.tasks.collectAsState()
    val state by vm.state.collectAsState()
    val cancelling by vm.cancelling.collectAsState()
    val error by vm.error.collectAsState()
    val visual = PageVisual.from(state)
    Column(Modifier.fillMaxSize()) {
        when (visual.mode) {
            PageVisual.Mode.Loading -> if (tasks.isEmpty()) LoadingBox() else TaskList(vm, tasks, cancelling)
            PageVisual.Mode.Content -> TaskList(vm, tasks, cancelling)
            PageVisual.Mode.Empty -> EmptyBox("没有处理中的任务")
            PageVisual.Mode.Error, PageVisual.Mode.Offline -> MessageAndRetry(visual.message ?: "", tasks.isNotEmpty()) { vm.start() }
        }
        error?.let {
            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(8.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(it, color = MaterialTheme.colorScheme.error)
                OutlinedButton(onClick = { vm.dismissError() }) { Text("关闭") }
            }
        }
    }
}

@Composable
private fun TaskList(
    vm: TasksViewModel,
    tasks: List<mediaflow.api.models.ProcessingTask>,
    cancelling: Set<String>,
) {
    LazyColumn(Modifier.fillMaxSize()) {
        items(tasks, key = { it.id }) { task ->
            Card(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 4.dp),
            ) {
                Row(
                    Modifier
                        .padding(12.dp)
                        .fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(task.relativePath, style = MaterialTheme.typography.titleSmall)
                        Text("${task.status} · ${task.stage}", style = MaterialTheme.typography.bodySmall)
                    }
                    val cancellable = task.allowedActions.any { it.toString() == "cancel" }
                    if (cancellable) {
                        OutlinedButton(
                            onClick = { vm.cancel(task.id) },
                            enabled = task.id !in cancelling,
                        ) { Text(if (task.id in cancelling) "取消中…" else "取消") }
                    }
                }
            }
        }
    }
}

@Composable
private fun LoadingBox() {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        CircularProgressIndicator()
        Spacer(Modifier.height(8.dp))
        Text("加载中…")
    }
}

@Composable
private fun EmptyBox(message: String) {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) { Text(message) }
}

@Composable
private fun MessageAndRetry(
    message: String,
    keepContent: Boolean,
    retry: () -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(message, color = if (keepContent) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.error)
        OutlinedButton(onClick = retry) { Text("重试") }
    }
}

private fun typeLabel(type: String): String =
    when (type) {
        "movie" -> "电影"
        "series" -> "剧集"
        else -> "通用视频"
    }

private fun statusLabel(status: String): String = if (status == "complete") "完整" else "部分"
