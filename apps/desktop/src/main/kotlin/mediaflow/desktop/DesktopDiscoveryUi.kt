package mediaflow.desktop

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import mediaflow.client.core.media.MediaFilter
import mediaflow.client.core.tasks.TaskFilter

private const val MAX_SEARCH_LENGTH = 200

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun MediaFilters(
    ui: DesktopUi,
    onApply: (MediaFilter) -> Unit,
    onReloadLibraries: () -> Unit,
) {
    var query by remember(ui.mediaFilter) { mutableStateOf(ui.mediaFilter.query.orEmpty()) }
    var type by remember(ui.mediaFilter) { mutableStateOf(ui.mediaFilter.type) }
    var library by remember(ui.mediaFilter) { mutableStateOf(ui.mediaFilter.libraryId) }
    var status by remember(ui.mediaFilter) { mutableStateOf(ui.mediaFilter.localStatus) }
    val applyFilter = { onApply(MediaFilter(type = type, libraryId = library, localStatus = status, query = query)) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(
            query,
            { query = it.take(MAX_SEARCH_LENGTH) },
            label = { Text("搜索媒体") },
            supportingText = { Text("标题关键词，最多${MAX_SEARCH_LENGTH}字") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { applyFilter() }),
            modifier = Modifier.fillMaxWidth(),
        )
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChoice("媒体类型", type, listOf(null to "全部类型", "movie" to "电影", "series" to "剧集", "generic-video" to "通用视频")) { type = it }
            FilterChoice("媒体库", library, listOf(null to "全部媒体库") + ui.mediaLibraries.map { it.id to it.name }) { library = it }
            FilterChoice("入库状态", status, listOf(null to "全部状态", "complete" to "完整", "partial" to "部分入库")) { status = it }
        }
        if (ui.librariesLoading && ui.mediaLibraries.isEmpty()) Text("正在加载媒体库列表", style = MaterialTheme.typography.bodySmall)
        ui.libraryMessage?.let {
            Text(it, style = MaterialTheme.typography.bodySmall)
            TextButton(enabled = !ui.librariesLoading, onClick = onReloadLibraries) { Text("重试媒体库列表") }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(onClick = applyFilter) { Text("应用筛选") }
            TextButton(onClick = {
                query = ""
                type = null
                library = null
                status = null
                onApply(MediaFilter())
            }) { Text("清除筛选") }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun TaskFilters(
    ui: DesktopUi,
    onApply: (TaskFilter) -> Unit,
) {
    var query by remember(ui.taskFilter) { mutableStateOf(ui.taskFilter.query.orEmpty()) }
    var status by remember(ui.taskFilter) { mutableStateOf(ui.taskFilter.status) }
    var stage by remember(ui.taskFilter) { mutableStateOf(ui.taskFilter.stage) }
    val enabled = !ui.writeBusy && !ui.writeUnknown
    val applyFilter = { onApply(TaskFilter(ui.taskFilter.view, stage, status, query)) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for ((value, label) in taskViews) {
                FilterChip(
                    ui.taskFilter.view == value,
                    { onApply(ui.taskFilter.copy(view = value)) },
                    label = { Text(label) },
                    enabled = enabled,
                )
            }
        }
        OutlinedTextField(
            query,
            { query = it.take(MAX_SEARCH_LENGTH) },
            label = { Text("搜索任务") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
            enabled = enabled,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { applyFilter() }),
        )
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChoice("任务状态", status, listOf(null to "全部状态") + taskStatuses, enabled) { status = it }
            FilterChoice("处理阶段", stage, listOf(null to "全部阶段") + taskStages, enabled) { stage = it }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(enabled = enabled, onClick = applyFilter) { Text("应用筛选") }
            TextButton(enabled = enabled, onClick = {
                query = ""
                status = null
                stage = null
                onApply(TaskFilter())
            }) { Text("清除筛选") }
        }
    }
}

@Composable
private fun FilterChoice(
    label: String,
    value: String?,
    options: List<Pair<String?, String>>,
    enabled: Boolean = true,
    onSelect: (String?) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        OutlinedButton(enabled = enabled, onClick = { open = true }, modifier = Modifier.widthIn(max = 260.dp)) {
            Text("$label：${options.firstOrNull { it.first == value }?.second ?: "当前选项不可用"}")
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            for ((id, title) in options) {
                DropdownMenuItem(text = { Text(title) }, enabled = enabled, onClick = {
                    open = false
                    onSelect(id)
                })
            }
        }
    }
}

internal val taskViews = listOf("pending" to "待处理", "running" to "运行中", "completed" to "已完成", "all" to "全部任务")
internal val taskStatuses =
    listOf(
        "queued" to "排队中",
        "running" to "运行中",
        "waiting-confirmation" to "待人工确认",
        "paused" to "已暂停",
        "partial-success" to "部分成功",
        "completed" to "已完成",
        "failed" to "失败",
        "cancelled" to "已取消",
    )
internal val taskStages =
    listOf(
        "identification" to "识别",
        "planning" to "规划",
        "file-operation" to "文件整理",
        "nfo" to "写入NFO",
        "completion" to "完成",
    )

internal fun taskStatus(value: String): String = taskStatuses.firstOrNull { it.first == value }?.second ?: value

internal fun taskStage(value: String): String = taskStages.firstOrNull { it.first == value }?.second ?: value

internal fun taskEmptyMessage(filter: TaskFilter): String =
    if (filter.copy(view = "pending") != TaskFilter()) {
        "没有符合筛选条件的任务，请调整或清除筛选"
    } else {
        when (filter.view) {
            "pending" -> "当前没有待处理任务"
            "running" -> "当前没有运行中的任务"
            "completed" -> "当前没有已完成任务"
            else -> "当前没有任务"
        }
    }

internal fun taskReason(value: String?): String =
    when (value) {
        null -> "未提供原因"
        "identification.ambiguous", "identification.multiple-strong-candidates" -> "存在多个候选，需要确认"
        "identification.confirmed-external-id" -> "已按外部标识确认"
        "identification.confirmed-title-year" -> "已按标题和年份确认"
        "identification.no-candidate" -> "未找到匹配候选"
        "identification.probable-title" -> "标题匹配需要确认"
        "identification.provider-unavailable", "integration.unavailable" -> "外部服务暂不可用"
        "identification.provider-unauthorized", "integration.unauthorized" -> "外部服务认证失败"
        "identification.revision-changed" -> "文件已变化，需要重新识别"
        "auxiliary.sample" -> "样片已跳过"
        "auxiliary.trailer" -> "预告片已跳过"
        "auxiliary.extra" -> "附加内容已跳过"
        "watcher.unavailable" -> "文件监控暂不可用"
        "reconcile.failed" -> "文件核对失败"
        "integration.unconfigured" -> "尚未配置外部服务"
        "integration.healthy" -> "外部服务连接正常"
        "integration.rate-limited" -> "外部服务请求过于频繁"
        "organization.plan-paused" -> "整理计划已暂停，需要处理"
        "organization.io-temporary" -> "文件暂时无法访问"
        "organization.manual-review" -> "文件状态需要人工核对"
        "organization.nfo-failed" -> "元数据文件写入失败"
        "organization.catalog-unavailable" -> "媒体库暂不可用"
        else -> "服务器返回了新的处理原因，请查看服务器任务详情"
    }

internal fun taskCheckpoint(value: String): String =
    when (value) {
        "pending" -> "等待开始"
        "identification-complete" -> "识别完成"
        "planning-requested" -> "等待规划"
        "plan-prepared" -> "计划已准备"
        "execution-authorized" -> "执行已授权"
        "planning-paused" -> "规划已暂停"
        "file-operation-prepared" -> "文件操作已准备"
        "file-operation-executing" -> "正在整理文件"
        "file-operation-verified" -> "文件结果已核实"
        "file-operation-manual-review" -> "文件结果待人工核对"
        "nfo-pending" -> "等待写入元数据"
        "nfo-verified" -> "元数据已核实"
        "nfo-failed" -> "元数据写入失败"
        "local-result-prepared" -> "本地结果已准备"
        "catalog-committed" -> "已写入媒体库"
        "waiting-confirmation" -> "等待人工确认"
        "dependency-blocked" -> "前置条件未满足"
        "skipped-auxiliary" -> "附加文件已跳过"
        "cancelled" -> "已取消"
        else -> "等待服务器更新"
    }
