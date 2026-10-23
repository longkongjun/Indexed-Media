package mediaflow.ui.android

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

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
        outline = Color(0xFFB9C9C1),
        outlineVariant = Color(0xFFDCE7E1),
        error = Color(0xFF8B2A2A),
        onError = Color(0xFFFFFFFF),
        errorContainer = Color(0xFFFFF7F7),
        onErrorContainer = Color(0xFF8B2A2A),
        surfaceContainerHigh = Color(0xFFFFFFFF),
        surfaceContainerHighest = Color(0xFFEDF4F0),
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
        outline = Color(0xFF2A3A32),
        outlineVariant = Color(0xFF1D2A23),
        error = Color(0xFFE57373),
        onError = Color(0xFF200A0A),
        errorContainer = Color(0xFF3A1A1A),
        onErrorContainer = Color(0xFFE57373),
        surfaceContainerHigh = Color(0xFF17221C),
        surfaceContainerHighest = Color(0xFF080D0A),
    )

/** 平台主题；只承载外观，不持久化宿主的主题偏好。 */
@Composable
fun MfTheme(
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(colorScheme = if (dark) Dark else Light) {
        CompositionLocalProvider(LocalContentColor provides MaterialTheme.colorScheme.onBackground) { content() }
    }
}

/** 受控Action；busy保留标签并阻止重复事件，结果和请求生命周期由宿主持有。 */
@Composable
fun MfButton(
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
    ) { Text(label) }
}

/** 通用内容卡；标题/副文是普通值，图片槽由宿主加载且不重复朗读标题。 */
@Composable
fun MfContentCard(
    title: String,
    subtitle: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    artwork: @Composable () -> Unit = {},
) {
    Card(
        onClick = onClick,
        modifier = modifier.defaultMinSize(minHeight = 48.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Row(
            Modifier.padding(16.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(Modifier.clearAndSetSemantics {}) { artwork() }
            Column {
                Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

/** 恢复提示组合；状态、文案与重试策略由宿主决定，不推导业务结果。 */
@Composable
fun MfMessage(
    message: String,
    modifier: Modifier = Modifier,
    action: @Composable ColumnScope.() -> Unit = {},
) {
    Column(modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(message, style = MaterialTheme.typography.bodyLarge)
        action()
    }
}
