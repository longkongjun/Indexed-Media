import MediaFlowUI
import SwiftUI

@main
struct Consumer: App {
    @State private var busy = false
    @State private var attempts = 0

    var body: some Scene {
        WindowGroup {
            MfTheme {
                NavigationStack {
                    List {
                        MfButton("Reserve seat", busy: busy, busyDescription: "Reserving") { attempts += 1; busy = true }
                        MfButton("Simulate failure") { busy = false }
                        Text("Attempts: \(attempts)").accessibilityIdentifier("attempts")
                        MfContentRow(
                            "Accessible interfaces: a longer conference session title",
                            subtitle: "Room A · 45 minutes",
                            action: { attempts += 1 }
                        ) {
                            Image(systemName: "calendar")
                        }
                    }.navigationTitle("Conference")
                }
            }
        }
    }
}
