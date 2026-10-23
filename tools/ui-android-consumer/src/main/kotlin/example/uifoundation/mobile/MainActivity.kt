package example.uifoundation.mobile

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import mediaflow.ui.android.MfButton
import mediaflow.ui.android.MfContentCard
import mediaflow.ui.android.MfMessage
import mediaflow.ui.android.MfTheme

/** 不依赖SDK、网络或业务实体的公开入口消费者。 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { Catalog() }
    }
}

@Composable
private fun Catalog() {
    var busy by remember { mutableStateOf(false) }
    var dark by remember { mutableStateOf(false) }
    var count by rememberSaveable { mutableStateOf(0) }
    MfTheme(dark = dark) {
        Column(
            Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text("Conference sessions", style = MaterialTheme.typography.headlineLarge)
            MfButton("Toggle theme", onClick = { dark = !dark })
            MfButton("Reserve seat", onClick = {
                count++
                busy = true
            }, busy = busy, busyDescription = "Reserving seat")
            MfButton("Simulate failure and retry", onClick = { busy = false })
            MfMessage("Attempts: $count. The host owns the pending and failure state.")
            MfContentCard("A long title about accessible native interfaces", "Room A · 45 minutes", onClick = { count++ })
        }
    }
}
