import SwiftUI

/// The host controls transitions. Errors never discard host form values.
public enum IndexedActionState: Equatable, Sendable {
    case ready
    case busy(description: String)
    case failed(message: String)

    public var isBusy: Bool {
        if case .busy = self { return true }
        return false
    }

    public func canActivate(enabled: Bool) -> Bool { enabled && !isBusy }

    public var accessibilityValue: String {
        switch self {
        case .ready: return ""
        case .busy(let description): return description
        case .failed(let message): return message
        }
    }
}

public enum IndexedActionVariant: Sendable { case primary, secondary, destructive }

/// Uses SwiftUI's native button styles, activation, focus and accessibility.
public struct IndexedButton: View {
    @Environment(\.indexedBrand) private var brand
    @Environment(\.colorScheme) private var scheme
    private let title: String
    private let variant: IndexedActionVariant
    private let state: IndexedActionState
    private let enabled: Bool
    private let action: () -> Void

    public init(_ title: String, variant: IndexedActionVariant = .primary,
                state: IndexedActionState = .ready, enabled: Bool = true,
                action: @escaping () -> Void) {
        self.title = title
        self.variant = variant
        self.state = state
        self.enabled = enabled
        self.action = action
    }

    private var button: some View {
        Button(role: variant == .destructive ? .destructive : nil) {
            guard state.canActivate(enabled: enabled) else { return }
            action()
        } label: {
            HStack(spacing: 8) {
                if state.isBusy { ProgressView().accessibilityHidden(true) }
                Text(title).fixedSize(horizontal: false, vertical: true)
            }
            #if os(iOS)
            .frame(minHeight: 44)
            #endif
        }
        .disabled(!enabled)
        .tint(variant == .destructive ? brand.palette(for: scheme).danger : brand.palette(for: scheme).accent)
        .accessibilityValue(state.accessibilityValue)
        .accessibilityHint(state.isBusy ? "进行中，重复激活会被忽略。" : "")
    }

    @ViewBuilder public var body: some View {
        if variant == .primary {
            button.buttonStyle(.borderedProminent)
        } else {
            button.buttonStyle(.bordered)
        }
    }
}
