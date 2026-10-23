package mediaflow.ui.tv

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.Border
import androidx.tv.material3.Button
import androidx.tv.material3.ButtonDefaults
import androidx.tv.material3.Card
import androidx.tv.material3.CardDefaults
import androidx.tv.material3.LocalContentColor
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import androidx.tv.material3.Typography
import androidx.tv.material3.darkColorScheme
import androidx.tv.material3.lightColorScheme

private val LocalFocusColor = staticCompositionLocalOf { Color(0xFF5CD39A) }

// labs/design-system/tokens/tokens.json v2.0.0，暂定evergreen语义子集。
private val Light =
    lightColorScheme(
        primary = Color(0xFF08744B),
        onPrimary = Color(0xFFFFFFFF),
        background = Color(0xFFF4F7F5),
        onBackground = Color(0xFF12231C),
        surface = Color(0xFFFFFFFF),
        onSurface = Color(0xFF12231C),
        onSurfaceVariant = Color(0xFF50655C),
        primaryContainer = Color(0xFFE8F5EE),
        onPrimaryContainer = Color(0xFF12231C),
        secondary = Color(0xFF08744B),
        onSecondary = Color(0xFFFFFFFF),
        secondaryContainer = Color(0xFFE8F5EE),
        onSecondaryContainer = Color(0xFF12231C),
        surfaceVariant = Color(0xFFEDF4F0),
        border = Color(0xFFB9C9C1),
        borderVariant = Color(0xFFDCE7E1),
        error = Color(0xFF8B2A2A),
        onError = Color(0xFFFFFFFF),
        errorContainer = Color(0xFFFFF7F7),
        onErrorContainer = Color(0xFF8B2A2A),
    )

private val Dark =
    darkColorScheme(
        primary = Color(0xFF4CC38A),
        onPrimary = Color(0xFF0A1410),
        background = Color(0xFF0C1310),
        onBackground = Color(0xFFE6EDE8),
        surface = Color(0xFF121B16),
        onSurface = Color(0xFFE6EDE8),
        onSurfaceVariant = Color(0xFF9FB3A7),
        primaryContainer = Color(0xFF16302A),
        onPrimaryContainer = Color(0xFFE6EDE8),
        secondary = Color(0xFF4CC38A),
        onSecondary = Color(0xFF0A1410),
        secondaryContainer = Color(0xFF16302A),
        onSecondaryContainer = Color(0xFFE6EDE8),
        surfaceVariant = Color(0xFF080D0A),
        border = Color(0xFF2A3A32),
        borderVariant = Color(0xFF1D2A23),
        error = Color(0xFFE57373),
        onError = Color(0xFF200A0A),
        errorContainer = Color(0xFF3A1A1A),
        onErrorContainer = Color(0xFFE57373),
    )

/** 平台主题；只承载外观，不持久化宿主的主题偏好。 */
@Composable
fun MfTvTheme(
    dark: Boolean = true,
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = if (dark) Dark else Light,
        typography =
            Typography(
                bodyLarge = TextStyle(fontSize = 20.sp, lineHeight = 30.sp),
                labelLarge = TextStyle(fontSize = 20.sp, lineHeight = 30.sp),
                titleLarge = TextStyle(fontSize = 20.sp, lineHeight = 30.sp),
                headlineLarge = TextStyle(fontSize = 39.sp, lineHeight = 49.sp),
                headlineMedium = TextStyle(fontSize = 31.sp, lineHeight = 40.sp),
            ),
        content = {
            CompositionLocalProvider(
                LocalContentColor provides MaterialTheme.colorScheme.onBackground,
                LocalFocusColor provides if (dark) Color(0xFF5CD39A) else Color(0xFF0B7A50),
            ) { content() }
        },
    )
}

/** 受控Action；busy保留标签并阻止重复事件，结果和请求生命周期由宿主持有。 */
@Composable
fun MfTvButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    busy: Boolean = false,
    busyDescription: String = label,
) {
    Button(
        onClick = onClick,
        modifier =
            modifier
                .defaultMinSize(minWidth = 48.dp, minHeight = 48.dp)
                .semantics { if (busy) stateDescription = busyDescription },
        enabled = enabled && !busy,
        scale = ButtonDefaults.scale(focusedScale = 1f),
        border =
            ButtonDefaults.border(
                border = Border.None,
                focusedBorder = focusBorder(),
                pressedBorder = focusBorder(),
                focusedDisabledBorder = focusBorder(),
            ),
        colors =
            ButtonDefaults.colors(
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
                focusedContainerColor = MaterialTheme.colorScheme.primary,
                focusedContentColor = MaterialTheme.colorScheme.onPrimary,
            ),
    ) { Text(label) }
}

/** 通用内容卡；标题/副文是普通值，图片槽由宿主加载且不重复朗读标题。 */
@Composable
fun MfTvContentCard(
    title: String,
    subtitle: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    artwork: @Composable () -> Unit = {},
) {
    Card(
        onClick = onClick,
        modifier =
            modifier
                .defaultMinSize(minHeight = 48.dp),
        scale = CardDefaults.scale(focusedScale = 1f),
        border = CardDefaults.border(focusedBorder = focusBorder(), pressedBorder = focusBorder()),
        colors =
            CardDefaults.colors(
                containerColor = MaterialTheme.colorScheme.surface,
                contentColor = MaterialTheme.colorScheme.onSurface,
                focusedContainerColor = MaterialTheme.colorScheme.surface,
                focusedContentColor = MaterialTheme.colorScheme.onSurface,
            ),
    ) {
        Column(
            Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(Modifier.clearAndSetSemantics {}) { artwork() }
            Column {
                Text(title, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(subtitle, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

/** 恢复提示组合；状态、文案与重试策略由宿主决定，不推导业务结果。 */
@Composable
fun MfTvMessage(
    message: String,
    modifier: Modifier = Modifier,
    action: @Composable ColumnScope.() -> Unit = {},
) {
    Column(modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(message, style = MaterialTheme.typography.bodyLarge)
        action()
    }
}

// 4dp外移留出画布间隔，主操作同色实心背景仍有可辨识焦点边界。
@Composable
private fun focusBorder(): Border =
    Border(
        border = BorderStroke(3.dp, LocalFocusColor.current),
        inset = 4.dp,
        shape = RoundedCornerShape(12.dp),
    )
