import MediaFlowUI
import SwiftUI

@main
struct Consumer: App {
    var body: some Scene { WindowGroup { FoundationCatalog() } }
}

private struct FoundationCatalog: View {
    @FocusState private var editorFocused: Bool
    @State private var theme = "System"
    @State private var busy = false
    @State private var attempts = 0
    @State private var selectedTitle: String?
    @State private var sheet = false
    @State private var expanded = false
    @State private var enabled = true
    @State private var mode = "List"
    @State private var draft = ""
    @State private var error = ""
    @State private var formBusy = false
    @State private var confirmDiscard = false
    @State private var selection = "All"

    private var colorScheme: ColorScheme? { theme == "System" ? nil : (theme == "Dark" ? .dark : .light) }

    var body: some View {
        MfTheme {
            NavigationStack {
                List {
                    Section("Foundation studio · iOS 16+") {
                        Text("Independent application · MediaFlowUI public product · synthetic local values · no service or credentials.")
                        Picker("Theme", selection: $theme) {
                            ForEach(["System", "Light", "Dark"], id: \.self) { Text($0) }
                        }
                        Text("Headline").font(.title)
                        Text("Body and supporting text use semantic fonts").font(.body)
                        Text("Evergreen tint · native surfaces · system contrast and font scaling").font(.caption)
                        Text("MfTheme { content } · system semantic fonts · 44 pt minimum actions")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Section("C01 · Actions") {
                        Text("MfButton(title, enabled, busy, busyDescription, action)").font(.caption)
                        Text("Attempts: \(attempts)").accessibilityIdentifier("attempts")
                        MfButton("Reserve seat", busy: busy, busyDescription: "Reserving") { attempts += 1; busy = true }
                        MfButton("Simulate failure") { busy = false; error = "The request failed. Values are retained; retry is explicit." }
                        MfButton("Unavailable", enabled: false) { attempts += 100 }
                        Text("The name remains while busy. The host owns requests and failure recovery.")
                    }
                    Section("C07 / C13 / C14 · Content") {
                        Text("MfContentRow(title, subtitle, action, artwork) · artwork is decorative").font(.caption)
                        MfContentRow(
                            "Accessible interfaces: a longer conference session title",
                            subtitle: "Room A · 45 minutes",
                            action: { selectedTitle = "Accessible interfaces: a longer conference session title" },
                            artwork: { Image(systemName: "calendar").frame(width: 56, height: 84).background(.quaternary) }
                        )
                        MfContentRow(
                            "Unavailable artwork retains the complete readable title",
                            subtitle: "Image loading belongs to the host",
                            action: { selectedTitle = "Unavailable artwork retains the complete readable title" },
                            artwork: { Image(systemName: "photo").frame(width: 56, height: 84).background(.quaternary) }
                        )
                    }
                    Section("C02–C05 · Native form and selection") {
                        Text("SwiftUI TextField / Toggle / Picker · values owned by this application").font(.caption)
                        TextField("Display name", text: $draft).textInputAutocapitalization(.never).accessibilityIdentifier("catalog-name")
                        Toggle("Enable local preview", isOn: $enabled)
                        Picker("Presentation", selection: $mode) {
                            Text("List").tag("List"); Text("Grid").tag("Grid")
                        }.pickerStyle(.segmented)
                        Picker("Filter", selection: $selection) {
                            ForEach(["All", "Upcoming", "Completed"], id: \.self) { Text($0) }
                        }
                        MfButton("Edit modal") { sheet = true; error = "" }
                    }
                    Section("C06 / C11 / C12 · Feedback and recovery") {
                        Label("Synthetic preview", systemImage: "info.circle")
                        MfMessage(error.isEmpty ? "Content is available. Choose a state or simulate a failure." : error) {
                            MfButton("Clear feedback") { error = "" }
                        }
                        Menu("Preview request state") {
                            ForEach([
                                "Loading · actions disabled", "Empty · no records", "No match · clear filter",
                                "Offline · retained content", "Error · explicit retry", "Stale · checking latest values"
                            ], id: \.self) { value in
                                Button(value) { error = value }
                            }
                        }
                    }
                    Section("C08 / C09 / C15–C17 · Native composition") {
                        Text("List / NavigationStack / DisclosureGroup / Menu · stable IDs and host navigation")
                        DisclosureGroup("Read complete information", isExpanded: $expanded) {
                            Text(
                                "A deliberately long local description can be read at large Dynamic Type sizes. " +
                                "Expanding and closing does not issue any remote operation."
                            )
                        }
                        MfButton("Load more", busy: busy, busyDescription: "Loading more") { attempts += 1; busy = true }
                        Text(
                            "Manual checks: light/dark; Dynamic Type; VoiceOver order; keyboard; dirty modal cancellation; " +
                            "failed save retains draft; detail back; disabled/busy prevent repeated actions."
                        )
                    }
                }
                .navigationTitle("Foundation studio")
                .navigationDestination(isPresented: Binding(
                    get: { selectedTitle != nil },
                    set: { if !$0 { selectedTitle = nil } }
                )) {
                    Text(selectedTitle ?? "").font(.title).padding().navigationTitle("Complete title")
                }
            }
        }
        .preferredColorScheme(colorScheme)
        .sheet(isPresented: $sheet) { editor }
    }

    private var editor: some View {
        NavigationStack {
            Form {
                Section("Native sheet · C10") {
                    TextField("Display name", text: $draft)
                        .focused($editorFocused)
                        .submitLabel(.done)
                        .onSubmit { editorFocused = false }
                        .disabled(formBusy)
                        .accessibilityIdentifier("modal-name")
                    Text("A failed save retains this draft. Cancel requires explicit discard when the value is nonempty.")
                    if !error.isEmpty { Text(error).accessibilityIdentifier("modal-error") }
                    MfButton("Save preview", busy: formBusy, busyDescription: "Saving") {
                        editorFocused = false
                        if draft.trimmingCharacters(in: .whitespaces).isEmpty {
                            error = "Enter a display name."
                        } else {
                            formBusy = true
                            error = "Saving locally…"
                        }
                    }
                    MfButton("Fail save") { formBusy = false; error = "Save failed. Your draft is retained." }
                    MfButton("Complete save", enabled: !draft.isEmpty && !formBusy) { sheet = false; error = "Local preview saved." }
                }
            }
            .navigationTitle("Edit local preview")
            .toolbar { ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { if draft.isEmpty { sheet = false } else { confirmDiscard = true } }.disabled(formBusy)
            } }
            .interactiveDismissDisabled(!draft.isEmpty || formBusy)
            .alert("Discard this local draft?", isPresented: $confirmDiscard) {
                Button("Discard draft", role: .destructive) { draft = ""; sheet = false }
                Button("Continue editing", role: .cancel) {}
            }
        }
    }
}
