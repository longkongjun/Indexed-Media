import SwiftUI

public enum IndexedMessageKind: Sendable { case information, success, warning, error }

/// Persistent feedback. The host supplies explicit recovery content and owns dismissal.
public struct IndexedMessage<Action: View>: View {
    @Environment(\.indexedBrand) private var brand
    @Environment(\.colorScheme) private var scheme
    private let title: String
    private let detail: String
    private let kind: IndexedMessageKind
    private let action: Action

    public init(_ title: String, detail: String = "", kind: IndexedMessageKind = .information,
                @ViewBuilder action: () -> Action) {
        self.title = title
        self.detail = detail
        self.kind = kind
        self.action = action()
    }

    private var symbol: String {
        switch kind {
        case .information: return "info.circle"
        case .success: return "checkmark.circle"
        case .warning: return "exclamationmark.triangle"
        case .error: return "exclamationmark.circle"
        }
    }

    private var color: Color {
        let palette = brand.palette(for: scheme)
        switch kind {
        case .information: return palette.information
        case .success: return palette.success
        case .warning: return palette.warning
        case .error: return palette.danger
        }
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label(title, systemImage: symbol).font(.headline).foregroundStyle(color)
            if !detail.isEmpty { Text(detail).font(.body).foregroundStyle(brand.palette(for: scheme).textPrimary) }
            action
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .background(color.opacity(0.08), in: RoundedRectangle(cornerRadius: brand.cornerRadius))
    }
}

extension IndexedMessage where Action == EmptyView {
    public init(_ title: String, detail: String = "", kind: IndexedMessageKind = .information) {
        self.init(title, detail: detail, kind: kind) { EmptyView() }
    }
}

public struct IndexedEmptyState<Action: View>: View {
    @Environment(\.indexedBrand) private var brand
    @Environment(\.colorScheme) private var scheme
    private let title: String
    private let detail: String
    private let symbol: String
    private let action: Action

    public init(_ title: String, detail: String, symbol: String = "tray",
                @ViewBuilder action: () -> Action) {
        self.title = title
        self.detail = detail
        self.symbol = symbol
        self.action = action()
    }

    public var body: some View {
        VStack(spacing: 16) {
            Image(systemName: symbol).font(.largeTitle)
                .foregroundStyle(brand.palette(for: scheme).textSecondary).accessibilityHidden(true)
            Text(title).font(.headline).foregroundStyle(brand.palette(for: scheme).textPrimary)
            Text(detail).font(.body).foregroundStyle(brand.palette(for: scheme).textSecondary)
            action
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(24)
    }
}
