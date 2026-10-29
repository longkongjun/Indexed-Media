import SwiftUI

/// Semantic colors are resolved by the host's system appearance. No global mutable theme.
public struct IndexedPalette: Sendable {
    public let accent: Color
    public let canvas: Color
    public let surface: Color
    public let border: Color
    public let danger: Color
    public let success: Color
    public let warning: Color
    public let information: Color
    public let textPrimary: Color
    public let textSecondary: Color

    public init(accent: Color, canvas: Color, surface: Color, border: Color,
                danger: Color = .red, success: Color = .green,
                warning: Color = .orange, information: Color = .blue,
                textPrimary: Color = .primary, textSecondary: Color = .secondary) {
        self.accent = accent
        self.canvas = canvas
        self.surface = surface
        self.border = border
        self.danger = danger
        self.success = success
        self.warning = warning
        self.information = information
        self.textPrimary = textPrimary
        self.textSecondary = textSecondary
    }
}

public struct IndexedBrand: Sendable {
    public let light: IndexedPalette
    public let dark: IndexedPalette
    public let cornerRadius: CGFloat

    public init(light: IndexedPalette, dark: IndexedPalette, cornerRadius: CGFloat = 12) {
        self.light = light
        self.dark = dark
        self.cornerRadius = max(0, cornerRadius)
    }

    public func palette(for scheme: ColorScheme) -> IndexedPalette {
        scheme == .dark ? dark : light
    }

    /// Evergreen starter theme. Replace the complete brand at the application root.
    public static let evergreen = IndexedBrand(
        light: IndexedPalette(accent: Color(red: 8 / 255, green: 116 / 255, blue: 75 / 255),
                              canvas: Color(red: 244 / 255, green: 247 / 255, blue: 245 / 255),
                              surface: .white, border: Color(indexedHex: 0xb9c9c1),
                              danger: Color(indexedHex: 0x8b2a2a), success: Color(indexedHex: 0x0d5c30),
                              warning: Color(indexedHex: 0x7d5f0e), information: Color(indexedHex: 0x1d5fa8),
                              textPrimary: Color(indexedHex: 0x12231c), textSecondary: Color(indexedHex: 0x50655c)),
        dark: IndexedPalette(accent: Color(red: 76 / 255, green: 195 / 255, blue: 138 / 255),
                             canvas: Color(red: 12 / 255, green: 19 / 255, blue: 16 / 255),
                             surface: Color(indexedHex: 0x121b16), border: Color(indexedHex: 0x2a3a32),
                             danger: Color(indexedHex: 0xe57373), success: Color(indexedHex: 0x58c98d),
                             warning: Color(indexedHex: 0xd9a53f), information: Color(indexedHex: 0x6aa9e8),
                             textPrimary: Color(indexedHex: 0xe6ede8), textSecondary: Color(indexedHex: 0x9fb3a7))
    )
}

private extension Color {
    init(indexedHex value: UInt32) {
        self.init(red: Double((value >> 16) & 255) / 255,
                  green: Double((value >> 8) & 255) / 255,
                  blue: Double(value & 255) / 255)
    }
}

private struct IndexedBrandKey: EnvironmentKey {
    static let defaultValue = IndexedBrand.evergreen
}

extension EnvironmentValues {
    public var indexedBrand: IndexedBrand {
        get { self[IndexedBrandKey.self] }
        set { self[IndexedBrandKey.self] = newValue }
    }
}

/// ColorScheme preference remains a host concern (`preferredColorScheme(nil/.light/.dark)`).
public struct IndexedTheme<Content: View>: View {
    @Environment(\.colorScheme) private var scheme
    private let brand: IndexedBrand
    private let content: Content

    public init(brand: IndexedBrand = .evergreen, @ViewBuilder content: () -> Content) {
        self.brand = brand
        self.content = content()
    }

    public var body: some View {
        content.environment(\.indexedBrand, brand)
            .tint(brand.palette(for: scheme).accent)
            .background(brand.palette(for: scheme).canvas)
    }
}
