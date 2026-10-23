package example.uifoundation.tv

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import mediaflow.ui.tv.MfTvButton
import mediaflow.ui.tv.MfTvContentCard
import mediaflow.ui.tv.MfTvMessage
import mediaflow.ui.tv.MfTvTheme

/** 不依赖SDK、网络或业务实体的公开入口消费者。 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { MfTvTheme { Catalog() } }
    }
}

@Composable
private fun Catalog() {
    var ids by remember { mutableStateOf((1..12).map { "session-$it" }) }
    var selected by rememberSaveable { mutableStateOf<String?>(null) }
    var lastId by rememberSaveable { mutableStateOf("session-1") }
    var lastIndex by rememberSaveable { mutableStateOf(0) }
    val detailFocus = remember { FocusRequester() }
    val resetFocus = remember { FocusRequester() }
    val requesters = remember(ids) { ids.associateWith { FocusRequester() } }
    BackHandler(enabled = selected != null) { selected = null }
    LaunchedEffect(selected, ids) {
        when {
            selected != null -> detailFocus.requestFocus()
            ids.isEmpty() -> resetFocus.requestFocus()
            else -> requesters[lastId]?.requestFocus()
        }
    }
    Column(
        Modifier
            .fillMaxSize()
            .background(
                MaterialTheme.colorScheme.background,
            ).verticalScroll(rememberScrollState())
            .padding(horizontal = 48.dp, vertical = 27.dp),
        verticalArrangement = Arrangement.spacedBy(24.dp),
    ) {
        Text("Conference sessions", style = MaterialTheme.typography.headlineLarge)
        if (selected != null) {
            MfTvButton("Back to sessions", onClick = { selected = null }, modifier = Modifier.focusRequester(detailFocus))
            Text("Session $selected: accessibility and resilient interfaces", style = MaterialTheme.typography.headlineMedium)
            Text("A long description can be read without a network or a MediaFlow SDK.", fontSize = 20.sp)
        } else {
            MfTvButton("Remove focused session", onClick = {
                val index = ids.indexOf(lastId).coerceAtLeast(0)
                ids = ids.filterNot { it == lastId }
                lastIndex = index.coerceAtMost((ids.size - 1).coerceAtLeast(0))
                lastId = ids.getOrNull(lastIndex).orEmpty()
            }, enabled = ids.isNotEmpty())
            if (ids.isEmpty()) {
                MfTvMessage("No sessions remain.") {
                    MfTvButton("Reset sessions", modifier = Modifier.focusRequester(resetFocus), onClick = {
                        ids = (1..12).map { "session-$it" }
                        lastId = ids.first()
                    })
                }
            }
            ids.chunked(4).forEach { row ->
                Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                    row.forEach { id ->
                        key(id) {
                            MfTvContentCard(
                                title = "Session $id: a longer title about accessible interfaces",
                                subtitle = "Room A · 45 minutes",
                                onClick = { selected = id },
                                modifier =
                                    Modifier.weight(1f).focusRequester(requesters.getValue(id)).onFocusChanged {
                                        if (it.isFocused) {
                                            lastId = id
                                            lastIndex = ids.indexOf(id)
                                        }
                                    },
                            )
                        }
                    }
                    repeat(4 - row.size) { Spacer(Modifier.weight(1f)) }
                }
            }
        }
    }
}
