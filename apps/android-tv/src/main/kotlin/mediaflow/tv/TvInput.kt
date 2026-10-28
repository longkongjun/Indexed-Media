package mediaflow.tv

import android.app.AlertDialog
import android.content.Context
import android.text.InputFilter
import android.text.InputType
import android.view.WindowManager
import android.widget.EditText
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import dev.indexed.design.tv.IndexedTvButton

@Composable
internal fun TvConnect(
    ui: TvUi,
    model: TvModel,
) {
    val context = LocalContext.current
    var server by rememberSaveable { mutableStateOf(ui.server) }
    var code by remember { mutableStateOf("") }
    var allowHttp by rememberSaveable { mutableStateOf(false) }
    var dialog by remember { mutableStateOf<AlertDialog?>(null) }
    val serverFocus = remember { FocusRequester() }
    val codeFocus = remember { FocusRequester() }
    LaunchedEffect(ui.server) { if (server.isBlank()) server = ui.server }
    LaunchedEffect(Unit) { serverFocus.requestFocus() }
    DisposableEffect(Unit) { onDispose { dialog?.dismiss() } }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(24.dp)) {
        Text("连接影视库", style = MaterialTheme.typography.headlineLarge)
        Text("先在Web管理端生成一次性配对码。此电视将申请只读访问。")
        IndexedTvButton("服务器：${server.ifBlank { "输入服务器地址" }}", onClick = {
            dialog =
                tvTextInput(
                    context,
                    "服务器地址",
                    server,
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI,
                    onSubmit = { server = it.trim() },
                    onDismiss = {
                        dialog = null
                        runCatching { serverFocus.requestFocus() }
                    },
                )
        }, modifier = Modifier.focusRequester(serverFocus), enabled = !ui.connecting)
        IndexedTvButton(if (code.isBlank()) "输入一次性配对码" else "配对码已输入", onClick = {
            dialog =
                tvTextInput(
                    context,
                    "一次性配对码",
                    code,
                    InputType.TYPE_CLASS_TEXT,
                    onSubmit = { code = it.trim() },
                    onDismiss = {
                        dialog = null
                        runCatching { codeFocus.requestFocus() }
                    },
                )
        }, modifier = Modifier.focusRequester(codeFocus), enabled = !ui.connecting)
        IndexedTvButton("${if (allowHttp) "✓ " else ""}允许可信局域网HTTP", onClick = { allowHttp = !allowHttp }, enabled = !ui.connecting)
        IndexedTvButton(
            "连接",
            onClick = {
                val submitted = code
                code = ""
                model.connect(server, allowHttp, submitted)
            },
            enabled =
                server.isNotBlank() && code.isNotBlank(),
            busy = ui.connecting,
        )
        IndexedTvButton("恢复已有设备连接", onClick = {
            model.connect(server, allowHttp, restore = true)
        }, enabled = server.isNotBlank(), busy = ui.connecting)
        ui.message?.let { Text(it) }
    }
}

/** 系统EditText/IME桥接；取消或提交都关闭原生窗口并把焦点还给调用方。 */
internal fun tvTextInput(
    context: Context,
    title: String,
    initial: String,
    type: Int = InputType.TYPE_CLASS_TEXT,
    onSubmit: (String) -> Unit,
    onDismiss: () -> Unit,
): AlertDialog {
    val input =
        EditText(context).apply {
            inputType = type
            filters = arrayOf(InputFilter.LengthFilter(200))
            importantForAutofill = android.view.View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
            isSingleLine = true
            setText(initial)
            setSelection(text.length)
            setPadding(24, 16, 24, 16)
        }
    return AlertDialog
        .Builder(context)
        .setTitle(title)
        .setView(input)
        .setPositiveButton("确认") { _, _ -> onSubmit(input.text.toString()) }
        .setNegativeButton("取消", null)
        .create()
        .apply {
            setOnDismissListener { onDismiss() }
            setOnShowListener {
                input.requestFocus()
                window?.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_VISIBLE)
            }
            show()
        }
}
