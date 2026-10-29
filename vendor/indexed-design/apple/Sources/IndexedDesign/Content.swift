import SwiftUI

public struct IndexedCard<Content: View>: View {
    @Environment(\.indexedBrand) private var brand
    @Environment(\.colorScheme) private var scheme
    private let title: String
    private let content: Content

    public init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title).font(.headline).foregroundStyle(brand.palette(for: scheme).textPrimary)
                .accessibilityAddTraits(.isHeader)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .background(brand.palette(for: scheme).surface,
                    in: RoundedRectangle(cornerRadius: brand.cornerRadius))
        .overlay(RoundedRectangle(cornerRadius: brand.cornerRadius)
            .stroke(brand.palette(for: scheme).border, lineWidth: 1))
    }
}

/// Content action suitable for a native List; navigation is owned by the host.
public struct IndexedContentRow<Artwork: View>: View {
    @Environment(\.indexedBrand) private var brand
    @Environment(\.colorScheme) private var scheme
    private let title: String
    private let subtitle: String
    private let artwork: Artwork
    private let action: () -> Void

    public init(_ title: String, subtitle: String, action: @escaping () -> Void,
                @ViewBuilder artwork: () -> Artwork) {
        self.title = title
        self.subtitle = subtitle
        self.action = action
        self.artwork = artwork()
    }

    public var body: some View {
        Button(action: action) {
            HStack(spacing: 16) {
                artwork.accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 6) {
                    Text(title).font(.headline).foregroundStyle(brand.palette(for: scheme).textPrimary)
                    Text(subtitle).font(.subheadline).foregroundStyle(brand.palette(for: scheme).textSecondary)
                }
                Spacer(minLength: 0)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
    }
}

public struct IndexedTag: View {
    @Environment(\.indexedBrand) private var brand
    @Environment(\.colorScheme) private var scheme
    private let title: String

    public init(_ title: String) { self.title = title }

    public var body: some View {
        Text(title).font(.caption).padding(.horizontal, 10).padding(.vertical, 5)
            .foregroundStyle(brand.palette(for: scheme).accent)
            .background(brand.palette(for: scheme).accent.opacity(0.12), in: Capsule())
    }
}
