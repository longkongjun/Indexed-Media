import SwiftUI

// labs/design-system/tokens/tokens.json v2.0.0：暂定evergreen语义值。
private enum Palette {
    static func primary(_ scheme: ColorScheme) -> Color {
        scheme == .dark
            ? Color(red: 76 / 255, green: 195 / 255, blue: 138 / 255)
            : Color(red: 8 / 255, green: 116 / 255, blue: 75 / 255)
    }
    static func canvas(_ scheme: ColorScheme) -> Color {
        scheme == .dark
            ? Color(red: 12 / 255, green: 19 / 255, blue: 16 / 255)
            : Color(red: 244 / 255, green: 247 / 255, blue: 245 / 255)
    }
    static func onAction(_ scheme: ColorScheme) -> Color {
        scheme == .dark
            ? Color(red: 10 / 255, green: 20 / 255, blue: 16 / 255)
            : .white
    }
}

/// 语义主题容器；宿主通过系统colorScheme或preferredColorScheme控制偏好。
public struct MfTheme<Content: View>: View {
    @Environment(\.colorScheme) private var scheme
    private let content: Content

    public init(@ViewBuilder content: () -> Content) { self.content = content() }

    public var body: some View {
        content.tint(Palette.primary(scheme)).background(Palette.canvas(scheme))
    }
}

/// 受控主操作；busy保留名称、阻止重复事件，异步请求和结果由宿主维护。
public struct MfButton: View {
    @Environment(\.colorScheme) private var scheme
    private let title: String
    private let enabled: Bool
    private let busy: Bool
    private let busyDescription: String
    private let action: () -> Void

    public init(
        _ title: String,
        enabled: Bool = true,
        busy: Bool = false,
        busyDescription: String = "",
        action: @escaping () -> Void
    ) {
        self.title = title
        self.enabled = enabled
        self.busy = busy
        self.busyDescription = busyDescription
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            Text(title).frame(minWidth: 44, minHeight: 44).fixedSize(horizontal: false, vertical: true)
        }
            .buttonStyle(.borderedProminent)
            .tint(Palette.primary(scheme))
            .foregroundStyle(Palette.onAction(scheme))
            .disabled(!enabled || busy)
            .accessibilityValue(busy ? busyDescription : "")
    }
}

/// 原生List内容行；图片槽由宿主持有，标题两行且详情须能读取完整标题。
public struct MfContentRow<Artwork: View>: View {
    private let title: String
    private let subtitle: String
    private let artwork: Artwork
    private let action: () -> Void

    public init(_ title: String, subtitle: String, action: @escaping () -> Void, @ViewBuilder artwork: () -> Artwork) {
        self.title = title
        self.subtitle = subtitle
        self.action = action
        self.artwork = artwork()
    }

    public var body: some View {
        Button(action: action) {
            HStack(spacing: 16) {
                artwork.accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 8) {
                    Text(title).font(.headline).foregroundStyle(.primary).lineLimit(2)
                    Text(subtitle).font(.subheadline).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }.frame(minHeight: 44).contentShape(Rectangle())
        }
    }
}

/// 常驻恢复提示；不保存请求、业务结果或重试策略。
public struct MfMessage<Action: View>: View {
    private let message: String
    private let action: Action

    public init(_ message: String, @ViewBuilder action: () -> Action) {
        self.message = message
        self.action = action()
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) { Text(message).font(.body); action }
            .padding(16)
    }
}
