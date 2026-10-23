package example.uifoundation.mobile

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import mediaflow.ui.android.MfButton
import mediaflow.ui.android.MfContentCard
import mediaflow.ui.android.MfMessage
import mediaflow.ui.android.MfTheme

/** Standalone local catalog consuming the production library public package. */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { Catalog() }
    }
}

@Composable
private fun Catalog() {
    var menuOpen by rememberSaveable { mutableStateOf(false) }
    var busy by rememberSaveable { mutableStateOf(false) }
    var dark by rememberSaveable { mutableStateOf(false) }
    var count by rememberSaveable { mutableStateOf(0) }
    var draft by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf("All") }
    var selected by rememberSaveable { mutableStateOf<String?>(null) }
    var message by rememberSaveable { mutableStateOf("Content available. All values are synthetic and local.") }
    var modal by rememberSaveable { mutableStateOf(false) }
    var confirmDiscard by rememberSaveable { mutableStateOf(false) }
    var formBusy by rememberSaveable { mutableStateOf(false) }
    var checked by rememberSaveable { mutableStateOf(false) }
    var expanded by rememberSaveable { mutableStateOf(false) }
    MfTheme(dark = dark) {
        Column(
            Modifier
                .fillMaxSize()
                .systemBarsPadding()
                .background(MaterialTheme.colorScheme.background)
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text("Foundation studio", style = MaterialTheme.typography.headlineLarge)
            Text("Android 26+ · independent application · no SDK, service or credentials")
            Text("MfTheme(dark, content) · Material semantic fonts · minimum action 48 dp")
            Row(Modifier.fillMaxWidth().toggleable(value = dark, role = Role.Switch, onValueChange = { dark = it })) {
                Text("Dark theme", Modifier.weight(1f))
                Switch(checked = dark, onCheckedChange = null)
            }
            Text("Evergreen · canvas / surface / primary / error · system font scale")
            Text("Headline", style = MaterialTheme.typography.headlineMedium)
            Text("Title", style = MaterialTheme.typography.titleLarge)
            Text("Body and supporting text follow Material typography", style = MaterialTheme.typography.bodyLarge)
            Text("Spacing: 8 / 12 / 16 / 24 dp. Layout grows with text; no fixed page height.")
            HorizontalDivider()
            Text("C01 · Actions", style = MaterialTheme.typography.titleLarge)
            Text("MfButton(label, onClick, modifier, enabled, busy, busyDescription)")
            MfButton("Reserve seat", onClick = {
                count++
                busy = true
            }, busy = busy, busyDescription = "Reserving seat")
            MfButton("Simulate failure and retry", onClick = {
                busy = false
                message =
                    "Request failed. Draft and previous content are retained."
            })
            MfButton("Unavailable", onClick = { count += 100 }, enabled = false)
            Text("Attempts: $count. Busy retains the label and prevents repeated activation.")
            HorizontalDivider()
            Text("C07 / C13 / C14 · Content", style = MaterialTheme.typography.titleLarge)
            Text("MfContentCard(title, subtitle, onClick, modifier, artwork)")
            MfContentCard("A long title about accessible native interfaces", "Room A · 45 minutes", onClick = {
                selected =
                    "A long title about accessible native interfaces"
            })
            MfContentCard("Unavailable artwork retains a readable title", "Image lifecycle belongs to the host", onClick = {
                selected =
                    "Unavailable artwork retains a readable title"
            })
            selected?.let { title ->
                MfMessage(title) { MfButton("Back to catalog", onClick = { selected = null }) }
            }
            HorizontalDivider()
            Text("C02–C05 · Native form and selection", style = MaterialTheme.typography.titleLarge)
            Text("M3 OutlinedTextField / Switch / Checkbox / FilterChip · host-owned values")
            OutlinedTextField(value = draft, onValueChange = {
                draft = it
            }, label = {
                Text("Display name")
            }, modifier = Modifier.fillMaxWidth(), supportingText = {
                Text(
                    "Keyboard, large font and rotation can be checked with this local draft.",
                )
            })
            Row(Modifier.fillMaxWidth().toggleable(value = checked, role = Role.Checkbox, onValueChange = { checked = it })) {
                Checkbox(checked = checked, onCheckedChange = null)
                Text("Enable local preview")
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("All", "Upcoming").forEach { item ->
                    FilterChip(selected = filter == item, onClick = { filter = item }, label = { Text(item) })
                }
            }
            Box {
                MfButton("Selection menu: $filter", onClick = { menuOpen = true })
                DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                    listOf("All", "Upcoming", "Completed").forEach { item ->
                        DropdownMenuItem(text = { Text(item) }, onClick = {
                            filter = item
                            menuOpen = false
                        })
                    }
                }
            }
            MfButton("Edit modal", onClick = {
                modal = true
                message = ""
            })
            HorizontalDivider()
            Text("C06 / C11 / C12 · Feedback", style = MaterialTheme.typography.titleLarge)
            MfMessage(message) { MfButton("Clear feedback", onClick = { message = "Content available." }) }
            listOf("Loading", "Empty", "No match", "Offline with retained content", "Error", "Stale").forEach { state ->
                MfButton(state, onClick = { message = "$state · local preview only; recovery never automatically repeats an operation." })
            }
            HorizontalDivider()
            Text("C08 / C09 / C15–C17 · Composition", style = MaterialTheme.typography.titleLarge)
            Text("Compose scrolling collection and host navigation; native menus/dialogs remain platform primitives.")
            MfButton(if (expanded) "Collapse information" else "Read complete information", onClick = { expanded = !expanded })
            if (expanded) {
                Text(
                    "A deliberately long local description remains readable at large system fonts. Expanded state, theme, selected filter and draft survive Activity recreation.",
                )
            }
            MfButton("Load more", onClick = {
                count++
                busy = true
            }, busy = busy, busyDescription = "Loading more")
            Text(
                "Manual checks: light/dark; fontScale; TalkBack; keyboard; rotation; modal cancel/discard; failed save retains draft; disabled/busy prevent repeated actions.",
            )
        }
        if (modal) {
            AlertDialog(
                onDismissRequest = {
                    if (!formBusy) {
                        if (draft.isEmpty()) modal = false else confirmDiscard = true
                    }
                },
                title = { Text("Edit local preview · C10") },
                text = {
                    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        OutlinedTextField(
                            value = draft,
                            onValueChange = { draft = it },
                            label = { Text("Display name") },
                            enabled = !formBusy,
                        )
                        Text(message)
                        MfButton("Save preview", onClick = {
                            if (draft.isBlank()) {
                                message = "Enter a display name."
                            } else {
                                formBusy = true
                                message =
                                    "Saving locally…"
                            }
                        }, busy = formBusy, busyDescription = "Saving")
                        MfButton("Fail save", onClick = {
                            formBusy = false
                            message = "Save failed. Your draft is retained."
                        })
                    }
                },
                confirmButton = {
                    TextButton(
                        onClick = {
                            modal = false
                            message = "Local preview saved."
                        },
                        enabled =
                            draft.isNotBlank() && !formBusy,
                    ) { Text("Complete save") }
                },
                dismissButton = {
                    TextButton(
                        onClick = { if (draft.isEmpty()) modal = false else confirmDiscard = true },
                        enabled = !formBusy,
                    ) { Text("Cancel") }
                },
            )
        }
        if (confirmDiscard) {
            AlertDialog(
                onDismissRequest = { confirmDiscard = false },
                title = { Text("Discard this local draft?") },
                confirmButton = { TextButton(onClick = { confirmDiscard = false }) { Text("Continue editing") } },
                dismissButton = {
                    TextButton(onClick = {
                        draft = ""
                        confirmDiscard = false
                        modal = false
                    }) { Text("Discard draft") }
                },
            )
        }
    }
}
