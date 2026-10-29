import SwiftUI

/// Label/help/error composition around native inputs. Binding, validation and keyboard stay native.
public struct IndexedFormField<Content: View>: View {
    @Environment(\.indexedBrand) private var brand
    @Environment(\.colorScheme) private var scheme
    private let label: String
    private let help: String
    private let error: String?
    private let content: Content

    public init(_ label: String, help: String = "", error: String? = nil,
                @ViewBuilder content: () -> Content) {
        self.label = label
        self.help = help
        self.error = error
        self.content = content()
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label).font(.headline).foregroundStyle(brand.palette(for: scheme).textPrimary)
            content.accessibilityLabel(label)
            if let error, !error.isEmpty {
                Label(error, systemImage: "exclamationmark.circle")
                    .font(.subheadline).foregroundStyle(brand.palette(for: scheme).danger)
            } else if !help.isEmpty {
                Text(help).font(.subheadline).foregroundStyle(brand.palette(for: scheme).textSecondary)
            }
        }
    }
}
